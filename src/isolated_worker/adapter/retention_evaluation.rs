//! Explicit Host-only experiment. No default eligibility or security-policy adoption.
use super::*;
use std::sync::{Condvar, Mutex};

/// Startup scheduling only. Never transfers a process, SID, input or authority.
#[derive(Default)]
pub(super) struct StartupGate {
    busy: Mutex<bool>,
    wake: Condvar,
}
pub(super) struct StartupPermit(Arc<StartupGate>);
impl StartupGate {
    pub(super) fn acquire(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<StartupPermit, CoreError> {
        let mut busy = self
            .busy
            .lock()
            .map_err(|_| fail("worker-thread", "startup scheduling unavailable"))?;
        loop {
            if cancel.is_cancelled() {
                return Err(fail("worker-cancelled", "startup cancelled"));
            }
            if !*busy {
                *busy = true;
                return Ok(StartupPermit(self.clone()));
            }
            busy = self
                .wake
                .wait_timeout(busy, Duration::from_millis(5))
                .map_err(|_| fail("worker-thread", "startup scheduling unavailable"))?
                .0;
        }
    }
}
impl Drop for StartupPermit {
    fn drop(&mut self) {
        *self.0.busy.lock().unwrap_or_else(|p| p.into_inner()) = false;
        self.0.wake.notify_all();
    }
}

pub(in crate::isolated_worker) type Stages = Vec<(String, WasmNode, Value)>;

pub(in crate::isolated_worker) struct Owners {
    owners: Vec<retained::Owner>,
    sizes: Vec<usize>,
    experimental: bool,
}

impl Owners {
    pub(in crate::isolated_worker) fn evaluation_group_size() -> Result<Option<usize>, CoreError> {
        match std::env::var_os("DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION").as_deref() {
            None => Ok(None),
            Some(s) if s == std::ffi::OsStr::new("0") => Ok(None),
            Some(s) if s == std::ffi::OsStr::new("separate") => Ok(Some(1)),
            Some(s) if s == std::ffi::OsStr::new("grouped") => Ok(Some(4)),
            _ => Err(fail(
                "retained-evaluation",
                "invalid explicit evaluation mode",
            )),
        }
    }

    pub(in crate::isolated_worker) fn fixed(owner: retained::Owner) -> Self {
        Self {
            owners: vec![owner],
            sizes: vec![4],
            experimental: false,
        }
    }

    pub(in crate::isolated_worker) fn install_evaluation(
        host: ExecutionHost,
        controller: GuardedController,
        groups: Vec<Stages>,
    ) -> Result<Self, CoreError> {
        if groups.is_empty()
            || groups.len() > 20
            || host.policy().max_physical_jobs < groups.len() as u64 + 1
            || groups.iter().map(Vec::len).sum::<usize>() > 20
        {
            return Err(fail(
                "retained-evaluation",
                "insufficient bounded lifetime/input capacity",
            ));
        }
        let mut result = Self {
            owners: Vec::new(),
            sizes: Vec::new(),
            experimental: true,
        };
        // Avoid launching twenty full-module compilers simultaneously on a
        // small-memory machine. Warm execution remains independent per group.
        let startup_gate = Arc::new(StartupGate::default());
        // Twenty idle coordinators must not fill the Core's existing sixteen
        // request permits with simultaneous liveness probes. Keep every check
        // fresh, with one outstanding probe for this evaluation cohort.
        let control_probe = Arc::new(Mutex::new(()));
        for stages in groups {
            result.sizes.push(stages.len());
            let owner = retained::Owner::install_evaluation(
                host.clone(),
                controller.clone(),
                stages,
                startup_gate.clone(),
                control_probe.clone(),
            )?
            .ok_or_else(|| {
                fail(
                    "retained-evaluation",
                    "unsupported retained evaluation policy",
                )
            })?;
            result.owners.push(owner);
        }
        Ok(result)
    }

    pub(in crate::isolated_worker) fn physical_jobs(&self) -> u64 {
        self.owners.iter().map(|o| o.physical_jobs()).sum()
    }
    pub(in crate::isolated_worker) fn start(&self) {
        for owner in &self.owners {
            owner.start();
        }
    }
    pub(in crate::isolated_worker) fn witness_terminal(&self, c: &GuardedController) {
        for owner in &self.owners {
            owner.witness_terminal(c);
        }
    }
    pub(in crate::isolated_worker) fn ready(&self) -> bool {
        self.owners.iter().all(|o| o.ready())
    }
    pub(in crate::isolated_worker) fn process_id(&self) -> Option<u64> {
        if self.owners.len() == 1 {
            self.owners[0].process_id()
        } else {
            None
        }
    }
    pub(in crate::isolated_worker) fn generation(&self) -> u64 {
        self.owners.iter().map(|o| o.generation()).sum()
    }
    pub(in crate::isolated_worker) fn decoded_replies(&self) -> u64 {
        self.owners.iter().map(|o| o.decoded_replies()).sum()
    }
    pub(in crate::isolated_worker) fn active_input(&self) -> bool {
        self.owners.iter().any(|o| o.active_input())
    }
    pub(in crate::isolated_worker) fn group_sizes(&self) -> &[usize] {
        &self.sizes
    }
    pub(in crate::isolated_worker) fn experimental(&self) -> bool {
        self.experimental
    }
    pub(in crate::isolated_worker) fn process_ids(&self) -> Vec<u64> {
        self.owners.iter().filter_map(|o| o.process_id()).collect()
    }
    pub(in crate::isolated_worker) fn startup_failures(&self) -> u64 {
        self.owners.iter().map(|o| o.startup_failures()).sum()
    }
    pub(in crate::isolated_worker) fn compiled_generations(&self) -> u64 {
        self.owners.iter().map(|o| o.compiled_generations()).sum()
    }
    pub(in crate::isolated_worker) fn call_samples(&self) -> Vec<Value> {
        self.owners
            .iter()
            .filter_map(|o| o.call_samples())
            .collect()
    }
}

impl Drop for Owners {
    fn drop(&mut self) {
        // Cancel every group first, then each original owner joins and cleans up.
        for owner in &self.owners {
            owner.close();
        }
    }
}

impl WasmNode {
    pub(in crate::isolated_worker) fn retention_evaluation_eligible(&self) -> bool {
        // A classification, package name or guest self-report grants nothing.
        // These exact reviewed official bytes are already pinned by distribution.
        self.artifact.0.identity.rsplit(':').next()
            == Some("105c0ceb2854dba27a747eae5faa6e393bb99a88ce5ab3048a468fa9add88134")
            && matches!(
                self.definition.kind.as_str(),
                "experimental.packet-passthrough"
                    | "experimental.text-trim"
                    | "experimental.text-case"
                    | "demo.identity"
                    | "text.romaji-to-hiragana"
                    | "experimental.text-concat"
            )
            && self.definition.version == 1
            && self.network.is_none()
            && self.dictionary.is_none()
            && self.prepared_dictionary.get().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, mpsc};

    #[test]
    fn startup_waiter_cancels_without_consuming_or_releasing_another_permit() {
        let gate = Arc::new(StartupGate::default());
        let held = gate.acquire(&CancellationToken::default()).unwrap();
        let cancel = CancellationToken::default();
        let token = cancel.clone();
        let waiting = gate.clone();
        let (send, receive) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            send.send(()).unwrap();
            waiting.acquire(&token).err().unwrap().code
        });
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.cancel();
        assert_eq!(thread.join().unwrap(), "worker-cancelled");
        assert!(*gate.busy.lock().unwrap());
        drop(held);
        assert!(!*gate.busy.lock().unwrap());
        drop(gate.acquire(&CancellationToken::default()).unwrap());
    }

    #[test]
    fn startup_concurrency_is_bounded_and_all_waiters_make_progress() {
        let gate = Arc::new(StartupGate::default());
        let active = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let (gate, active, completed) = (gate.clone(), active.clone(), completed.clone());
            threads.push(std::thread::spawn(move || {
                for _ in 0..4 {
                    let permit = gate.acquire(&CancellationToken::default()).unwrap();
                    assert_eq!(active.fetch_add(1, Ordering::AcqRel), 0);
                    std::thread::yield_now();
                    assert_eq!(active.fetch_sub(1, Ordering::AcqRel), 1);
                    completed.fetch_add(1, Ordering::AcqRel);
                    drop(permit);
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(completed.load(Ordering::Acquire), 32);
        assert!(!*gate.busy.lock().unwrap());
    }
}
