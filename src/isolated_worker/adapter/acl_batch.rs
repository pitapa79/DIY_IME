//! Private same-cache ACL coalescing. No worker, input or grant lifetime owner.
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::Instant,
};
use windows_sys::Win32::Security::{CopySid, GetLengthSid, IsValidSid, PSID};

const MAX_PENDING: usize = 64;

pub(super) struct Sid(Vec<u32>);
impl Sid {
    pub(super) fn copy(profile: &super::Profile) -> Result<Self, String> {
        let sid = profile.sid;
        if unsafe { IsValidSid(sid) } == 0 {
            return Err("invalid private profile SID".into());
        }
        let length = unsafe { GetLengthSid(sid) };
        if !(8..=68).contains(&length) {
            return Err("private profile SID exceeds bound".into());
        }
        let mut words = vec![0u32; (length as usize).div_ceil(4)];
        if unsafe { CopySid(length, words.as_mut_ptr().cast(), sid) } == 0 {
            return Err("private profile SID copy failed".into());
        }
        Ok(Self(words))
    }
    pub(super) fn raw(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }
}
pub(super) struct Change {
    sid: Sid,
    pub(super) allow: bool,
    queued: Instant,
    result: OnceLock<Result<u64, String>>,
}
impl Change {
    pub(super) fn sid(&self) -> PSID {
        self.sid.raw()
    }
}
struct State {
    healthy: bool,
    active: bool,
    pending: VecDeque<Arc<Change>>,
}
pub(super) struct Batch {
    state: Mutex<State>,
    changed: Condvar,
}
impl Batch {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                healthy: true,
                active: false,
                pending: VecDeque::with_capacity(MAX_PENDING),
            }),
            changed: Condvar::new(),
        }
    }
    pub(super) fn check_healthy(&self) -> Result<(), String> {
        if self.state.lock().map_err(|_| "ACL batch poisoned")?.healthy {
            Ok(())
        } else {
            Err("batched code ACL health lost".into())
        }
    }
    /// A caller remains the synchronous owner until its exact change settles.
    /// A competing caller may apply its SID, never its input/profile/process.
    pub(super) fn change(
        &self,
        sid: Sid,
        allow: bool,
        apply: impl FnOnce(&[Arc<Change>]) -> Result<(), String>,
    ) -> Result<u64, String> {
        let request = Arc::new(Change {
            sid,
            allow,
            queued: Instant::now(),
            result: OnceLock::new(),
        });
        let mut state = self.state.lock().map_err(|_| "ACL batch poisoned")?;
        while state.pending.len() == MAX_PENDING {
            if allow && !state.healthy {
                return Err("batched code ACL health lost".into());
            }
            state = self.changed.wait(state).map_err(|_| "ACL batch poisoned")?;
        }
        if allow && !state.healthy {
            return Err("batched code ACL health lost".into());
        }
        state.pending.push_back(request.clone());
        loop {
            if let Some(result) = request.result.get() {
                return result.clone();
            }
            if !state.active {
                // No fixed coalescing sleep. Only requests already queued while
                // a previous filesystem update ran can share the next update.
                state.active = true;
                let healthy = state.healthy;
                let mut selected = Vec::with_capacity(state.pending.len());
                let mut waiting = Vec::with_capacity(state.pending.len());
                for change in state.pending.drain(..) {
                    if change.allow && !healthy {
                        let _ = change
                            .result
                            .set(Err("batched code ACL health lost".into()));
                    } else {
                        waiting
                            .push(change.queued.elapsed().as_micros().min(u64::MAX as u128) as u64);
                        selected.push(change);
                    }
                }
                self.changed.notify_all();
                drop(state);
                // A failed or panicking update cannot strand waiters or restore
                // health. Revocations can still run after a failure; grants cannot.
                let result = if selected.is_empty() {
                    Ok(())
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(&selected)))
                        .unwrap_or_else(|_| Err("batched code ACL update panicked".into()))
                };
                state = self.state.lock().map_err(|_| "ACL batch poisoned")?;
                if result.is_err() {
                    state.healthy = false;
                }
                for (change, wait) in selected.iter().zip(waiting) {
                    let _ = change.result.set(result.clone().map(|()| wait));
                }
                state.active = false;
                self.changed.notify_all();
                // This caller's request was in the drained queue, so it never
                // monopolizes subsequent batches or needs its callback twice.
                return request.result.get().expect("selected ACL caller").clone();
            }
            state = self.changed.wait(state).map_err(|_| "ACL batch poisoned")?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};
    fn sid(id: u32) -> Sid {
        // Aligned synthetic S-1-15-2-id, never used to execute a Worker.
        Sid(vec![0x0000_0201, 0x0f00_0000, 2, id])
    }
    #[test]
    fn competing_changes_coalesce_without_losing_individual_ownership() {
        let batch = Batch::new();
        let sizes = Mutex::new(Vec::new());
        thread::scope(|scope| {
            let (started, observed) = mpsc::channel();
            let (resume, gate) = mpsc::channel();
            let first_batch = &batch;
            let first_sizes = &sizes;
            let first = scope.spawn(move || {
                first_batch.change(sid(1), true, |changes| {
                    first_sizes.lock().unwrap().push(changes.len());
                    started.send(()).unwrap();
                    gate.recv_timeout(Duration::from_secs(2)).unwrap();
                    Ok(())
                })
            });
            observed.recv_timeout(Duration::from_secs(2)).unwrap();
            let rest: Vec<_> = (2..=5)
                .map(|id| {
                    let batch = &batch;
                    let sizes = &sizes;
                    scope.spawn(move || {
                        batch.change(sid(id), id % 2 == 0, |changes| {
                            sizes.lock().unwrap().push(changes.len());
                            let mut ids = changes
                                .iter()
                                .map(|change| {
                                    let id = change.sid.0[3];
                                    assert_eq!(change.allow, id % 2 == 0);
                                    id
                                })
                                .collect::<Vec<_>>();
                            ids.sort();
                            assert_eq!(ids, [2, 3, 4, 5]);
                            Ok(())
                        })
                    })
                })
                .collect();
            let deadline = Instant::now() + Duration::from_secs(2);
            while batch.state.lock().unwrap().pending.len() != 4 {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            resume.send(()).unwrap();
            assert!(first.join().unwrap().is_ok());
            for owner in rest {
                assert!(owner.join().unwrap().is_ok());
            }
        });
        assert_eq!(*sizes.lock().unwrap(), [1, 4]);
        assert!(batch.check_healthy().is_ok());
        assert!(!batch.state.lock().unwrap().active);
        assert!(batch.state.lock().unwrap().pending.is_empty());
    }
    #[test]
    fn failed_update_denies_new_grants_but_still_attempts_revocation() {
        let batch = Batch::new();
        assert!(
            batch
                .change(sid(1), true, |_| Err("synthetic partial failure".into()))
                .is_err()
        );
        assert!(
            batch
                .change(sid(2), true, |_| panic!("unhealthy grant applied"))
                .is_err()
        );
        assert!(
            batch
                .change(sid(1), false, |changes| {
                    assert_eq!(changes.len(), 1);
                    assert!(!changes[0].allow);
                    Ok(())
                })
                .is_ok()
        );
        assert!(
            batch.check_healthy().is_err(),
            "revocation repaired unknown health"
        );
    }
    #[test]
    fn panicking_update_settles_owner_and_keeps_revocation_possible() {
        let batch = Batch::new();
        assert!(
            batch
                .change(sid(1), true, |_| panic!("synthetic update panic"))
                .is_err()
        );
        assert!(batch.change(sid(1), false, |_| Ok(())).is_ok());
        assert!(batch.check_healthy().is_err());
    }

    #[test]
    fn real_batched_acl_changes_preserve_each_sid_and_inherited_file_rights() {
        use super::super::{CodeBudget, CodeFiles, Profile};
        use std::{path::Path, ptr};
        use windows_sys::Win32::{
            Security::{
                ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation,
                Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
                DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
            },
            Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ},
        };
        fn mask(path: &Path, profile: &Profile) -> Option<u32> {
            let name = crate::isolated_worker::wide(path.as_os_str());
            let mut acl = ptr::null_mut();
            let mut descriptor = ptr::null_mut();
            assert_eq!(
                unsafe {
                    GetNamedSecurityInfoW(
                        name.as_ptr(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        &mut acl,
                        ptr::null_mut(),
                        &mut descriptor,
                    )
                },
                0
            );
            let _descriptor = crate::isolated_worker::LocalMemory(descriptor);
            let mut size: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe {
                    GetAclInformation(
                        acl,
                        ptr::from_mut(&mut size).cast(),
                        std::mem::size_of_val(&size) as u32,
                        AclSizeInformation,
                    )
                },
                0
            );
            let mut found = None;
            for index in 0..size.AceCount {
                let mut raw = ptr::null_mut();
                assert_ne!(unsafe { GetAce(acl, index, &mut raw) }, 0);
                let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
                if ace.Header.AceType == 0
                    && unsafe {
                        EqualSid(ptr::from_ref(&ace.SidStart).cast_mut().cast(), profile.sid)
                    } != 0
                {
                    assert!(found.is_none(), "duplicate private SID ACE");
                    found = Some(ace.Mask);
                }
            }
            found
        }
        let budget = Arc::new(CodeBudget::default());
        let mut files = CodeFiles::new(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &[
                ("node.wasm", b"not executable"),
                ("node.cwasm", b"not executable"),
            ],
            &budget,
        )
        .unwrap();
        let root = files.cache.path().to_owned();
        let paths = [
            root.clone(),
            root.join("node.wasm"),
            root.join("node.cwasm"),
        ];
        let mut profiles = (0..3).map(|_| Profile::new().unwrap()).collect::<Vec<_>>();
        let changes = profiles
            .iter()
            .map(|profile| (profile.sid, true))
            .collect::<Vec<_>>();
        files
            .cache
            .change_inherited_code_access_batch(2, &changes)
            .unwrap();
        for path in &paths {
            for profile in &profiles {
                assert_eq!(
                    mask(path, profile),
                    Some(FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)
                );
            }
        }
        assert!(
            files
                .cache
                .change_inherited_code_access_batch(
                    2,
                    &[(profiles[0].sid, false), (profiles[0].sid, true)]
                )
                .is_err()
        );
        files
            .cache
            .change_inherited_code_access_batch(2, &[(profiles[0].sid, false)])
            .unwrap();
        for path in &paths {
            assert!(mask(path, &profiles[0]).is_none());
            for profile in &profiles[1..] {
                assert!(mask(path, profile).is_some());
            }
        }
        let changes = profiles
            .iter()
            .map(|profile| (profile.sid, false))
            .collect::<Vec<_>>();
        files
            .cache
            .change_inherited_code_access_batch(2, &changes)
            .unwrap();
        for path in &paths {
            for profile in &profiles {
                assert!(mask(path, profile).is_none());
            }
        }
        files.acl_batch = Some(Batch::new());
        files.inherited_acl = true;
        let lease = files.lease(&profiles[0]).unwrap();
        for path in &paths {
            assert!(mask(path, &profiles[0]).is_some());
        }
        drop(lease);
        *files.healthy.lock().unwrap() = false;
        assert_eq!(
            files.lease(&profiles[0]).err().unwrap().code,
            "worker-asset"
        );
        for path in &paths {
            assert!(
                mask(path, &profiles[0]).is_none(),
                "unhealthy source granted code access"
            );
        }
        for profile in &mut profiles {
            profile.delete().unwrap();
        }
        drop(files);
        assert!(!root.exists());
        assert_eq!(budget.used.load(std::sync::atomic::Ordering::Acquire), 0);
    }
}
