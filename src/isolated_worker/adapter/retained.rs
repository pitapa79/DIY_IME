//! Session-owned fixed-official coordinator. Nodes hold weak links only.
use super::retention_evaluation::{StartupDemand, StartupKind, StartupLease, StartupScope};
use super::*;
use crate::execution_control::{ExecutionHost, RunDelivery, RunKey, RunPhase, RunSnapshot};
use crate::isolated_worker::{
    DriverPolicy, DriverStop, RetainedBinding, RetainedClient, RetainedReservation,
    RetainedWorkerRequest,
};
use std::sync::{Mutex, Weak, mpsc};

type Arguments = (OwnedInvocation, CompletionPermit, CancellationToken);
const STARTUP_RETRY_WINDOW: Duration = Duration::from_millis(500);
struct Route {
    owner: Weak<Shared>,
}
#[derive(Clone, Default)]
pub(super) struct NodeLink(Arc<Mutex<std::collections::BTreeMap<String, Route>>>);
struct Command {
    stage: usize,
    node: WasmNode,
    invocation: OwnedInvocation,
    completion: CompletionPermit,
    cancel: CancellationToken,
    done: mpsc::SyncSender<()>,
    startup_retry_until: Option<std::time::Instant>,
    restarted_after_owner_end: bool,
}
impl Command {
    fn fail(self, error: CoreError) {
        if !self.cancel.is_cancelled() || !self.completion.finish_cancelled() {
            self.completion.complete(Err(error));
        }
        let _ = self.done.send(());
    }
    fn legacy(self, shared: &Shared) {
        // The parallel comparison must not conceal startup/preparation failure
        // by executing the input through the ordinary S0 fallback. No input is
        // retried here; the original owner has already settled where required.
        if shared
            .startup_gate
            .as_ref()
            .is_some_and(|gate| gate.requires_compiled())
        {
            self.fail(fail(
                "retained-startup",
                "parallel retained route unavailable",
            ));
            return;
        }
        let completion = self.completion.clone();
        if let Err(error) = self
            .node
            .start_legacy(self.invocation, self.completion, self.cancel)
        {
            completion.complete(Err(error));
        }
        let _ = self.done.send(());
    }
}
struct Shared {
    qualifications: Option<Vec<retention_review::Qualification>>,
    send: mpsc::SyncSender<Command>,
    stop: CancellationToken,
    input: Mutex<Option<ActiveInput>>,
    #[cfg(test)]
    end_ready_owner_before_input: std::sync::atomic::AtomicBool,
    lifetime: Mutex<Option<CancellationToken>>,
    physical: AtomicU64,
    ready: std::sync::atomic::AtomicBool,
    process_id: Arc<AtomicU64>,
    generation: AtomicU64,
    start: std::sync::atomic::AtomicBool,
    eager_startup: bool,
    ids: Vec<String>,
    replies: AtomicU64,
    startup_gate: Option<Arc<retention_evaluation::StartupGate>>,
    startup_scope: Option<StartupScope>,
    idle_retired: std::sync::atomic::AtomicBool,
    initial_pending: std::sync::atomic::AtomicBool,
    control_probe: Option<Arc<Mutex<()>>>,
    startup_failures: AtomicU64,
    compiled_generations: AtomicU64,
    samples: Option<crate::isolated_worker::retained_metrics::Samples>,
}
struct ActiveInput {
    key: RunKey,
    cancel: CancellationToken,
    completed: bool,
}
fn successful(snapshot: &RunSnapshot) -> bool {
    snapshot.current
        && snapshot.state == RunPhase::Terminal
        && snapshot.result.as_ref().is_some_and(|r| {
            matches!(
                r.delivery,
                RunDelivery::Complete | RunDelivery::Recovered | RunDelivery::NoOutput
            )
        })
}
impl Shared {
    fn record_phase(&self, phase: &'static str) {
        self.record_run_phase(phase, None);
    }
    fn record_run_phase(&self, phase: &'static str, run_id: Option<DecimalId>) {
        if let Some(samples) = &self.samples {
            let pid = self.process_id.load(Ordering::Acquire);
            samples.phase(phase, (pid != 0).then_some(pid), run_id);
        }
    }
    fn cancel_input(&self, key: &RunKey) {
        if let Some(input) = &*self.input.lock().unwrap_or_else(|p| p.into_inner())
            && &input.key == key
            && !input.completed
        {
            self.record_run_phase("input-cancel-requested", Some(key.run_id));
            input.cancel.cancel();
        }
    }
    fn completed(&self, key: &RunKey) -> bool {
        self.input
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|input| &input.key == key && input.completed)
    }
    fn close(&self) {
        self.stop.cancel();
        if let Some(cancel) = &*self.lifetime.lock().unwrap_or_else(|p| p.into_inner()) {
            cancel.cancel();
        }
        if let Some(input) = &*self.input.lock().unwrap_or_else(|p| p.into_inner()) {
            input.cancel.cancel();
        }
    }
}
impl NodeLink {
    pub(super) fn start(
        &self,
        node: &WasmNode,
        arguments: Arguments,
    ) -> Result<(), Box<Arguments>> {
        let Some(shared) = self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(arguments.0.context().node_id)
            .and_then(|route| route.owner.upgrade())
        else {
            return Err(Box::new(arguments));
        };
        let Some(stage) = shared
            .ids
            .iter()
            .position(|id| id == arguments.0.context().node_id)
        else {
            return Err(Box::new(arguments));
        };
        let (invocation, completion, cancel) = arguments;
        if let Some(qualifications) = &shared.qualifications
            && let Err(error) = qualifications[stage].check(
                node,
                &shared.ids[stage],
                invocation.config(),
                Some(invocation.key()),
            )
        {
            shared.close();
            completion.complete(Err(error));
            return Ok(());
        }
        shared.record_run_phase("invocation-offered", Some(invocation.key().run_id));
        let node = node.clone();
        let token = cancel.clone();
        let key = invocation.key().clone();
        let (done, receive) = mpsc::sync_channel(1);
        // Core already owns this callback's bounded physical/input reservation.
        // This thread forwards cancellation while the coordinator blocks in IO.
        let command = Command {
            stage,
            node,
            invocation,
            completion,
            cancel,
            done,
            startup_retry_until: None,
            restarted_after_owner_end: false,
        };
        // A failed thread creation must return ownership to the caller.
        let owned = Arc::new(Mutex::new(Some(command)));
        let worker_owned = owned.clone();
        let worker = std::thread::Builder::new()
            .name("diyime-retained-callback".into())
            .spawn(move || {
                let command = worker_owned
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .unwrap();
                if let Err(error) = shared.send.send(command) {
                    error
                        .0
                        .fail(fail("retained-closed", "retained Session is closed"));
                    return;
                }
                loop {
                    match receive.recv_timeout(Duration::from_millis(5)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if token.is_cancelled() {
                                shared.cancel_input(&key);
                            }
                        }
                    }
                }
            });
        match worker {
            Ok(_) => Ok(()),
            Err(_) => {
                let command = owned
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .unwrap();
                Err(Box::new((
                    command.invocation,
                    command.completion,
                    command.cancel,
                )))
            }
        }
    }
}

pub(in crate::isolated_worker) struct Owner {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Owner {
    /// Called only with actual prepared nodes from the exact static-reviewed plan.
    pub(in crate::isolated_worker) fn install(
        host: ExecutionHost,
        controller: GuardedController,
        stages: Vec<(String, WasmNode, Value)>,
    ) -> Result<Option<Self>, CoreError> {
        Self::install_stages(host, controller, stages, None, None, None, None)
    }

    pub(super) fn install_evaluation(
        host: ExecutionHost,
        controller: GuardedController,
        stages: Vec<(String, WasmNode, Value)>,
        startup_gate: Arc<retention_evaluation::StartupGate>,
        control_probe: Arc<Mutex<()>>,
        diagnostic_origin: Option<std::time::Instant>,
        startup_scope: Option<StartupScope>,
    ) -> Result<Option<Self>, CoreError> {
        let grouped = retention_evaluation::Owners::evaluation_group_size()? == Some(4);
        if stages
            .iter()
            .any(|(_, node, config)| !node.retention_evaluation_eligible(config, grouped))
        {
            return Err(fail("retained-evaluation", "unreviewed implementation"));
        }
        Self::install_stages(
            host,
            controller,
            stages,
            Some(startup_gate),
            Some(control_probe),
            diagnostic_origin,
            startup_scope,
        )
    }

    fn install_stages(
        host: ExecutionHost,
        controller: GuardedController,
        stages: Vec<(String, WasmNode, Value)>,
        startup_gate: Option<Arc<retention_evaluation::StartupGate>>,
        control_probe: Option<Arc<Mutex<()>>>,
        diagnostic_origin: Option<std::time::Instant>,
        startup_scope: Option<StartupScope>,
    ) -> Result<Option<Self>, CoreError> {
        let evaluation = startup_gate.is_some();
        let mut ids = Vec::new();
        let mut nodes = Vec::new();
        let mut configs = Vec::new();
        for (id, node, config) in stages {
            ids.push(id);
            nodes.push(node);
            configs.push(config);
        }
        if (!evaluation && nodes.len() != 4) || !(1..=4).contains(&nodes.len()) {
            return Err(fail("retained-plan", "four prepared stages required"));
        }
        let policy = &nodes[0].artifact.0.policy;
        // Unsupported policies keep the legacy path; never add fuel/memory caps.
        if host.policy().max_physical_jobs < 2
            || host.policy().max_queued_jobs < 1
            || !policy.supports_range_pipeline()
            || nodes.iter().any(|node| {
                let image = &retained_artifact(node).0;
                node.network.is_some()
                    || image.policy != *policy
                    || !Arc::ptr_eq(&image.runtime_files, &nodes[0].artifact.0.runtime_files)
            })
        {
            return Ok(None);
        }
        let qualifications = if evaluation {
            let grouped = retention_evaluation::Owners::evaluation_group_size()? == Some(4);
            Some(
                nodes
                    .iter()
                    .zip(&ids)
                    .zip(&configs)
                    .map(|((node, id), config)| {
                        retention_review::Qualification::review(
                            node,
                            id,
                            config,
                            &controller,
                            grouped,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
        } else {
            None
        };
        let (send, receive) = mpsc::sync_channel(1);
        let samples = if evaluation
            && std::env::var_os("DIY_EXPERIMENT_RETAINED_TIMING").as_deref()
                == Some(std::ffi::OsStr::new("1"))
        {
            Some(crate::isolated_worker::retained_metrics::Samples::new(
                &host,
                diagnostic_origin.ok_or_else(|| fail("retained-timing", "missing cohort clock"))?,
            )?)
        } else {
            None
        };
        let defer_startup = evaluation
            && nodes.len() == 1
            && retention_evaluation::Owners::evaluation_group_size()? == Some(1)
            && retention_evaluation::Owners::deferred_initial_requested();
        let initial_pending = startup_scope
            .as_ref()
            .is_some_and(|scope| defer_startup && scope.initial && scope.rank > 0);
        let shared = Arc::new(Shared {
            qualifications,
            send,
            stop: CancellationToken::default(),
            input: Mutex::new(None),
            #[cfg(test)]
            end_ready_owner_before_input: std::sync::atomic::AtomicBool::new(false),
            lifetime: Mutex::new(None),
            physical: AtomicU64::new(0),
            ready: std::sync::atomic::AtomicBool::new(false),
            process_id: Arc::new(AtomicU64::new(0)),
            generation: AtomicU64::new(0),
            start: std::sync::atomic::AtomicBool::new(false),
            eager_startup: !defer_startup,
            ids: ids.clone(),
            replies: AtomicU64::new(0),
            startup_gate,
            startup_scope,
            idle_retired: std::sync::atomic::AtomicBool::new(false),
            initial_pending: std::sync::atomic::AtomicBool::new(initial_pending),
            control_probe,
            startup_failures: AtomicU64::new(0),
            compiled_generations: AtomicU64::new(0),
            samples,
        });
        let worker_shared = shared.clone();
        let links: Vec<_> = ids
            .iter()
            .cloned()
            .zip(nodes.iter().map(|n| n.retained.clone()))
            .collect();
        let thread = std::thread::Builder::new()
            .name("diyime-retained-session".into())
            .spawn(move || {
                while !worker_shared.start.load(Ordering::Acquire)
                    && !worker_shared.stop.is_cancelled()
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if worker_shared.stop.is_cancelled() {
                    return;
                }
                coordinate(
                    host,
                    controller,
                    nodes,
                    ids,
                    configs,
                    worker_shared,
                    receive,
                )
            })
            .map_err(|_| fail("worker-thread", "retained Session thread unavailable"))?;
        for (id, link) in links {
            link.0.lock().unwrap_or_else(|p| p.into_inner()).insert(
                id,
                Route {
                    owner: Arc::downgrade(&shared),
                },
            );
        }
        Ok(Some(Self {
            shared,
            thread: Some(thread),
        }))
    }
    pub(super) fn close(&self) {
        self.shared.close();
    }
    pub(super) fn initial_pending(&self) -> bool {
        self.shared.initial_pending.load(Ordering::Acquire)
    }
    pub(super) fn idle_retired(&self) -> bool {
        self.shared.idle_retired.load(Ordering::Acquire)
    }
    pub(super) fn startup_failures(&self) -> u64 {
        self.shared.startup_failures.load(Ordering::Acquire)
    }
    pub(super) fn compiled_generations(&self) -> u64 {
        self.shared.compiled_generations.load(Ordering::Acquire)
    }
    pub(super) fn call_samples(&self) -> Option<Value> {
        self.shared.samples.as_ref().map(|samples| {
            let mut value = serde_json::Map::new();
            value.insert("nodeIds".into(), json!(self.shared.ids));
            value.insert("calls".into(), samples.snapshot());
            Value::Object(value)
        })
    }
    pub(in crate::isolated_worker) fn physical_jobs(&self) -> u64 {
        self.shared.physical.load(Ordering::Acquire)
    }
    /// Open only after the preceding owner has actually ended and cleaned up.
    pub(in crate::isolated_worker) fn start(&self) {
        self.shared.start.store(true, Ordering::Release);
    }
    /// Capture the actual completed Core state before Host invalidation can
    /// withdraw the snapshot. A guest reply is never this witness.
    pub(in crate::isolated_worker) fn witness_terminal(&self, controller: &GuardedController) {
        let key = self
            .shared
            .input
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|input| input.key.clone());
        if let Some(key) = key
            && controller
                .get_run(&key)
                .is_ok_and(|snapshot| successful(&snapshot))
            && let Some(input) = self
                .shared
                .input
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
            && input.key == key
        {
            input.completed = true;
        }
    }
    pub(in crate::isolated_worker) fn ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire)
    }
    pub(in crate::isolated_worker) fn process_id(&self) -> Option<u64> {
        let id = self.shared.process_id.load(Ordering::Acquire);
        (id != 0).then_some(id)
    }
    pub(in crate::isolated_worker) fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
    pub(in crate::isolated_worker) fn decoded_replies(&self) -> u64 {
        self.shared.replies.load(Ordering::Acquire)
    }
    pub(in crate::isolated_worker) fn active_input(&self) -> bool {
        self.shared
            .input
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|input| !input.completed)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Native {
    recovery: Option<StartupLease>,
    client: Option<RetainedClient>,
    cancel: CancellationToken,
    thread: Option<std::thread::JoinHandle<Result<Option<ExitReceipt>, CoreError>>>,
    _pin: RetainedReservation,
    shared: Arc<Shared>,
    startup_permit: Option<retention_evaluation::StartupPermit>,
    started: std::time::Instant,
}
fn limits(
    budget: Duration,
    resources: &std::collections::BTreeMap<String, Arc<[u8]>>,
) -> ResourceRangeLimits {
    // v1 still uses the original payload. Pin and charge the exact immutable
    // snapshot allocation; this is not the A prepared-artifact role profile.
    ResourceRangeLimits {
        max_resources: resources.len().max(1) as u32,
        max_snapshot_bytes: resources
            .values()
            .map(|bytes| bytes.len())
            .sum::<usize>()
            .max(1),
        max_read_bytes: 1,
        max_total_read_bytes: 1,
        max_reads: 1,
        budget,
    }
}
fn scope(
    node: &WasmNode,
    id: &str,
    config: &Value,
    key: &RunKey,
    cancel: CancellationToken,
    budget: Duration,
) -> Result<ResourceRangeSession, CoreError> {
    if let Some(prepared) = node.prepared_dictionary.get() {
        return prepared.scope(node, id, key, cancel, budget);
    }
    let resources = node
        .dictionary
        .as_ref()
        .map(|plans| plans.snapshots(config))
        .transpose()?
        .unwrap_or_default();
    let limits = limits(budget, &resources);
    if resources.is_empty() {
        return ResourceRangeSession::open_empty(
            node.gate.clone(),
            node.grant_target(&key.session_epoch, key.plan_version, id),
            key.clone(),
            cancel,
            limits,
        );
    }
    ResourceRangeSession::open(
        node.gate.clone(),
        node.grant_target(&key.session_epoch, key.plan_version, id),
        key.clone(),
        cancel,
        resources,
        limits,
    )
}

fn retained_artifact(node: &WasmNode) -> &WasmArtifact {
    node.prepared_dictionary
        .get()
        .map_or(&node.artifact, |prepared| &prepared.consumer)
}

fn control_state(
    shared: &Shared,
    controller: &GuardedController,
) -> Result<crate::execution_control::SessionControlState, CoreError> {
    let _probe = shared
        .control_probe
        .as_ref()
        .map(|probe| {
            probe
                .lock()
                .map_err(|_| fail("worker-thread", "retained control probe unavailable"))
        })
        .transpose()?;
    controller.control_state()
}

fn current_run(shared: &Shared, controller: &GuardedController, key: &RunKey) -> bool {
    let _probe = shared
        .control_probe
        .as_ref()
        .map(|probe| probe.lock().unwrap_or_else(|p| p.into_inner()));
    !shared.stop.is_cancelled()
        && controller
            .get_run(key)
            .is_ok_and(|snapshot| snapshot.current && snapshot.state != RunPhase::Terminal)
}

impl StartupDemand {
    fn current(&self, shared: &Shared, controller: &GuardedController) -> bool {
        !self.cancel.is_cancelled()
            && current_run(shared, controller, &self.key)
            && !self.cancel.is_cancelled()
    }
    fn validate(&self, shared: &Shared, controller: &GuardedController) -> Result<(), CoreError> {
        if self.current(shared, controller) {
            Ok(())
        } else {
            Err(fail(
                "worker-cancelled",
                "scoped startup demand is no longer current",
            ))
        }
    }
}

enum StartupAttempt<'a> {
    Ordinary,
    Recovery(&'a StartupDemand),
    Retry {
        key: &'a RunKey,
        cancel: &'a CancellationToken,
        deadline: std::time::Instant,
    },
}

impl StartupAttempt<'_> {
    fn check_retry_current(
        &self,
        shared: &Shared,
        controller: &GuardedController,
    ) -> Result<(), CoreError> {
        if let Self::Retry { key, cancel, .. } = self
            && (cancel.is_cancelled()
                || !current_run(shared, controller, key)
                || cancel.is_cancelled())
        {
            return Err(fail(
                "worker-cancelled",
                "startup retry is no longer current",
            ));
        }
        Ok(())
    }
}

impl Native {
    fn start(
        host: &ExecutionHost,
        controller: &GuardedController,
        nodes: &[WasmNode],
        ids: &[String],
        configs: &[Value],
        shared: &Arc<Shared>,
        attempt: StartupAttempt<'_>,
    ) -> Result<Self, CoreError> {
        let recovery = match attempt {
            StartupAttempt::Recovery(demand) => Some(demand),
            _ => None,
        };
        if let Some(qualifications) = &shared.qualifications {
            for (((qualification, node), id), config) in
                qualifications.iter().zip(nodes).zip(ids).zip(configs)
            {
                qualification.check(node, id, config, None)?;
            }
        }
        if let Some(samples) = &shared.samples {
            samples.begin_attempt();
        }
        shared.record_phase("attempt-started");
        let startup_permit = shared
            .startup_gate
            .as_ref()
            .map(|gate| match attempt {
                StartupAttempt::Recovery(demand) => {
                    let rank = shared
                        .startup_scope
                        .as_ref()
                        .filter(|scope| demand.kind == StartupKind::Initial || scope.ordered_idle);
                    match rank {
                        Some(scope) => gate.acquire_scoped(
                            &demand.cancel,
                            &shared.stop,
                            demand.kind,
                            scope.rank,
                        ),
                        None => gate.acquire(&demand.cancel),
                    }
                }
                StartupAttempt::Ordinary => gate.acquire(&shared.stop),
                StartupAttempt::Retry {
                    key,
                    cancel,
                    deadline,
                } => gate
                    .acquire_retry(cancel, &shared.stop, deadline)
                    .inspect_err(|error| {
                        if error.code == "retained-startup-gate-busy" {
                            shared
                                .record_run_phase("startup-retry-permit-expired", Some(key.run_id));
                        }
                    }),
            })
            .transpose()?;
        shared.record_phase("startup-permit-acquired");
        attempt.check_retry_current(shared, controller)?;
        if let StartupAttempt::Retry { deadline, .. } = attempt
            && std::time::Instant::now() >= deadline
        {
            return Err(fail(
                "retained-startup-gate-busy",
                "startup retry window elapsed",
            ));
        }
        if let Some(demand) = recovery {
            demand.validate(shared, controller)?;
            let phase = match demand.kind {
                StartupKind::Idle => "idle-recovery-demand-validated",
                StartupKind::Initial => "initial-startup-demand-validated",
            };
            shared.record_run_phase(phase, Some(demand.key.run_id));
        }
        let state = control_state(shared, controller).inspect_err(|error| {
            shared.record_phase(if error.code == "busy" {
                "startup-control-busy"
            } else {
                "startup-control-failed"
            });
        })?;
        let key = RunKey {
            session_epoch: controller.session_epoch().clone(),
            plan_version: state
                .active_plan_version
                .ok_or_else(|| fail("retained-plan", "active plan required"))?,
            run_id: DecimalId::new(1)?,
            input_revision: None,
            delivery_generation: state.delivery_generation,
        };
        let cancel = CancellationToken::default();
        let recovery_lease = recovery.map(|demand| demand.bind(&cancel)).transpose()?;
        *shared.lifetime.lock().unwrap_or_else(|p| p.into_inner()) = Some(cancel.clone());
        if shared.stop.is_cancelled() {
            cancel.cancel();
        }
        let mut modules: Vec<WasmArtifact> = Vec::new();
        let bindings = nodes
            .iter()
            .zip(ids)
            .zip(configs)
            .map(|((node, id), config)| {
                let artifact = retained_artifact(node);
                let slot = modules
                    .iter()
                    .position(|module| module.0.module == artifact.0.module)
                    .unwrap_or_else(|| {
                        let slot = modules.len();
                        modules.push(artifact.clone());
                        slot
                    });
                Ok(RetainedBinding {
                    module_slot: slot as u32,
                    scope: scope(
                        node,
                        id,
                        config,
                        &key,
                        cancel.clone(),
                        Duration::from_secs(600),
                    )?,
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()
            .inspect_err(|_| shared.record_phase("startup-scope-failed"))?;
        // Reserve BEFORE any compiler process starts. Preparation and the
        // replacement process never overlap and use this same charged slot.
        let pin = RetainedReservation::reserve(host, bindings, &cancel).map_err(|error| {
            if error.code == "busy" {
                shared.record_phase("startup-reservation-busy");
                fail(
                    "retained-startup-reservation-busy",
                    "retained Worker reservation is full",
                )
            } else {
                shared.record_phase("startup-reservation-failed");
                error
            }
        })?;
        shared.record_phase("reservation-acquired");
        if let StartupAttempt::Retry { key, .. } = attempt {
            shared.record_run_phase("startup-reservation-retry-admitted", Some(key.run_id));
        }
        let compiled_route = shared.startup_gate.is_some()
            && nodes.len() == 1
            && retention_evaluation::Owners::evaluation_group_size()? == Some(1)
            && std::env::var_os("DIY_EXPERIMENT_RETAINED_COMPILED_JIT").as_deref()
                == Some(std::ffi::OsStr::new("1"));
        let cached = if compiled_route
            && shared
                .startup_gate
                .as_ref()
                .is_some_and(|gate| gate.requires_compiled())
            && std::env::var_os("DIY_EXPERIMENT_RETAINED_READY_CODE_NO_PREPARE_WAIT").as_deref()
                == Some(std::ffi::OsStr::new("1"))
            && std::env::var_os("DIY_EXPERIMENT_COMPILED_JIT").as_deref()
                == Some(std::ffi::OsStr::new("1"))
        {
            ready_retained_code(&modules[0].0.jit_compiled, &cancel)
        } else {
            None
        };
        let preparation_permit = if cached.is_some() {
            shared.record_phase("preparation-permit-skipped-ready-code");
            None
        } else {
            shared.record_phase("preparation-permit-waiting");
            let permit = shared
                .startup_gate
                .as_ref()
                .map(|gate| gate.prepare(&cancel))
                .transpose()?
                .flatten();
            shared.record_phase("preparation-permit-acquired");
            permit
        };
        if let Some(demand) = recovery {
            demand.validate(shared, controller)?;
        }
        attempt.check_retry_current(shared, controller)?;
        shared.record_phase("code-preparation-started");
        let compiled = if compiled_route {
            // Source selection is this captured Node's own factory. Preparing
            // code runs no input/Instance and settles the original compiler
            // before granting a new retained process access to its output.
            let image = if let Some(image) = cached {
                if cancel.is_cancelled() {
                    return Err(fail(
                        "worker-cancelled",
                        "cached startup cancelled before launch",
                    ));
                }
                shared.record_phase("code-preparation-cache-hit");
                image
            } else {
                modules[0].prepared_jit_image_owned(&cancel, Some(pin.clone()), |phase| {
                    shared.record_phase(phase);
                })
            };
            match image {
                WorkerImage::PreparedCompiled(files, len, digest) => {
                    files.check_healthy()?;
                    Some(RetainedCompiledCode {
                        files,
                        len,
                        digest,
                        source: modules[0].0.identity.rsplit(':').next().unwrap().to_owned(),
                    })
                }
                _ => None,
            }
        } else {
            None
        };
        let used_compiled = compiled.is_some();
        shared.record_phase("code-preparation-finished");
        // The preparation process has settled, its captured output is owned,
        // and the same physical pin remains charged through replacement exit.
        // Release scheduling only; never return execution ownership here.
        drop(preparation_permit);
        if let Some(demand) = recovery {
            demand.validate(shared, controller)?;
        }
        attempt.check_retry_current(shared, controller)?;
        if !used_compiled
            && shared
                .startup_gate
                .as_ref()
                .is_some_and(|gate| gate.requires_compiled())
        {
            return Err(fail(
                "retained-startup",
                "parallel startup requires captured compiled code",
            ));
        }
        let reply_limit = host.policy().max_result_bytes_per_run.min(8 * 1024 * 1024);
        let stages = (0..nodes.len()).collect();
        let timing_flag = if used_compiled {
            "DIY_EXPERIMENT_RETAINED_NATIVE_STARTUP_TIMING"
        } else {
            "DIY_EXPERIMENT_RETAINED_NATIVE_RAW_STARTUP_TIMING"
        };
        let timed = nodes.len() == 1
            && shared.samples.is_some()
            && std::env::var_os(timing_flag).as_deref() == Some(std::ffi::OsStr::new("1"));
        let startup_observer = timed.then(|| {
            let process_id = shared.process_id.clone();
            Box::new(
                move |timing: crate::isolated_worker::retained_startup::Timing| {
                    use std::io::Write;
                    let (label, module_phase) = if used_compiled {
                        ("retained-native-startup", "restore_us")
                    } else {
                        ("retained-native-raw-startup", "compile_us")
                    };
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "{label} pid={} verify_us={} load_us={} read_us={} {module_phase}={}",
                        process_id.load(Ordering::Acquire),
                        timing.dll_verify_us,
                        timing.dll_load_us,
                        timing.module_read_us,
                        timing.module_restore_us,
                    );
                },
            ) as crate::isolated_worker::retained_startup::Observer
        });
        let (client, request) = RetainedClient::channel_observed(
            host.clone(),
            pin.clone(),
            stages,
            DriverPolicy {
                idle_timeout: Duration::from_secs(60),
                input_timeout: Duration::from_secs(120),
                max_calls: 4096,
                max_input_bytes: 32 * 1024 * 1024,
                max_reply_bytes: reply_limit,
            },
            startup_observer,
        )?;
        let artifact = nodes[0].artifact.clone();
        shared.ready.store(false, Ordering::Release);
        shared.process_id.store(0, Ordering::Release);
        let owner_shared = shared.clone();
        let cleanup_pin = pin.clone();
        let startup_demand = recovery.cloned();
        let retry_cancel = match attempt {
            StartupAttempt::Retry { cancel, .. } => Some(cancel.clone()),
            _ => None,
        };
        let control = RunControl {
            cancel: cancel.clone(),
            walltime: Some(Duration::from_secs(600)),
            termination_grace: artifact.0.policy.termination_grace,
        };
        let thread = std::thread::Builder::new()
            .name("diyime-retained-native-owner".into())
            .spawn(move || {
                if startup_demand.is_some_and(|demand| demand.cancel.is_cancelled())
                    || retry_cancel.is_some_and(|cancel| cancel.is_cancelled())
                {
                    return Err(fail("worker-cancelled", "startup request was withdrawn"));
                }
                artifact.retained_lifetime(
                    &modules,
                    RetainedStartup { compiled, timed },
                    request,
                    &control,
                    cleanup_pin,
                    owner_shared,
                )
            })
            .map_err(|_| fail("worker-thread", "retained native owner unavailable"))?;
        shared.physical.fetch_add(1, Ordering::AcqRel);
        shared.generation.fetch_add(1, Ordering::AcqRel);
        if used_compiled {
            shared.compiled_generations.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Self {
            recovery: recovery_lease,
            client: Some(client),
            cancel,
            thread: Some(thread),
            _pin: pin,
            shared: shared.clone(),
            startup_permit,
            started: std::time::Instant::now(),
        })
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.shared.ready.store(false, Ordering::Release);
        self.shared.record_phase("retirement-requested");
        self.cancel.cancel();
        drop(self.client.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.shared.record_phase("owner-joined");
        self.shared.physical.fetch_sub(1, Ordering::AcqRel);
        self.shared.process_id.store(0, Ordering::Release);
    }
}

/// Only constructed above from this source's settled compile-only process.
/// Arc retains locked code and its finite budget through original cleanup.
struct RetainedCompiledCode {
    files: Arc<CodeFiles>,
    len: usize,
    digest: String,
    source: String,
}

struct RetainedStartup {
    compiled: Option<RetainedCompiledCode>,
    timed: bool,
}

fn ready_retained_code(
    cache: &Mutex<CompiledState>,
    cancel: &CancellationToken,
) -> Option<WorkerImage> {
    if cancel.is_cancelled() {
        return None;
    }
    // A cache read cannot launch a compiler or publish a new asset.
    // Pending, unavailable and busy owners keep the serial preparation path.
    let state = cache.try_lock().ok()?;
    let CompiledState::Ready(asset) = &*state else {
        return None;
    };
    Some(WorkerImage::PreparedCompiled(
        asset.files.clone(),
        asset.len,
        asset.digest.clone(),
    ))
}

impl WasmArtifact {
    fn retained_lifetime(
        &self,
        modules: &[WasmArtifact],
        startup: RetainedStartup,
        request: RetainedWorkerRequest,
        control: &RunControl,
        _cleanup_pin: RetainedReservation,
        shared: Arc<Shared>,
    ) -> Result<Option<ExitReceipt>, CoreError> {
        let RetainedStartup { compiled, timed } = startup;
        shared.record_phase("profile-create-started");
        let prepared = if compiled.is_some()
            && shared
                .qualifications
                .as_ref()
                .is_some_and(|nodes| nodes.len() == 1)
            && std::env::var_os("DIY_EXPERIMENT_RETAINED_PREPARED_PROFILE").as_deref()
                == Some(std::ffi::OsStr::new("1"))
        {
            // The pool only contains unused profiles. Taking one transfers
            // ownership; this lifetime's existing disposal path deletes it.
            let profile = self.0.profile_pool.as_ref().and_then(|pool| pool.take());
            shared.record_phase(if profile.is_some() {
                "profile-pool-consumed"
            } else {
                "profile-pool-miss"
            });
            profile
        } else {
            None
        };
        let mut profile = PreparationProfile {
            profile: prepared
                .map(Ok)
                .unwrap_or_else(Profile::new)
                .map_err(|_| fail("worker-isolation", "AppContainer unavailable"))?,
            strict: compiled.is_some(),
        };
        shared.record_phase("profile-create-finished");
        shared.record_phase("code-access-started");
        let runtime = self.runtime_code_files()?;
        let module_files = if let Some(code) = &compiled {
            code.files.check_healthy()?;
            vec![code.files.clone()]
        } else {
            modules
                .iter()
                .map(WasmArtifact::module_code_files)
                .collect::<Result<Vec<_>, CoreError>>()?
        };
        let mut runtime_lease = runtime.lease(&profile)?;
        let mut module_leases = Vec::new();
        let receipt = (|| {
            use crate::isolated_worker::retained_startup::{
                RAW_TIMED_FLAG, TIMED_FLAG, VERIFIED_FLAG, VERIFIED_TIMED_FLAG,
            };

            let dll = runtime.cache.path().join("wasmtime.dll");
            let verified_runtime = if compiled.is_some()
                && std::env::var_os("DIY_EXPERIMENT_RETAINED_VERIFIED_RUNTIME_FILE").as_deref()
                    == Some(std::ffi::OsStr::new("1"))
            {
                Some(
                    runtime
                        .verified_runtime
                        .get_or_init(|| super::super::runtime::VerifiedRuntimeFile::open(&dll))
                        .as_ref()
                        .map_err(|_| {
                            fail("worker-identity", "retained runtime file proof unavailable")
                        })?,
                )
            } else {
                None
            };
            let mode = match (compiled.is_some(), timed, verified_runtime.is_some()) {
                (true, true, true) => VERIFIED_TIMED_FLAG,
                (true, false, true) => VERIFIED_FLAG,
                (true, true, false) => TIMED_FLAG,
                (true, false, false) => "--retained-compiled-resources-v1",
                (false, true, _) => RAW_TIMED_FLAG,
                (false, false, _) => "--retained-resources-v1",
            };
            let mut args: Vec<OsString> = vec![
                mode.into(),
                dll.into_os_string(),
                "4096".into(),
                self.0.policy.fuel_per_call.unwrap().to_string().into(),
                self.0.policy.wasm_memory_bytes.unwrap().to_string().into(),
            ];
            for (artifact, files) in modules.iter().zip(&module_files) {
                module_leases.push(files.lease(&profile)?);
                if let Some(code) = &compiled {
                    args.push(files.cache.path().join("node.cwasm").into_os_string());
                    args.push(code.len.to_string().into());
                    args.push(code.digest.clone().into());
                    args.push(code.source.clone().into());
                } else {
                    args.push(files.cache.path().join("node.wasm").into_os_string());
                    args.push(artifact.0.module.len().to_string().into());
                }
            }
            shared.record_phase("code-access-finished");
            shared.record_phase("job-create-started");
            let job = Job::with_memory_limit(self.0.policy.process_memory_bytes)
                .map_err(|_| fail("worker-isolation", "Worker Job unavailable"))?;
            shared.record_phase("job-create-finished");
            shared.record_phase("native-launch-started");
            crate::isolated_worker::pipe::execute_retained_observed(
                &profile,
                &job,
                &runtime.cache,
                &runtime.cache.path().join("worker.exe"),
                &args,
                request,
                control,
                verified_runtime,
                Some(crate::isolated_worker::pipe::RetainedObservation {
                    activity: self.0.activity.clone(),
                    process_id: shared.process_id.clone(),
                    launch_phase: shared.samples.as_ref().map(|_| {
                        let shared = Arc::clone(&shared);
                        let observer: crate::isolated_worker::pipe::LaunchPhaseObserver =
                            Box::new(move |phase| shared.record_phase(phase));
                        observer
                    }),
                }),
            )
            .map(settle)
            .map_err(|_| fail("worker-start", "retained Worker could not start"))
        })();
        shared.record_phase(if receipt.as_ref().is_ok_and(|exit| exit.is_some()) {
            "original-process-io-ended"
        } else {
            "original-exit-unconfirmed"
        });
        // Keep the original pin on this same thread until actual exit/IO and ACL
        // cleanup. A cleanup retry cannot mint a successor or refund ownership.
        shared.record_phase("acl-release-started");
        let mut observed_module_retry = false;
        for mut lease in module_leases {
            while lease.release().is_err() {
                if !observed_module_retry {
                    shared.record_phase("module-acl-release-retry");
                    observed_module_retry = true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        shared.record_phase("runtime-acl-release-started");
        let mut observed_runtime_retry = false;
        while runtime_lease.release().is_err() {
            if !observed_runtime_retry {
                shared.record_phase("runtime-acl-release-retry");
                observed_runtime_retry = true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        shared.record_phase("runtime-acl-release-finished");
        drop(runtime_lease);
        shared.record_phase("acl-release-finished");
        shared.record_phase("profile-delete-started");
        while profile.delete().is_err() {
            std::thread::sleep(Duration::from_millis(100));
        }
        shared.record_phase("profile-delete-finished");
        receipt
    }
}

fn retire_native(native: &mut Option<Native>, shared: &Shared, allow_idle_recovery: bool) {
    let retired = native.take();
    let idle = allow_idle_recovery
        && shared
            .startup_scope
            .as_ref()
            .is_some_and(|scope| scope.idle)
        && retired.as_ref().is_some_and(|owner| {
            owner.client.as_ref().unwrap().stop_cause() == Some(DriverStop::Idle)
        });
    drop(retired);
    if idle {
        shared.idle_retired.store(true, Ordering::Release);
    }
}

fn claim_pre_delivery_restart(restarted: &mut bool, was_ready: bool, owner_ended: bool) -> bool {
    if *restarted || !was_ready || !owner_ended {
        return false;
    }
    *restarted = true;
    true
}

fn claim_and_trace_pre_delivery_restart(
    shared: &Shared,
    command: &mut Command,
    was_ready: bool,
    owner_ended: bool,
) -> bool {
    if !claim_pre_delivery_restart(
        &mut command.restarted_after_owner_end,
        was_ready,
        owner_ended,
    ) {
        return false;
    }
    command
        .startup_retry_until
        .get_or_insert_with(|| std::time::Instant::now() + STARTUP_RETRY_WINDOW);
    shared.record_run_phase(
        "invocation-requeued-after-owner-end",
        Some(command.invocation.key().run_id),
    );
    true
}

fn coordinate(
    host: ExecutionHost,
    controller: GuardedController,
    nodes: Vec<WasmNode>,
    ids: Vec<String>,
    configs: Vec<Value>,
    shared: Arc<Shared>,
    receive: mpsc::Receiver<Command>,
) {
    let stage_count = nodes.len();
    // A deferred coordinator starts through the same qualified invocation
    // path as an ordinary replacement, after Core has reserved its callback.
    let mut native = shared
        .eager_startup
        .then(|| {
            Native::start(
                &host,
                &controller,
                &nodes,
                &ids,
                &configs,
                &shared,
                StartupAttempt::Ordinary,
            )
            .inspect_err(|_| {
                shared.startup_failures.fetch_add(1, Ordering::AcqRel);
            })
            .ok()
        })
        .flatten();
    let mut pending = None;
    let mut provisional: Option<StartupDemand> = None;
    let mut last_demand = None;
    while !shared.stop.is_cancelled() && control_state(&shared, &controller).is_ok() {
        if provisional
            .as_ref()
            .is_some_and(|demand| !demand.current(&shared, &controller))
        {
            drop(native.take());
            provisional = None;
        }
        let ready = native
            .as_mut()
            .map(|owner| owner.client.as_mut().unwrap().poll_ready());
        match ready {
            Some(Ok(value)) => {
                shared.ready.store(value, Ordering::Release);
                if value {
                    if native.as_ref().unwrap().startup_permit.is_some() {
                        shared.record_phase("readiness-observed");
                    }
                    native.as_mut().unwrap().startup_permit.take();
                }
            }
            Some(Err(_)) => {
                // Readiness can fail after a running driver's lease ends.
                // Only failures before this owner became ready are startup failures.
                if !shared.ready.load(Ordering::Acquire) {
                    shared.startup_failures.fetch_add(1, Ordering::AcqRel);
                }
                retire_native(&mut native, &shared, provisional.is_none());
            }
            None => (),
        }
        if native.as_ref().is_some_and(|owner| {
            owner.startup_permit.is_some() && owner.started.elapsed() >= Duration::from_secs(10)
        }) {
            // Use the same readiness bound as an on-demand retained start.
            // Native drops its original owners before returning this permit.
            shared.startup_failures.fetch_add(1, Ordering::AcqRel);
            drop(native.take());
        }
        if native
            .as_ref()
            .is_some_and(|n| n.client.as_ref().unwrap().stop_cause().is_some())
        {
            retire_native(&mut native, &shared, provisional.is_none());
        }
        if native.is_none() {
            provisional = None;
            let demand = shared.startup_scope.as_ref().and_then(|startup| {
                startup
                    .demand
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone()
            });
            if let Some(demand) = demand
                && last_demand != Some(demand.key.run_id)
                && match demand.kind {
                    StartupKind::Idle => shared.idle_retired.load(Ordering::Acquire),
                    StartupKind::Initial => shared.initial_pending.load(Ordering::Acquire),
                }
            {
                last_demand = Some(demand.key.run_id);
                if demand.current(&shared, &controller) {
                    shared.idle_retired.store(false, Ordering::Release);
                    native = Native::start(
                        &host,
                        &controller,
                        &nodes,
                        &ids,
                        &configs,
                        &shared,
                        StartupAttempt::Recovery(&demand),
                    )
                    .ok();
                    if native.is_some() {
                        provisional = Some(demand);
                    }
                }
            }
        }
        let mut command = match pending
            .take()
            .map(Ok)
            .unwrap_or_else(|| receive.recv_timeout(Duration::from_millis(5)))
        {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if command.startup_retry_until.is_none() && !command.restarted_after_owner_end {
            shared.record_run_phase("invocation-dequeued", Some(command.invocation.key().run_id));
        }
        if command.cancel.is_cancelled() {
            command.fail(fail("worker-cancelled", "invocation cancelled"));
            continue;
        }
        if command.restarted_after_owner_end
            && !current_run(&shared, &controller, command.invocation.key())
        {
            command.fail(fail(
                "worker-cancelled",
                "replacement input is no longer current",
            ));
            continue;
        }
        if command.stage != 0 {
            command.legacy(&shared);
            continue;
        }
        shared.initial_pending.store(false, Ordering::Release);
        if let Some(demand) = &provisional
            && (&demand.key != command.invocation.key() || !demand.current(&shared, &controller))
        {
            drop(native.take());
            provisional = None;
        }
        if native.is_none() {
            shared.idle_retired.store(false, Ordering::Release);
            let attempt = if let Some(deadline) = command.startup_retry_until {
                if !current_run(&shared, &controller, command.invocation.key()) {
                    command.fail(fail(
                        "worker-cancelled",
                        "startup retry is no longer current",
                    ));
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    command.legacy(&shared);
                    continue;
                }
                StartupAttempt::Retry {
                    key: command.invocation.key(),
                    cancel: &command.cancel,
                    deadline,
                }
            } else {
                StartupAttempt::Ordinary
            };
            match Native::start(&host, &controller, &nodes, &ids, &configs, &shared, attempt) {
                Ok(owner) => native = Some(owner),
                Err(error)
                    if matches!(
                        error.code.as_str(),
                        "retained-startup-reservation-busy" | "retained-startup-gate-busy"
                    ) && shared
                        .startup_gate
                        .as_ref()
                        .is_some_and(|gate| gate.requires_compiled()) =>
                {
                    // Native::start has released its permit. Keep the exact
                    // Core callback charged while older work drains. No input
                    // was delivered, so this only retries Worker preparation.
                    command
                        .startup_retry_until
                        .get_or_insert_with(|| std::time::Instant::now() + STARTUP_RETRY_WINDOW);
                    pending = Some(command);
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error)
                    if command.restarted_after_owner_end && error.code == "worker-cancelled" =>
                {
                    command.fail(error);
                    continue;
                }
                Err(_) => {
                    command.legacy(&shared);
                    continue;
                }
            }
        }
        let Some(owner) = native.as_mut() else {
            command.legacy(&shared);
            continue;
        };
        let was_ready = shared.ready.load(Ordering::Acquire);
        let requires_compiled = shared
            .startup_gate
            .as_ref()
            .is_some_and(|gate| gate.requires_compiled());
        if owner
            .client
            .as_mut()
            .unwrap()
            .wait_ready(Duration::from_secs(10))
            .is_err()
        {
            let owner_ended = owner._pin.is_closed();
            drop(native.take());
            if provisional.take().is_some() {
                // No input reached this Worker. Use the normal start for the
                // already admitted invocation if speculative readiness failed.
                pending = Some(command);
            } else if requires_compiled
                && claim_and_trace_pre_delivery_restart(
                    &shared,
                    &mut command,
                    was_ready,
                    owner_ended,
                )
            {
                // The previously ready owner ended after the coordinator's
                // readiness poll. Join it before preparing a new owner for
                // this same command, which has not reached begin_input.
                pending = Some(command);
            } else {
                command.legacy(&shared);
            }
            continue;
        }
        if owner.startup_permit.is_some() {
            shared.record_phase("readiness-observed");
        }
        drop(owner.startup_permit.take());
        shared.ready.store(true, Ordering::Release);
        if command.cancel.is_cancelled()
            || (command.restarted_after_owner_end
                && !current_run(&shared, &controller, command.invocation.key()))
        {
            if command.restarted_after_owner_end {
                drop(native.take());
            }
            command.fail(fail(
                "worker-cancelled",
                "invocation cancelled before delivery",
            ));
            continue;
        }
        if provisional
            .as_ref()
            .is_some_and(|demand| !demand.current(&shared, &controller))
        {
            drop(native.take());
            provisional = None;
            pending = Some(command);
            continue;
        }
        // Only the actual authorized invocation adopts a provisional Worker.
        if let Some(lease) = owner.recovery.take() {
            if !lease.adopt() {
                drop(native.take());
                provisional = None;
                pending = Some(command);
                continue;
            }
            shared.record_run_phase(
                "startup-owner-adopted",
                Some(command.invocation.key().run_id),
            );
        }
        provisional = None;
        let key = command.invocation.key().clone();
        let input_cancel = CancellationToken::default();
        #[cfg(test)]
        if was_ready
            && shared
                .end_ready_owner_before_input
                .swap(false, Ordering::AcqRel)
        {
            owner.cancel.cancel();
        }
        *shared.input.lock().unwrap_or_else(|p| p.into_inner()) = Some(ActiveInput {
            key: key.clone(),
            cancel: input_cancel.clone(),
            completed: false,
        });
        let mut begin_error = None;
        let mut input = owner
            .client
            .as_mut()
            .unwrap()
            .begin_input(key.clone(), input_cancel.clone())
            .map_err(|error| {
                begin_error = Some(error.code);
            })
            .ok();
        if input.is_none() {
            drop(input);
            let closed_ready_owner = requires_compiled
                && (was_ready || command.restarted_after_owner_end)
                && begin_error.as_deref() == Some("retained-scope")
                && owner._pin.is_closed();
            drop(native.take());
            *shared.input.lock().unwrap_or_else(|p| p.into_inner()) = None;
            if closed_ready_owner {
                // The pin can close after readiness succeeds but before
                // begin_input acquires a charge. No request has been sent.
                if claim_and_trace_pre_delivery_restart(&shared, &mut command, was_ready, true) {
                    pending = Some(command);
                } else {
                    command.legacy(&shared);
                }
            } else if matches!(
                begin_error.as_deref(),
                Some("retained-call-limit" | "retained-scope")
            ) {
                pending = Some(command);
            } else {
                command.legacy(&shared);
            }
            continue;
        }
        let mut next = Some(command);
        let mut completed = 0usize;
        let mut failed = None;
        let mut fallback = None;
        let mut finished = false;
        loop {
            if let Some(command) = next.take() {
                if command.stage != completed || command.invocation.key() != &key {
                    failed = Some((
                        command,
                        fail("retained-stage-order", "unexpected Core stage"),
                    ));
                    break;
                }
                if input.as_ref().unwrap().remaining_budget()
                    < command.node.artifact.0.policy.walltime.unwrap()
                {
                    fallback = Some(command);
                    break;
                }
                let work = (|| {
                    let invocation = &command.invocation;
                    let request_phase = invocation.trace_host_phase(HostTracePhase::Request);
                    if command.cancel.is_cancelled() {
                        input_cancel.cancel();
                        return Err(fail(
                            "worker-cancelled",
                            "invocation cancelled before delivery",
                        ));
                    }
                    if command.invocation.config() != &configs[completed] {
                        return Err(fail(
                            "retained-plan",
                            "prepared stage configuration changed",
                        ));
                    }
                    command.node.validate_config(command.invocation.config())?;
                    if let Some(qualifications) = &shared.qualifications {
                        qualifications[completed].check(
                            &command.node,
                            &ids[completed],
                            command.invocation.config(),
                            Some(&key),
                        )?;
                    }
                    let mut scope = scope(
                        &command.node,
                        &ids[completed],
                        command.invocation.config(),
                        &key,
                        input_cancel.clone(),
                        command.node.artifact.0.policy.walltime.unwrap(),
                    )?;
                    scope.follow_cancellation(command.cancel.clone())?;
                    let request = if let Some(prepared) = command.node.prepared_dictionary.get() {
                        prepared.request(invocation, &command.node.definition, &mut scope)?
                    } else {
                        let mut request = wire::execution(invocation, &command.node.definition)?;
                        if let Some(plans) = &command.node.dictionary {
                            request["operation"]["resources"] =
                                plans.payload(command.invocation.config())?;
                        }
                        request
                    };
                    let bytes = serde_json::to_vec(&request)
                        .map_err(|_| fail("worker-request", "request serialization failed"))?;
                    drop(request_phase);
                    let measured = shared.samples.is_some();
                    shared.record_run_phase("invocation-call-started", Some(key.run_id));
                    let call_started = measured.then(std::time::Instant::now);
                    let reply = input
                        .as_mut()
                        .unwrap()
                        .call_measured(bytes, scope, measured);
                    let call_elapsed_ns = call_started
                        .and_then(|start| u64::try_from(start.elapsed().as_nanos()).ok());
                    shared.record_run_phase(
                        if reply.is_ok() {
                            "invocation-call-finished"
                        } else {
                            "invocation-call-failed"
                        },
                        Some(key.run_id),
                    );
                    if let Some(samples) = &shared.samples {
                        samples.push(crate::isolated_worker::retained_metrics::Sample {
                            run_id: key.run_id,
                            plan_version: key.plan_version,
                            input_revision: key.input_revision,
                            delivery_generation: key.delivery_generation,
                            stage: completed,
                            generation: shared.generation.load(Ordering::Acquire),
                            process_id: match shared.process_id.load(Ordering::Acquire) {
                                0 => None,
                                pid => Some(pid),
                            },
                            parent_call_ns: call_elapsed_ns,
                            outcome: if reply.is_ok() { "success" } else { "error" },
                            native: reply
                                .as_ref()
                                .ok()
                                .and_then(|(_, m)| m.as_ref())
                                .and_then(|m| m.value),
                        });
                    }
                    let (data, _measurement) = reply?;
                    shared.replies.fetch_add(1, Ordering::AcqRel);
                    let outputs = wire::retained_outputs(data, &command.node.definition)?;
                    input.as_mut().unwrap().publish_stage(|| {
                        if command.cancel.is_cancelled() {
                            return Err(fail("worker-cancelled", "invocation cancelled"));
                        }
                        if !command.completion.complete(Ok(outputs)) {
                            return Err(fail("worker-cancelled", "Core completion withdrawn"));
                        }
                        Ok(())
                    })
                })();
                if let Err(error) = work {
                    if matches!(
                        error.code.as_str(),
                        "retained-input-limit" | "retained-reply-limit" | "busy"
                    ) {
                        fallback = Some(command);
                    } else {
                        failed = Some((command, error));
                    }
                    break;
                }
                completed += 1;
                let _ = command.done.send(());
            }
            if completed == stage_count {
                finished = input.take().unwrap().finish(|| Ok(())).is_ok();
                break;
            }
            if shared.stop.is_cancelled() || input_cancel.is_cancelled() {
                break;
            }
            let snapshot = controller.get_run(&key);
            if shared.completed(&key) || snapshot.as_ref().is_ok_and(successful) {
                let suffix: Vec<_> = (completed..stage_count).collect();
                finished = input
                    .take()
                    .unwrap()
                    .finish_unexecuted_suffix(&suffix, || Ok(()))
                    .is_ok();
                break;
            }
            if !snapshot
                .is_ok_and(|snapshot| snapshot.current && snapshot.state != RunPhase::Terminal)
            {
                break;
            }
            match receive.recv_timeout(Duration::from_millis(5)) {
                Ok(command) if command.invocation.key() == &key => next = Some(command),
                Ok(command) => {
                    // An independent live run may use the legacy path. A replaced
                    // input must wait for this original Worker to end first.
                    if controller
                        .get_run(&key)
                        .is_ok_and(|s| s.current && s.state != RunPhase::Terminal)
                    {
                        command.legacy(&shared);
                    } else {
                        pending = Some(command);
                        if shared.completed(&key) {
                            let suffix: Vec<_> = (completed..stage_count).collect();
                            finished = input
                                .take()
                                .unwrap()
                                .finish_unexecuted_suffix(&suffix, || Ok(()))
                                .is_ok();
                        }
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // Non-consuming exits still own the input guard here.
        // Successful finish consumed it above; Rust enforces the exclusive client.
        drop(input);
        if !finished {
            drop(native.take());
        }
        *shared.input.lock().unwrap_or_else(|p| p.into_inner()) = None;
        if let Some((command, error)) = failed {
            command.fail(error);
        }
        if let Some(command) = fallback {
            command.legacy(&shared);
        }
    }
    drop(native);
    if let Some(command) = pending {
        command.fail(fail("retained-closed", "retained Session is closed"));
    }
    while let Ok(command) = receive.try_recv() {
        command.fail(fail("retained-closed", "retained Session is closed"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_control::{ExecutionService, ResourcePolicy, ResourceUsage, StartRequest};
    use crate::{
        builtins::register_system_nodes,
        graph::{ExecutionPlan, Graph},
        node::Registry,
    };

    #[test]
    fn permits_one_restart_only_for_a_previously_ready_closed_owner() {
        let mut restarted = false;
        assert!(!claim_pre_delivery_restart(&mut restarted, false, false));
        assert!(!claim_pre_delivery_restart(&mut restarted, true, false));
        assert!(!claim_pre_delivery_restart(&mut restarted, false, true));
        assert!(!restarted);
        assert!(claim_pre_delivery_restart(&mut restarted, true, true));
        assert!(restarted);
        assert!(!claim_pre_delivery_restart(&mut restarted, true, true));
    }

    #[test]
    fn closed_ready_driver_releases_original_charge_before_an_undelivered_restart() {
        use super::super::retained_tests::{fixture_empty, fixture_host, fixture_key};
        use std::{fs::File, io::Read, os::windows::io::OwnedHandle};

        let host = fixture_host("ready-owner-before-delivery");
        let gate = ExecutionGate::default();
        let key = fixture_key(1);
        let target = GrantTarget::new(
            "ready-owner-fixture".into(),
            &key.session_epoch,
            key.plan_version,
            "node".into(),
        );
        gate.approve(target.clone());
        let reserve = |cancel: &CancellationToken| {
            RetainedReservation::reserve(
                &host,
                vec![RetainedBinding {
                    module_slot: 0,
                    scope: fixture_empty(&gate, &target, 1, cancel.clone()),
                }],
                cancel,
            )
        };
        let old_cancel = CancellationToken::default();
        let original = reserve(&old_cancel).unwrap();
        let (mut client, request) = RetainedClient::channel(
            host.clone(),
            original.clone(),
            vec![0],
            DriverPolicy {
                idle_timeout: Duration::from_secs(5),
                input_timeout: Duration::from_secs(5),
                max_calls: 1,
                max_input_bytes: 4096,
                max_reply_bytes: 4096,
            },
        )
        .unwrap();
        let (ready_reader, mut ready_writer) = std::io::pipe().unwrap();
        let (mut input_reader, input_writer) = std::io::pipe().unwrap();
        crate::isolated_worker::retained_startup::write_ready(&mut ready_writer, 1, None).unwrap();
        drop(ready_writer);
        let RetainedWorkerRequest {
            reservation,
            exchange,
        } = request;
        let old_io = std::thread::spawn(move || {
            let _io_pin = reservation;
            exchange(
                File::from(OwnedHandle::from(ready_reader)),
                File::from(OwnedHandle::from(input_writer)),
            )
        });
        let timeout = Duration::from_secs(2);
        client.wait_ready(timeout).unwrap();
        old_cancel.cancel();
        assert_eq!(
            client
                .begin_input(key, CancellationToken::default())
                .err()
                .expect("closed owner cannot begin the pending input")
                .code,
            "retained-scope"
        );
        assert_eq!(
            client.wait_ready(timeout).unwrap_err().code,
            "retained-scope"
        );
        let mut restarted = false;
        assert!(claim_pre_delivery_restart(
            &mut restarted,
            true,
            original.is_closed()
        ));
        let replacement_cancel = CancellationToken::default();
        assert!(
            reserve(&replacement_cancel).is_err(),
            "old physical charge still owns the slot"
        );

        assert!(old_io.join().unwrap().is_err());
        drop(client);
        drop(original);
        let mut sent = Vec::new();
        input_reader.read_to_end(&mut sent).unwrap();
        assert!(
            sent.is_empty(),
            "no invocation was sent to the closed owner"
        );
        let replacement = reserve(&replacement_cancel).unwrap();
        assert!(!replacement.is_closed());
        assert!(!claim_pre_delivery_restart(&mut restarted, true, true));
        drop(replacement);
        assert_eq!(host.usage(), ResourceUsage::default());
    }

    #[test]
    fn ready_cache_read_never_waits_or_publishes_code() {
        let cancel = CancellationToken::default();
        let cache = Mutex::new(CompiledState::Pending);
        assert!(ready_retained_code(&cache, &cancel).is_none());
        *cache.lock().unwrap() = CompiledState::Unavailable;
        assert!(ready_retained_code(&cache, &cancel).is_none());
        let code = b"synthetic cache fixture, never executed";
        let files = Arc::new(
            CodeFiles::new(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &[("node.cwasm", code)],
                &Arc::new(CodeBudget::default()),
            )
            .unwrap(),
        );
        let digest = sha256(code).unwrap();
        let mut state = cache.lock().unwrap();
        *state = CompiledState::Ready(CompiledAsset {
            files: files.clone(),
            len: code.len(),
            digest: digest.clone(),
        });
        assert!(ready_retained_code(&cache, &cancel).is_none());
        drop(state);
        let Some(WorkerImage::PreparedCompiled(read, len, observed)) =
            ready_retained_code(&cache, &cancel)
        else {
            panic!("expected the already captured asset");
        };
        assert!(Arc::ptr_eq(&read, &files));
        assert_eq!((len, observed), (code.len(), digest));
        cancel.cancel();
        assert!(ready_retained_code(&cache, &cancel).is_none());
    }

    #[test]
    #[ignore = "requires the actual Worker, pinned DLL and qualified fixture"]
    fn real_reviewed_coordinator_requeues_after_ready_owner_exit() {
        let asset =
            |name| PathBuf::from(std::env::var_os(name).expect("explicit Worker fixture asset"));
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("retained-ready-requeue-{}", std::process::id()));
        std::fs::create_dir(&workspace).unwrap();
        let mut policy = WorkerPolicy::bounded_probe();
        policy.termination_grace = Duration::from_millis(5);
        let artifact = WasmArtifact::capture(
            &workspace,
            &asset("DIY_RANGE_WORKER_BINARY"),
            &asset("DIY_RANGE_RUNTIME_DLL"),
            std::fs::read(asset("DIY_RETAINED_HOST_FIXTURE_WASM")).unwrap(),
            policy,
        )
        .unwrap();
        let gate = ExecutionGate::default();
        let config = json!({"mode":"echo"});
        let node = artifact
            .prepare(
                "fixture.wasm",
                1,
                vec![config.clone()],
                gate.clone(),
                CancellationToken::default(),
            )
            .unwrap()
            .bind_package("retained-ready-requeue-fixture".into());
        let mut registry = Registry::new();
        register_system_nodes(&mut registry).unwrap();
        registry.register_deferred(node.clone()).unwrap();
        let graph = Graph::from_json(
            &json!({
                "version":1,
                "nodes":[{"id":"input","kind":"input.text","version":1},
                         {"id":"stage0","kind":"fixture.wasm","version":1,"config":config}],
                "edges":[{"from":{"node":"input","port":"out"},"to":{"node":"stage0","port":"in"}}]
            })
            .to_string(),
        )
        .unwrap();
        let resources: Value = serde_json::from_str(include_str!(
            "../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let host = ExecutionHost::new(
            "real-reviewed-ready-requeue",
            ResourcePolicy::from_value(resources["resourceProfiles"]["noTombstones"].clone())
                .unwrap(),
        )
        .unwrap();
        let service = ExecutionService::new(host.clone()).unwrap();
        let plan_version = DecimalId::new(1).unwrap();
        let raw = service
            .open_ready(
                ExecutionPlan::compile(&graph, &registry).unwrap(),
                plan_version,
            )
            .unwrap();
        let target = node.grant_target(raw.session_epoch(), plan_version, "stage0");
        gate.approve(target.clone());
        let controller = gate.seal(raw, vec![target]).unwrap();
        let owner = Owner::install_evaluation(
            host.clone(),
            controller.clone(),
            vec![("stage0".into(), node, config)],
            Arc::new(retention_evaluation::StartupGate::bounded(2).unwrap()),
            Arc::new(Mutex::new(())),
            Some(std::time::Instant::now()),
            None,
        )
        .unwrap()
        .unwrap();
        owner.start();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !owner.ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "original owner must become ready"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(owner.compiled_generations(), 1);
        let old_pid = owner.process_id().unwrap();
        owner
            .shared
            .end_ready_owner_before_input
            .store(true, Ordering::Release);
        let text = "same latest input";
        let request: StartRequest = serde_json::from_value(json!({
            "startKey":"1","expectedPlanVersion":"1","targets":[{"node":"stage0","port":"out"}],
            "input":{"revision":1,"text":text,"mode":"compose"}
        }))
        .unwrap();
        let key = controller.start_run(request).unwrap().key;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let result = loop {
            let snapshot = controller.get_run(&key).unwrap();
            if snapshot.state == RunPhase::Terminal
                && snapshot.physical.running == 0
                && snapshot.physical.cancel_requested == 0
                && snapshot.physical.queued == 0
                && snapshot.physical.stalled == 0
            {
                assert!(snapshot.current);
                break snapshot.result.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "latest input must settle"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(result.delivery, RunDelivery::Complete);
        assert_eq!(result.ports[0].value["packet"]["root"]["text"], text);
        assert_eq!(owner.compiled_generations(), 2);
        let replacement_pid = owner.process_id().unwrap();
        assert_ne!(old_pid, replacement_pid);
        let samples = owner.call_samples().unwrap();
        assert_eq!(samples["calls"]["phaseDropped"], 0);
        let phases = samples["calls"]["phases"].as_array().unwrap();
        let stage = |name, run: Option<DecimalId>, pid: Option<u64>| {
            let matches: Vec<_> = phases
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    row["phase"] == name
                        && run.is_none_or(|id| row["runId"] == json!(id))
                        && pid.is_none_or(|id| row["processId"] == id)
                })
                .collect();
            assert_eq!(matches.len(), 1, "{name}: {matches:?}");
            matches[0].0
        };
        let dequeued = stage("invocation-dequeued", Some(key.run_id), Some(old_pid));
        let ended = stage("original-process-io-ended", None, Some(old_pid));
        let joined = stage("owner-joined", None, Some(old_pid));
        let restarted = stage(
            "invocation-requeued-after-owner-end",
            Some(key.run_id),
            None,
        );
        let called = stage("invocation-call-started", Some(key.run_id), None);
        assert_eq!(phases[called]["processId"], replacement_pid);
        assert!(dequeued < ended && ended < joined && joined < restarted && restarted < called);
        controller.close();
        drop(owner);
        drop(controller);
        drop(service);
        drop(registry);
        drop(artifact);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while host.usage() != ResourceUsage::default() {
            assert!(
                std::time::Instant::now() < deadline,
                "remaining ownership: {:?}",
                host.usage()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        std::fs::remove_dir(workspace).unwrap();
        eprintln!(
            "reviewed ready requeue: original PID {old_pid}, successor PID {replacement_pid}"
        );
    }

    /// Direct trusted-native composition for Core bridge verification only.
    /// No external package or graph acquires fixed-official eligibility here.
    #[test]
    #[ignore = "requires explicit real Windows Worker/runtime/fixture"]
    fn real_core_skipped_suffix() {
        let required = |name| {
            PathBuf::from(std::env::var_os(name).expect("explicit retained Core fixture asset"))
        };
        let worker = required("DIY_RANGE_WORKER_BINARY");
        let dll = required("DIY_RANGE_RUNTIME_DLL");
        let wasm = required("DIY_RETAINED_HOST_FIXTURE_WASM");
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "retained-core-suffix-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let mut policy = WorkerPolicy::bounded_probe();
        std::fs::create_dir(&workspace).unwrap();
        policy.termination_grace = Duration::from_millis(5);
        let module = std::fs::read(wasm).unwrap();
        let artifact =
            WasmArtifact::capture(&workspace, &worker, &dll, module.clone(), policy.clone())
                .unwrap();
        // A valid custom section gives the second stage a separate code identity.
        let mut alternate_module = module;
        alternate_module.extend_from_slice(&[0, 2, 1, b'x']);
        let alternate_artifact = artifact.with_module(alternate_module, policy).unwrap();
        let gate = ExecutionGate::default();
        let node = artifact
            .prepare(
                "fixture.wasm.chain",
                1,
                vec![json!({"mode":"noOutput"})],
                gate.clone(),
                CancellationToken::default(),
            )
            .unwrap();
        let alternate_node = artifact
            .prepare(
                "fixture.wasm.chain-alt",
                1,
                vec![json!({"mode":"preparedNoOutput"})],
                gate.clone(),
                CancellationToken::default(),
            )
            .unwrap();
        assert!(
            alternate_node
                .prepared_dictionary
                .set(super::prepared::PreparedDictionary {
                    consumer: alternate_artifact.clone(),
                    index: PreparedArtifact::retained_fixture(),
                })
                .is_ok()
        );
        let mut registry = Registry::new();
        register_system_nodes(&mut registry).unwrap();
        registry.register_deferred(node.clone()).unwrap();
        registry.register_deferred(alternate_node.clone()).unwrap();
        let ids: Vec<_> = (0..4).map(|n| format!("stage{n}")).collect();
        let nodes = [&node, &alternate_node, &node, &node];
        let configs: Vec<_> = (0..4)
            .map(|n| json!({"mode":if n==1{"preparedNoOutput"}else{"echo"}}))
            .collect();
        let mut graph_nodes = vec![json!({"id":"input","kind":"input.text","version":1})];
        graph_nodes.extend(ids.iter().zip(&configs).zip(nodes).map(
            |((id, config), node)| {
                json!({"id":id,"kind":node.definition.kind,"version":1,"config":config})
            },
        ));
        let mut edges =
            vec![json!({"from":{"node":"input","port":"out"},"to":{"node":ids[0],"port":"in"}})];
        edges.extend((0..3).map(
            |i| json!({"from":{"node":ids[i],"port":"out"},"to":{"node":ids[i+1],"port":"in"}}),
        ));
        let graph =
            Graph::from_json(&json!({"version":1,"nodes":graph_nodes,"edges":edges}).to_string())
                .unwrap();
        let resource_policy: Value = serde_json::from_str(include_str!(
            "../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        // Use the bounded fixture without host-lifetime run history so the
        // teardown can require every resource counter to return to zero.
        let host = ExecutionHost::new(
            "actual-core-skipped-suffix",
            ResourcePolicy::from_value(resource_policy["resourceProfiles"]["noTombstones"].clone())
                .unwrap(),
        )
        .unwrap();
        let service = ExecutionService::new(host.clone()).unwrap();
        let raw = service
            .open_ready(
                ExecutionPlan::compile(&graph, &registry).unwrap(),
                DecimalId::new(1).unwrap(),
            )
            .unwrap();
        let targets: Vec<_> = ids
            .iter()
            .zip(nodes)
            .map(|(id, node)| {
                node.grant_target(raw.session_epoch(), DecimalId::new(1).unwrap(), id)
            })
            .collect();
        for target in &targets {
            gate.approve(target.clone());
        }
        let controller = gate.seal(raw, targets).unwrap();
        let stages = ids
            .iter()
            .zip(&configs)
            .zip(nodes)
            .map(|((id, config), node)| (id.clone(), node.clone(), config.clone()))
            .collect();
        let owner = Owner::install(host.clone(), controller.clone(), stages)
            .unwrap()
            .unwrap();
        owner.start();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !owner.ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "remaining ownership: {:?}",
                host.usage()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let pid = owner.process_id().unwrap();
        for revision in 1..=3 {
            if revision > 1 {
                owner.witness_terminal(&controller);
                controller
                    .invalidate_input(DecimalId::new(1).unwrap(), revision)
                    .unwrap();
            }
            let request:StartRequest=serde_json::from_value(json!({"startKey":revision.to_string(),"expectedPlanVersion":"1","targets":[{"node":ids[3],"port":"out"}],"input":{"revision":revision,"text":"sample","mode":"compose"}})).unwrap();
            let receipt = controller.start_run(request).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let snapshot = loop {
                let snapshot = controller.get_run(&receipt.key).unwrap();
                if snapshot.state == RunPhase::Terminal
                    && snapshot.physical.running == 0
                    && snapshot.physical.cancel_requested == 0
                    && snapshot.physical.stalled == 0
                    && snapshot.physical.queued == 0
                {
                    break snapshot;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            };
            assert_eq!(
                snapshot.result.as_ref().unwrap().delivery,
                RunDelivery::NoOutput
            );
            let result = snapshot.result.unwrap();
            for id in &ids[2..] {
                assert_eq!(
                    result.nodes.iter().find(|n| n["nodeId"] == *id).unwrap()["status"],
                    "skipped"
                );
            }
            assert_eq!(owner.process_id(), Some(pid));
            assert_eq!(
                owner.shared.replies.load(Ordering::Acquire),
                revision * 2,
                "Core Skipped suffix must not cause invented native calls"
            );
        }
        controller.close();
        drop(owner);
        drop(controller);
        drop(service);
        drop(registry);
        drop(node);
        drop(alternate_node);
        drop(artifact);
        drop(alternate_artifact);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let usage = host.usage();
            if usage == ResourceUsage::default() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "remaining ownership after closing retained worker: {usage:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        // Cache::new leaves the shared staging parents in place. This test
        // owns its workspace, so remove those parents only once they are empty.
        let staging = workspace.join("target/appcontainer-probes");
        loop {
            match std::fs::remove_dir(&staging) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "retained fixture staging still contains: {:?}",
                        std::fs::read_dir(&staging).map(|entries| {
                            entries
                                .map(|entry| entry.map(|entry| entry.file_name()))
                                .collect::<Vec<_>>()
                        })
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("could not remove retained fixture staging: {error}"),
            }
        }
        std::fs::remove_dir(workspace.join("target")).unwrap();
        std::fs::remove_dir(workspace).unwrap();
    }
}
