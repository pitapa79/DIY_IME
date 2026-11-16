//! Explicit Host-only experiment. No default eligibility or security-policy adoption.
use super::*;
use std::sync::{Condvar, Mutex};

/// Startup scheduling only. Never transfers a process, SID, input or authority.
pub(super) struct StartupGate {
    busy: Mutex<usize>,
    wake: Condvar,
    limit: usize,
    preparation: Option<Arc<StartupGate>>,
}
pub(super) struct StartupPermit(Arc<StartupGate>);
impl Default for StartupGate {
    fn default() -> Self {
        Self {
            busy: Mutex::new(0),
            wake: Condvar::new(),
            limit: 1,
            preparation: None,
        }
    }
}
impl StartupGate {
    fn bounded(limit: usize) -> Result<Self, CoreError> {
        if !matches!(limit, 1 | 2 | 4) {
            return Err(fail("retained-startup", "unsupported startup concurrency"));
        }
        Ok(Self {
            limit,
            preparation: (limit > 1).then(|| Arc::new(Self::default())),
            ..Self::default()
        })
    }
    pub(super) fn requires_compiled(&self) -> bool {
        self.limit > 1
    }
    pub(super) fn prepare(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<StartupPermit>, CoreError> {
        self.preparation
            .as_ref()
            .map(|gate| gate.acquire(cancel))
            .transpose()
    }
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
            if *busy < self.limit {
                *busy += 1;
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
        let mut busy = self.0.busy.lock().unwrap_or_else(|p| p.into_inner());
        *busy = busy.checked_sub(1).expect("owned startup permit");
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
    fn startup_limit_value(value: Option<&std::ffi::OsStr>) -> Result<usize, CoreError> {
        match value {
            None => Ok(1),
            Some(value) if value == "1" => Ok(1),
            Some(value) if value == "2" => Ok(2),
            Some(value) if value == "4" => Ok(4),
            _ => Err(fail(
                "retained-startup",
                "invalid explicit startup concurrency",
            )),
        }
    }

    fn validate_startup_mode(
        limit: usize,
        group_size: Option<usize>,
        compiled: bool,
        singleton_groups: bool,
    ) -> Result<(), CoreError> {
        if limit > 1 && (group_size != Some(1) || !compiled || !singleton_groups) {
            return Err(fail(
                "retained-startup",
                "parallel startup requires compiled separate evaluation",
            ));
        }
        Ok(())
    }

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
        let startup_limit = Self::startup_limit_value(
            std::env::var_os("DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM").as_deref(),
        )?;
        Self::validate_startup_mode(
            startup_limit,
            Self::evaluation_group_size()?,
            std::env::var_os("DIY_EXPERIMENT_RETAINED_COMPILED_JIT").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            groups.iter().all(|group| group.len() == 1),
        )?;
        let startup_gate = Arc::new(StartupGate::bounded(startup_limit)?);
        // Twenty idle coordinators must not fill the Core's existing sixteen
        // request permits with simultaneous liveness probes. Keep every check
        // fresh, with one outstanding probe for this evaluation cohort.
        let control_probe = Arc::new(Mutex::new(()));
        let diagnostic_origin = (std::env::var_os("DIY_EXPERIMENT_RETAINED_TIMING").as_deref()
            == Some(std::ffi::OsStr::new("1")))
        .then(std::time::Instant::now);
        for stages in groups {
            result.sizes.push(stages.len());
            let owner = retained::Owner::install_evaluation(
                host.clone(),
                controller.clone(),
                stages,
                startup_gate.clone(),
                control_probe.clone(),
                diagnostic_origin,
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
    fn parallel_startup_is_explicit_finite_and_compiled_separate_only() {
        use std::ffi::OsStr;
        assert_eq!(Owners::startup_limit_value(None).unwrap(), 1);
        for limit in [1, 2, 4] {
            assert_eq!(
                Owners::startup_limit_value(Some(OsStr::new(&limit.to_string()))).unwrap(),
                limit
            );
            assert_eq!(
                StartupGate::bounded(limit).unwrap().requires_compiled(),
                limit > 1
            );
            Owners::validate_startup_mode(limit, Some(1), true, true).unwrap();
        }
        for invalid in ["", "0", "3", "8", "02", " 2", "all"] {
            assert!(Owners::startup_limit_value(Some(OsStr::new(invalid))).is_err());
        }
        for limit in [0, 3, 8, usize::MAX] {
            assert!(StartupGate::bounded(limit).is_err());
        }
        for limit in [2, 4] {
            assert!(Owners::validate_startup_mode(limit, Some(4), true, true).is_err());
            assert!(Owners::validate_startup_mode(limit, None, true, true).is_err());
            assert!(Owners::validate_startup_mode(limit, Some(1), false, true).is_err());
            assert!(Owners::validate_startup_mode(limit, Some(1), true, false).is_err());
        }
        Owners::validate_startup_mode(1, Some(4), false, false).unwrap();
    }

    #[test]
    fn parallel_startup_owns_distinct_permits_and_cancellable_serial_preparation() {
        let gate = Arc::new(StartupGate::bounded(2).unwrap());
        let first = gate.acquire(&CancellationToken::default()).unwrap();
        let second = gate.acquire(&CancellationToken::default()).unwrap();
        let prepare = gate.prepare(&CancellationToken::default()).unwrap();
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert!(gate.acquire(&cancel).is_err());
        assert!(gate.prepare(&cancel).is_err());
        assert_eq!(*gate.busy.lock().unwrap(), 2);
        assert_eq!(*gate.preparation.as_ref().unwrap().busy.lock().unwrap(), 1);
        drop(prepare);
        assert_eq!(
            *gate.busy.lock().unwrap(),
            2,
            "preparation is scheduling, not a startup refund"
        );
        drop(gate.prepare(&CancellationToken::default()).unwrap());
        drop(second);
        let successor = gate.acquire(&CancellationToken::default()).unwrap();
        assert_eq!(*gate.busy.lock().unwrap(), 2);
        drop(first);
        drop(successor);
        assert_eq!(*gate.busy.lock().unwrap(), 0);
    }

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
        assert_eq!(*gate.busy.lock().unwrap(), 1);
        drop(held);
        assert_eq!(*gate.busy.lock().unwrap(), 0);
        drop(gate.acquire(&CancellationToken::default()).unwrap());
    }

    #[test]
    fn startup_concurrency_is_bounded_and_all_waiters_make_progress() {
        for limit in [1, 2, 4] {
            let gate = Arc::new(StartupGate::bounded(limit).unwrap());
            let active = Arc::new(AtomicUsize::new(0));
            let completed = Arc::new(AtomicUsize::new(0));
            let mut threads = Vec::new();
            for _ in 0..8 {
                let (gate, active, completed) = (gate.clone(), active.clone(), completed.clone());
                threads.push(std::thread::spawn(move || {
                    for _ in 0..4 {
                        let permit = gate.acquire(&CancellationToken::default()).unwrap();
                        assert!(active.fetch_add(1, Ordering::AcqRel) < limit);
                        std::thread::yield_now();
                        assert!(active.fetch_sub(1, Ordering::AcqRel) <= limit);
                        completed.fetch_add(1, Ordering::AcqRel);
                        drop(permit);
                    }
                }));
            }
            for thread in threads {
                thread.join().unwrap();
            }
            assert_eq!(completed.load(Ordering::Acquire), 32);
            assert_eq!(*gate.busy.lock().unwrap(), 0);
        }
    }
}
