//! Session-owned fixed-official coordinator. Nodes hold weak links only.
use super::*;
use crate::execution_control::{ExecutionHost, RunDelivery, RunKey, RunPhase, RunSnapshot};
use crate::isolated_worker::{
    DriverPolicy, RetainedBinding, RetainedClient, RetainedReservation, RetainedWorkerRequest,
};
use std::sync::{Mutex, Weak, mpsc};

type Arguments = (OwnedInvocation, CompletionPermit, CancellationToken);
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
    lifetime: Mutex<Option<CancellationToken>>,
    physical: AtomicU64,
    ready: std::sync::atomic::AtomicBool,
    process_id: Arc<AtomicU64>,
    generation: AtomicU64,
    start: std::sync::atomic::AtomicBool,
    ids: Vec<String>,
    replies: AtomicU64,
    startup_gate: Option<Arc<retention_evaluation::StartupGate>>,
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
        Self::install_stages(host, controller, stages, false, None, None, None)
    }

    pub(super) fn install_evaluation(
        host: ExecutionHost,
        controller: GuardedController,
        stages: Vec<(String, WasmNode, Value)>,
        startup_gate: Arc<retention_evaluation::StartupGate>,
        control_probe: Arc<Mutex<()>>,
        diagnostic_origin: Option<std::time::Instant>,
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
            true,
            Some(startup_gate),
            Some(control_probe),
            diagnostic_origin,
        )
    }

    fn install_stages(
        host: ExecutionHost,
        controller: GuardedController,
        stages: Vec<(String, WasmNode, Value)>,
        evaluation: bool,
        startup_gate: Option<Arc<retention_evaluation::StartupGate>>,
        control_probe: Option<Arc<Mutex<()>>>,
        diagnostic_origin: Option<std::time::Instant>,
    ) -> Result<Option<Self>, CoreError> {
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
        let shared = Arc::new(Shared {
            qualifications,
            send,
            stop: CancellationToken::default(),
            input: Mutex::new(None),
            lifetime: Mutex::new(None),
            physical: AtomicU64::new(0),
            ready: std::sync::atomic::AtomicBool::new(false),
            process_id: Arc::new(AtomicU64::new(0)),
            generation: AtomicU64::new(0),
            start: std::sync::atomic::AtomicBool::new(false),
            ids: ids.clone(),
            replies: AtomicU64::new(0),
            startup_gate,
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

impl Native {
    fn start(
        host: &ExecutionHost,
        controller: &GuardedController,
        nodes: &[WasmNode],
        ids: &[String],
        configs: &[Value],
        shared: &Arc<Shared>,
    ) -> Result<Self, CoreError> {
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
            .map(|gate| gate.acquire(&shared.stop))
            .transpose()?;
        shared.record_phase("startup-permit-acquired");
        let state = control_state(shared, controller)?;
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
            .collect::<Result<Vec<_>, CoreError>>()?;
        // Reserve BEFORE any compiler process starts. Preparation and the
        // replacement process never overlap and use this same charged slot.
        let pin = RetainedReservation::reserve(host, bindings, &cancel)?;
        shared.record_phase("reservation-acquired");
        shared.record_phase("preparation-permit-waiting");
        let preparation_permit = shared
            .startup_gate
            .as_ref()
            .map(|gate| gate.prepare(&cancel))
            .transpose()?
            .flatten();
        shared.record_phase("preparation-permit-acquired");
        shared.record_phase("code-preparation-started");
        let compiled = if shared.startup_gate.is_some()
            && nodes.len() == 1
            && retention_evaluation::Owners::evaluation_group_size()? == Some(1)
            && std::env::var_os("DIY_EXPERIMENT_RETAINED_COMPILED_JIT").as_deref()
                == Some(std::ffi::OsStr::new("1"))
        {
            // Source selection is this captured Node's own factory. Preparing
            // code runs no input/Instance and settles the original compiler
            // before granting a new retained process access to its output.
            match modules[0].prepared_jit_image_owned(&cancel, Some(pin.clone())) {
                WorkerImage::PreparedCompiled(files, len, digest) => {
                    files.check_healthy()?;
                    Some(RetainedCompiledCode {
                        files,
                        len,
                        digest,
                        source: modules[0].0.identity.rsplit(':').next().unwrap().to_owned(),
                        timed_startup: shared.samples.is_some()
                            && std::env::var_os("DIY_EXPERIMENT_RETAINED_NATIVE_STARTUP_TIMING")
                                .as_deref()
                                == Some(std::ffi::OsStr::new("1")),
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
        let startup_observer = compiled.as_ref().filter(|code| code.timed_startup).map(|_| {
            let process_id = shared.process_id.clone();
            Box::new(move |timing: crate::isolated_worker::retained_startup::Timing| {
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "retained-native-startup pid={} verify_us={} load_us={} read_us={} restore_us={}",
                    process_id.load(Ordering::Acquire), timing.dll_verify_us, timing.dll_load_us,
                    timing.module_read_us, timing.module_restore_us,
                );
            }) as crate::isolated_worker::retained_startup::Observer
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
        let control = RunControl {
            cancel: cancel.clone(),
            walltime: Some(Duration::from_secs(600)),
            termination_grace: artifact.0.policy.termination_grace,
        };
        let thread = std::thread::Builder::new()
            .name("diyime-retained-native-owner".into())
            .spawn(move || {
                artifact.retained_lifetime(
                    &modules,
                    compiled,
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
    timed_startup: bool,
}

impl WasmArtifact {
    fn retained_lifetime(
        &self,
        modules: &[WasmArtifact],
        compiled: Option<RetainedCompiledCode>,
        request: RetainedWorkerRequest,
        control: &RunControl,
        _cleanup_pin: RetainedReservation,
        shared: Arc<Shared>,
    ) -> Result<Option<ExitReceipt>, CoreError> {
        shared.record_phase("profile-create-started");
        let mut profile = PreparationProfile {
            profile: Profile::new()
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
                TIMED_FLAG, VERIFIED_FLAG, VERIFIED_TIMED_FLAG,
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
            let timed = compiled.as_ref().is_some_and(|code| code.timed_startup);
            let mode = match (compiled.is_some(), timed, verified_runtime.is_some()) {
                (true, true, true) => VERIFIED_TIMED_FLAG,
                (true, false, true) => VERIFIED_FLAG,
                (true, true, false) => TIMED_FLAG,
                (true, false, false) => "--retained-compiled-resources-v1",
                (false, _, _) => "--retained-resources-v1",
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
        for mut lease in module_leases {
            while lease.release().is_err() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        while runtime_lease.release().is_err() {
            std::thread::sleep(Duration::from_millis(100));
        }
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
    let mut native = Native::start(&host, &controller, &nodes, &ids, &configs, &shared)
        .inspect_err(|_| {
            shared.startup_failures.fetch_add(1, Ordering::AcqRel);
        })
        .ok();
    let mut pending = None;
    while !shared.stop.is_cancelled() && control_state(&shared, &controller).is_ok() {
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
                shared.startup_failures.fetch_add(1, Ordering::AcqRel);
                drop(native.take());
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
            drop(native.take());
        }
        let command = match pending
            .take()
            .map(Ok)
            .unwrap_or_else(|| receive.recv_timeout(Duration::from_millis(5)))
        {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        shared.record_run_phase("invocation-dequeued", Some(command.invocation.key().run_id));
        if command.cancel.is_cancelled() {
            command.fail(fail("worker-cancelled", "invocation cancelled"));
            continue;
        }
        if command.stage != 0 {
            command.legacy(&shared);
            continue;
        }
        if native.is_none() {
            native = Native::start(&host, &controller, &nodes, &ids, &configs, &shared).ok();
        }
        let Some(owner) = native.as_mut() else {
            command.legacy(&shared);
            continue;
        };
        if owner
            .client
            .as_mut()
            .unwrap()
            .wait_ready(Duration::from_secs(10))
            .is_err()
        {
            drop(native.take());
            command.legacy(&shared);
            continue;
        }
        if owner.startup_permit.is_some() {
            shared.record_phase("readiness-observed");
        }
        drop(owner.startup_permit.take());
        shared.ready.store(true, Ordering::Release);
        if command.cancel.is_cancelled() {
            command.fail(fail(
                "worker-cancelled",
                "invocation cancelled before delivery",
            ));
            continue;
        }
        let key = command.invocation.key().clone();
        let input_cancel = CancellationToken::default();
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
            drop(native.take());
            *shared.input.lock().unwrap_or_else(|p| p.into_inner()) = None;
            if matches!(
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
