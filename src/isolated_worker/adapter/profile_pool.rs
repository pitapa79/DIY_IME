use super::Profile;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

// A profile owns its SID allocation and is moved, never shared, across threads.
// Windows profile handles have no thread affinity; cleanup remains with the owner.
struct OwnedProfile(Profile);
unsafe impl Send for OwnedProfile {}

// Cover a four-way burst without staging permissions, inputs or processes.
const SPARE_PROFILES: usize = 4;

/// Bounded unused profiles per runtime factory.
pub(super) struct ProfilePool {
    request: Option<mpsc::SyncSender<()>>,
    ready: Mutex<mpsc::Receiver<OwnedProfile>>,
    worker: Option<JoinHandle<()>>,
    occupied: Arc<AtomicUsize>,
    closing: Arc<AtomicBool>,
    capacity: usize,
}
impl ProfilePool {
    pub(super) fn start(physical_slots: u64) -> Option<Self> {
        let capacity = physical_slots.min(SPARE_PROFILES as u64) as usize;
        if capacity == 0 {
            return None;
        }
        let (request, requests) = mpsc::sync_channel::<()>(capacity);
        let (send, ready) = mpsc::sync_channel::<OwnedProfile>(capacity);
        let occupied = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(AtomicBool::new(false));
        let pending = occupied.clone();
        let stop = closing.clone();
        let worker = thread::Builder::new()
            .name("diyime-profile-preparation".into())
            .spawn(move || {
                while requests.recv().is_ok() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if let Ok(profile) = Profile::new() {
                        // Never wait for a consumer on shutdown. If surplus cleanup
                        // fails, stop refilling instead of accumulating profiles.
                        if let Err(error) = send.try_send(OwnedProfile(profile)) {
                            pending.fetch_sub(1, Ordering::AcqRel);
                            let OwnedProfile(mut surplus) = match error {
                                mpsc::TrySendError::Full(p)
                                | mpsc::TrySendError::Disconnected(p) => p,
                            };
                            if surplus.delete().is_err() {
                                break;
                            }
                        }
                    } else {
                        pending.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })
            .ok()?;
        let pool = Self {
            request: Some(request),
            ready: Mutex::new(ready),
            worker: Some(worker),
            occupied,
            closing,
            capacity,
        };
        for _ in 0..capacity {
            pool.refill();
        }
        Some(pool)
    }
    pub(super) fn take(&self) -> Option<Profile> {
        let profile = self
            .ready
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .try_recv()
            .ok();
        if profile.is_some() {
            self.occupied.fetch_sub(1, Ordering::AcqRel);
        }
        // Refill is bounded and never delays an invocation. A failed refill is
        // retried on a later call; the caller retains the ordinary fallback.
        self.refill();
        profile.map(|profile| profile.0)
    }

    fn refill(&self) {
        if let Some(request) = &self.request
            && self
                .occupied
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    (count < self.capacity).then_some(count + 1)
                })
                .is_ok()
            && request.try_send(()).is_err()
        {
            self.occupied.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
impl Drop for ProfilePool {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.request.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // Explicitly delete the unused spare after the producer has stopped.
        if let Ok(ready) = self.ready.get_mut() {
            while let Ok(OwnedProfile(mut profile)) = ready.try_recv() {
                let _ = profile.delete();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Security::{FreeSid, Isolation::CreateAppContainerProfile};
    fn try_create(name: &str) -> i32 {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut sid = std::ptr::null_mut();
        let hr = unsafe {
            CreateAppContainerProfile(
                wide.as_ptr(),
                wide.as_ptr(),
                wide.as_ptr(),
                std::ptr::null(),
                0,
                &mut sid,
            )
        };
        if !sid.is_null() {
            unsafe {
                FreeSid(sid);
            }
        }
        hr
    }
    fn wait(pool: &ProfilePool) -> Profile {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(profile) = pool.take() {
                return profile;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn delete(name: &str) {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        assert!(
            unsafe {
                windows_sys::Win32::Security::Isolation::DeleteAppContainerProfile(wide.as_ptr())
            } >= 0
        );
    }
    #[test]
    fn single_use_profiles_are_distinct_and_released() {
        let pool = ProfilePool::start(1).unwrap();
        assert_eq!(pool.capacity, 1);
        let first = wait(&pool);
        let second = wait(&pool);
        let name = first.name();
        assert_ne!(name, second.name());
        assert!(try_create(&name) < 0);
        drop(first);
        assert_eq!(try_create(&name), 0);
        delete(&name);
        drop(second);
        drop(pool);
    }
    #[test]
    fn shutdown_drains_all_unused_profiles() {
        let (sender, receiver) = mpsc::sync_channel(SPARE_PROFILES);
        let names: Vec<_> = (0..SPARE_PROFILES)
            .map(|_| {
                let profile = Profile::new().unwrap();
                let name = profile.name();
                sender.send(OwnedProfile(profile)).ok().unwrap();
                assert!(try_create(&name) < 0);
                name
            })
            .collect();
        let pool = ProfilePool {
            request: None,
            ready: Mutex::new(receiver),
            worker: None,
            occupied: Arc::new(AtomicUsize::new(SPARE_PROFILES)),
            closing: Arc::new(AtomicBool::new(false)),
            capacity: SPARE_PROFILES,
        };
        drop(pool);
        for name in names {
            assert_eq!(try_create(&name), 0);
            delete(&name);
        }
    }
}
