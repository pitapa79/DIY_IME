//! Explicit Host-only experiment. No default eligibility or security-policy adoption.
use super::*;
use crate::{
    execution_control::{RunKey, RunPhase, StartReceipt, StartRequest},
    graph::Endpoint,
};
use std::sync::{Condvar, Mutex};

/// Startup scheduling only. Never transfers a process, SID, input or authority.
pub(super) struct StartupGate {
    state: Mutex<StartupState>,
    wake: Condvar,
    limit: usize,
    preparation: Option<Arc<StartupGate>>,
}
#[derive(Default)]
struct StartupState {
    busy: usize,
    recovery_waiters: std::collections::BTreeSet<usize>,
}
pub(super) struct StartupPermit(Arc<StartupGate>);
impl Default for StartupGate {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
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
        self.acquire_with_priority(cancel, None)
    }

    pub(super) fn acquire_recovery(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        stop: &CancellationToken,
        rank: usize,
    ) -> Result<StartupPermit, CoreError> {
        self.acquire_with_priority(cancel, Some((rank, stop)))
    }

    pub(super) fn try_acquire(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        stop: &CancellationToken,
    ) -> Result<StartupPermit, CoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| fail("worker-thread", "startup scheduling unavailable"))?;
        if cancel.is_cancelled() || stop.is_cancelled() {
            return Err(fail("worker-cancelled", "startup cancelled"));
        }
        if state.busy == self.limit || !state.recovery_waiters.is_empty() {
            return Err(fail(
                "retained-startup-gate-busy",
                "startup slots are occupied",
            ));
        }
        state.busy += 1;
        Ok(StartupPermit(self.clone()))
    }

    fn acquire_with_priority(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        priority: Option<(usize, &CancellationToken)>,
    ) -> Result<StartupPermit, CoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| fail("worker-thread", "startup scheduling unavailable"))?;
        // A coordinator has at most one startup attempt in this gate.
        if let Some((rank, _)) = priority
            && !state.recovery_waiters.insert(rank)
        {
            return Err(fail("worker-thread", "duplicate recovery startup rank"));
        }
        loop {
            let cancelled =
                cancel.is_cancelled() || priority.is_some_and(|(_, stop)| stop.is_cancelled());
            let available = state.busy < self.limit
                && priority.is_none_or(|(rank, _)| state.recovery_waiters.first() == Some(&rank));
            if cancelled || available {
                if let Some((rank, _)) = priority {
                    state.recovery_waiters.remove(&rank);
                    self.wake.notify_all();
                }
                if cancelled {
                    return Err(fail("worker-cancelled", "startup cancelled"));
                }
                state.busy += 1;
                return Ok(StartupPermit(self.clone()));
            }
            state = self
                .wake
                .wait_timeout(state, Duration::from_millis(5))
                .map_err(|_| fail("worker-thread", "startup scheduling unavailable"))?
                .0;
        }
    }
}
impl Drop for StartupPermit {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.busy = state.busy.checked_sub(1).expect("owned startup permit");
        self.0.wake.notify_all();
    }
}

pub(in crate::isolated_worker) type Stages = Vec<(String, WasmNode, Value)>;

#[derive(Clone)]
pub(super) struct RecoveryDemand {
    pub(super) key: RunKey,
    pub(super) cancel: CancellationToken,
    pending: Arc<Mutex<Vec<CancellationToken>>>,
}

impl RecoveryDemand {
    fn withdraw(&self) {
        let pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        self.cancel.cancel();
        for cancel in pending.iter() {
            cancel.cancel();
        }
    }

    pub(super) fn bind(&self, cancel: &CancellationToken) -> Result<RecoveryLease, CoreError> {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if self.cancel.is_cancelled() || pending.len() >= 20 {
            return Err(fail("worker-cancelled", "idle startup demand unavailable"));
        }
        pending.push(cancel.clone());
        Ok(RecoveryLease {
            demand: self.clone(),
            cancel: cancel.clone(),
        })
    }
}

pub(super) struct RecoveryLease {
    demand: RecoveryDemand,
    cancel: CancellationToken,
}

impl RecoveryLease {
    pub(super) fn adopt(&self) -> bool {
        let mut pending = self
            .demand
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if self.cancel.is_cancelled() {
            return false;
        }
        pending.retain(|token| !token.same_source(&self.cancel));
        true
    }
}

impl Drop for RecoveryLease {
    fn drop(&mut self) {
        self.demand
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|token| !token.same_source(&self.cancel));
    }
}

struct IdleRecovery {
    target: Endpoint,
    demand: Arc<Mutex<Option<RecoveryDemand>>>,
    order: Option<Vec<String>>,
}

pub(super) struct RecoveryStartup {
    pub(super) demand: Arc<Mutex<Option<RecoveryDemand>>>,
    pub(super) rank: Option<usize>,
}

pub(in crate::isolated_worker) struct Owners {
    owners: Vec<retained::Owner>,
    sizes: Vec<usize>,
    experimental: bool,
    idle_recovery: Option<IdleRecovery>,
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
            idle_recovery: None,
        }
    }

    pub(in crate::isolated_worker) fn idle_overlap_requested() -> bool {
        std::env::var_os("DIY_EXPERIMENT_RETAINED_IDLE_OVERLAP").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    }

    pub(in crate::isolated_worker) fn install_evaluation(
        host: ExecutionHost,
        controller: GuardedController,
        groups: Vec<Stages>,
        idle_chain: Option<(Endpoint, Vec<String>)>,
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
            idle_recovery: None,
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
        if startup_limit > 1
            && groups.len() > 1
            && let Some((target, order)) = idle_chain
        {
            let ordered = std::env::var_os("DIY_EXPERIMENT_RETAINED_IDLE_RECOVERY_ORDER")
                .as_deref()
                == Some(std::ffi::OsStr::new("1"));
            result.idle_recovery = Some(IdleRecovery {
                target,
                demand: Arc::new(Mutex::new(None)),
                order: ordered.then_some(order),
            });
        }
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
            let rank = result
                .idle_recovery
                .as_ref()
                .and_then(|idle| idle.order.as_ref())
                .map(|order| {
                    order
                        .iter()
                        .position(|id| id == &stages[0].0)
                        .ok_or_else(|| {
                            fail("retained-evaluation", "recovery startup order lost a stage")
                        })
                })
                .transpose()?;
            let owner = retained::Owner::install_evaluation(
                host.clone(),
                controller.clone(),
                stages,
                startup_gate.clone(),
                control_probe.clone(),
                diagnostic_origin,
                result.idle_recovery.as_ref().map(|idle| RecoveryStartup {
                    demand: idle.demand.clone(),
                    rank,
                }),
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
    pub(in crate::isolated_worker) fn requests_idle_recovery(
        &self,
        request: &StartRequest,
    ) -> bool {
        self.idle_recovery.as_ref().is_some_and(|idle| {
            request.input.is_some()
                && request.targets.as_slice() == std::slice::from_ref(&idle.target)
                && self.owners.iter().any(retained::Owner::idle_retired)
        })
    }

    pub(in crate::isolated_worker) fn idle_recovery_status(&self) -> Value {
        json!({
            "eligible": self.idle_recovery.is_some(),
            "idleRetiredOwners": self.owners.iter().filter(|owner| owner.idle_retired()).count(),
            "prioritizesQueuedStages": self.idle_recovery.as_ref().is_some_and(|idle| idle.order.is_some()),
        })
    }

    pub(in crate::isolated_worker) fn observe_start(&self, receipt: &StartReceipt, prepare: bool) {
        if receipt.replayed {
            return;
        }
        if let Some(idle) = &self.idle_recovery {
            let mut slot = idle.demand.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(previous) = slot.take() {
                previous.withdraw();
            }
            if prepare && receipt.state != RunPhase::Terminal {
                *slot = Some(RecoveryDemand {
                    key: receipt.key.clone(),
                    cancel: CancellationToken::default(),
                    pending: Arc::default(),
                });
            }
        }
    }

    pub(in crate::isolated_worker) fn withdraw_idle_recovery(
        &self,
        should_withdraw: impl FnOnce(&RunKey) -> bool,
    ) {
        if let Some(idle) = &self.idle_recovery {
            let mut slot = idle.demand.lock().unwrap_or_else(|p| p.into_inner());
            if slot.as_ref().is_some_and(|d| should_withdraw(&d.key))
                && let Some(demand) = slot.take()
            {
                demand.withdraw();
            }
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
        self.withdraw_idle_recovery(|_| true);
        // Cancel every group first, then each original owner joins and cleans up.
        for owner in &self.owners {
            owner.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, mpsc};

    #[test]
    fn idle_recovery_withdrawal_only_revokes_unadopted_startups() {
        let demand = Arc::new(Mutex::new(None));
        let owners = Owners {
            owners: vec![],
            sizes: vec![],
            experimental: true,
            idle_recovery: Some(IdleRecovery {
                target: Endpoint::new("last", "out"),
                demand: demand.clone(),
                order: None,
            }),
        };
        let key = RunKey {
            session_epoch: crate::execution_control::SessionEpoch::new("a".repeat(32)).unwrap(),
            plan_version: crate::execution_control::DecimalId::new(1).unwrap(),
            run_id: crate::execution_control::DecimalId::new(2).unwrap(),
            input_revision: Some(3),
            delivery_generation: crate::execution_control::DecimalId::new(4).unwrap(),
        };
        let mut receipt = StartReceipt {
            state: RunPhase::Accepted,
            replayed: false,
            key: key.clone(),
        };
        owners.observe_start(&receipt, true);
        let current = demand.lock().unwrap().clone().unwrap();
        let adopted = CancellationToken::default();
        let waiting = CancellationToken::default();
        let lease = current.bind(&adopted).unwrap();
        assert!(lease.adopt());
        let waiting_lease = current.bind(&waiting).unwrap();
        receipt.replayed = true;
        owners.observe_start(&receipt, false);
        owners.withdraw_idle_recovery(|candidate| candidate.input_revision == Some(2));
        assert!(!current.cancel.is_cancelled());
        owners.withdraw_idle_recovery(|candidate| candidate == &key);
        assert!(current.cancel.is_cancelled());
        assert!(waiting.is_cancelled());
        assert!(!waiting_lease.adopt());
        assert!(!adopted.is_cancelled());
        assert!(current.bind(&CancellationToken::default()).is_err());
    }

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
        assert_eq!(gate.state.lock().unwrap().busy, 2);
        assert_eq!(
            gate.preparation
                .as_ref()
                .unwrap()
                .state
                .lock()
                .unwrap()
                .busy,
            1
        );
        drop(prepare);
        assert_eq!(
            gate.state.lock().unwrap().busy,
            2,
            "preparation is scheduling, not a startup refund"
        );
        drop(gate.prepare(&CancellationToken::default()).unwrap());
        drop(second);
        let successor = gate.acquire(&CancellationToken::default()).unwrap();
        assert_eq!(gate.state.lock().unwrap().busy, 2);
        drop(first);
        drop(successor);
        assert_eq!(gate.state.lock().unwrap().busy, 0);
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
        assert_eq!(gate.state.lock().unwrap().busy, 1);
        drop(held);
        assert_eq!(gate.state.lock().unwrap().busy, 0);
        drop(gate.acquire(&CancellationToken::default()).unwrap());
    }

    #[test]
    fn recovery_startup_prioritizes_queued_stages_and_removes_cancelled_waiters() {
        let gate = Arc::new(StartupGate::bounded(2).unwrap());
        let first = gate.acquire(&CancellationToken::default()).unwrap();
        let second = gate.acquire(&CancellationToken::default()).unwrap();
        let (send, receive) = mpsc::channel();
        let mut waiters = Vec::new();
        for rank in [17, 3, 2] {
            let controller_stop = CancellationToken::default();
            let stop = controller_stop.clone();
            let gate = gate.clone();
            let worker_gate = gate.clone();
            let send = send.clone();
            let worker = std::thread::spawn(move || {
                let permit =
                    worker_gate.acquire_recovery(&CancellationToken::default(), &stop, rank);
                send.send((rank, permit.ok())).unwrap();
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !gate.state.lock().unwrap().recovery_waiters.contains(&rank) {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            waiters.push((controller_stop, worker));
        }
        waiters[2].0.cancel();
        let (cancelled, permit) = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(cancelled, 2);
        assert!(permit.is_none());
        drop(first);
        let (next, permit) = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(next, 3);
        drop(permit.unwrap());
        let (last, permit) = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(last, 17);
        drop(permit.unwrap());
        for (_, worker) in waiters {
            worker.join().unwrap();
        }
        assert!(gate.state.lock().unwrap().recovery_waiters.is_empty());
        assert_eq!(gate.state.lock().unwrap().busy, 1);
        drop(second);
    }

    #[test]
    fn nonblocking_startup_permit_preserves_capacity_and_cancellation() {
        let gate = Arc::new(StartupGate::bounded(2).unwrap());
        let cancel = CancellationToken::default();
        let stop = CancellationToken::default();
        let first = gate.acquire(&cancel).unwrap();
        let second = gate.try_acquire(&cancel, &stop).unwrap();
        assert_eq!(
            gate.try_acquire(&cancel, &stop).err().unwrap().code,
            "retained-startup-gate-busy"
        );
        drop(second);
        stop.cancel();
        assert_eq!(
            gate.try_acquire(&cancel, &stop).err().unwrap().code,
            "worker-cancelled"
        );
        assert_eq!(gate.state.lock().unwrap().busy, 1);
        drop(first);
        assert_eq!(gate.state.lock().unwrap().busy, 0);
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
            assert_eq!(gate.state.lock().unwrap().busy, 0);
        }
    }
}
