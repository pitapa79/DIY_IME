use super::Profile;
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};

// A profile owns its SID allocation and is moved, never shared, across threads.
// Windows profile handles have no thread affinity; cleanup remains with the owner.
struct OwnedProfile(Profile);
unsafe impl Send for OwnedProfile {}

/// One unused profile per runtime factory. No permissions or input are staged.
pub(super) struct ProfilePool {
    request: Option<mpsc::SyncSender<()>>,
    ready: Mutex<mpsc::Receiver<OwnedProfile>>,
    worker: Option<JoinHandle<()>>,
}
impl ProfilePool {
    pub(super) fn start() -> Option<Self> {
        let (request, requests) = mpsc::sync_channel::<()>(1);
        let (send, ready) = mpsc::sync_channel::<OwnedProfile>(1);
        let worker = thread::Builder::new()
            .name("diyime-profile-preparation".into())
            .spawn(move || {
                while requests.recv().is_ok() {
                    if let Ok(profile) = Profile::new() {
                        // Never wait for a consumer on shutdown. If surplus cleanup
                        // fails, stop refilling instead of accumulating profiles.
                        if let Err(error) = send.try_send(OwnedProfile(profile)) {
                            let OwnedProfile(mut surplus) = match error {
                                mpsc::TrySendError::Full(p)
                                | mpsc::TrySendError::Disconnected(p) => p,
                            };
                            if surplus.delete().is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .ok()?;
        let _ = request.try_send(());
        Some(Self {
            request: Some(request),
            ready: Mutex::new(ready),
            worker: Some(worker),
        })
    }
    pub(super) fn take(&self) -> Option<Profile> {
        let profile = self
            .ready
            .try_lock()
            .ok()
            .and_then(|ready| ready.try_recv().ok());
        // Refill is bounded and never delays an invocation. A failed refill is
        // retried on a later call; the caller retains the ordinary fallback.
        if let Some(request) = &self.request {
            let _ = request.try_send(());
        }
        profile.map(|profile| profile.0)
    }
}
impl Drop for ProfilePool {
    fn drop(&mut self) {
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
        let pool = ProfilePool::start().unwrap();
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
    fn shutdown_drains_an_unused_profile() {
        let profile = Profile::new().unwrap();
        let name = profile.name();
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(OwnedProfile(profile)).ok().unwrap();
        let pool = ProfilePool {
            request: None,
            ready: Mutex::new(receiver),
            worker: None,
        };
        assert!(try_create(&name) < 0);
        drop(pool);
        assert_eq!(try_create(&name), 0);
        delete(&name);
    }
}
