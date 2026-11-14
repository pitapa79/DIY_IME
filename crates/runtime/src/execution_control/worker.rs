use super::{DecimalId, ResourceLease, RunKey};
use crate::{error_input::ErrorAwareExecution, node::CoreError, runtime::NodeExecution};
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStamp {
    pub key: RunKey,
    pub job_id: DecimalId,
}
pub(crate) enum WorkerOutcome {
    Sync(NodeExecution),
    Deferred(Result<ErrorAwareExecution, CoreError>),
    Terminated,
    StartReturned(Result<(), CoreError>),
    NotStarted,
}
pub(crate) struct WorkerEvent {
    pub stamp: JobStamp,
    pub authority: Arc<()>,
    pub outcome: WorkerOutcome,
}
#[derive(Clone)]
pub(crate) struct WorkerSender {
    events: Sender<WorkerEvent>,
    wake: super::CompletionWake,
    pending: Arc<AtomicBool>,
}
impl WorkerSender {
    fn send(&self, event: WorkerEvent) -> Result<(), ()> {
        self.events.send(event).map_err(|_| ())?;
        self.pending.store(true, Ordering::Release);
        self.wake.notify();
        Ok(())
    }
}
pub(crate) struct PermitState {
    pub stamp: JobStamp,
    pub authority: Arc<()>,
    pub sender: WorkerSender,
    pub finished: AtomicBool,
    pub host_cancelled: AtomicBool,
    pub lease: Mutex<Option<ResourceLease>>,
    pub invocation: Mutex<Option<super::OwnedInvocation>>,
}
impl PermitState {
    fn release_owned(&self) {
        let lease = self.lease.lock().unwrap_or_else(|p| p.into_inner()).take();
        let invocation = self
            .invocation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        drop(lease);
        drop(invocation);
    }
}

use super::{CancellationToken, ExecutionHost, Milliseconds, OwnedInvocation, ResourceUsage};
use crate::{node::RegisteredImplementation, runtime};
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerJobState {
    Running,
    CancelRequested,
    Stalled,
}
struct Job {
    stamp: JobStamp,
    authority: Arc<()>,
    invocation: OwnedInvocation,
    _lease: ResourceLease,
    cancel: CancellationToken,
    permit: Option<Arc<PermitState>>,
    state: WorkerJobState,
    cancelled_at: Option<Duration>,
    start_returned: bool,
    finished: bool,
    delivered: bool,
}
/// An authenticated dispatcher receipt. External Rust code may inspect or replay it,
/// but cannot manufacture one merely by copying job identifiers.
/// ```compile_fail
/// use diy_ime2_core::execution_control::{JobStamp, WorkerCompletion};
/// fn forge(stamp: JobStamp) -> WorkerCompletion {
///     WorkerCompletion { stamp, execution: None, physically_finished: true }
/// }
/// ```
#[derive(Clone)]
pub struct WorkerCompletion {
    pub(crate) stamp: JobStamp,
    pub(crate) execution: Option<NodeExecution>,
    pub(crate) physically_finished: bool,
}
impl WorkerCompletion {
    pub fn stamp(&self) -> &JobStamp {
        &self.stamp
    }
    pub fn execution(&self) -> Option<&NodeExecution> {
        self.execution.as_ref()
    }
    pub fn physically_finished(&self) -> bool {
        self.physically_finished
    }
    pub fn into_execution(self) -> Option<NodeExecution> {
        self.execution
    }
}
/// One physical reservation per submitted job. The scheduler owns queue policy.
pub struct WorkerDispatcher {
    host: ExecutionHost,
    sender: WorkerSender,
    receiver: Receiver<WorkerEvent>,
    jobs: BTreeMap<DecimalId, Job>,
    #[cfg(test)]
    start_gate: Option<Arc<Mutex<Receiver<()>>>>,
}
impl WorkerDispatcher {
    pub fn new(host: ExecutionHost) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            host,
            sender: WorkerSender {
                events: sender,
                wake: super::CompletionWake::default(),
                pending: Arc::new(AtomicBool::new(false)),
            },
            receiver,
            jobs: BTreeMap::new(),
            #[cfg(test)]
            start_gate: None,
        }
    }
    pub fn submit(&mut self, invocation: OwnedInvocation) -> Result<DecimalId, CoreError> {
        if matches!(
            invocation.plan.nodes[invocation.index].node.implementation,
            RegisteredImplementation::Control { .. }
        ) {
            return Err(CoreError::new(
                "unsupported-capability",
                "control callbacks are observers, not physical jobs",
            ));
        }
        let lease = self.host.try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })?;
        let id = issue_job_id()?;
        self.submit_reserved(invocation, id, lease)?;
        Ok(id)
    }
    pub(crate) fn submit_reserved(
        &mut self,
        invocation: OwnedInvocation,
        id: DecimalId,
        lease: ResourceLease,
    ) -> Result<(), CoreError> {
        self.host.check_lease(
            &lease,
            ResourceUsage {
                physical_jobs: 1,
                ..Default::default()
            },
        )?;
        if self.jobs.contains_key(&id) {
            return Err(CoreError::new("job-conflict", "job ID already running"));
        }
        let planned = &invocation.plan.nodes[invocation.index];
        if matches!(
            planned.node.implementation,
            RegisteredImplementation::Control { .. }
        ) {
            return Err(CoreError::new(
                "unsupported-capability",
                "control callbacks are observers, not physical jobs",
            ));
        }
        let stamp = JobStamp {
            key: invocation.key.clone(),
            job_id: id,
        };
        let authority = Arc::new(());
        let cancel = CancellationToken::default();
        let permit = matches!(
            planned.node.implementation,
            RegisteredImplementation::Deferred { .. }
        )
        .then(|| {
            Arc::new(PermitState {
                stamp: stamp.clone(),
                authority: Arc::clone(&authority),
                sender: self.sender.clone(),
                finished: AtomicBool::new(false),
                host_cancelled: AtomicBool::new(false),
                lease: Mutex::new(Some(lease.clone())),
                invocation: Mutex::new(Some(invocation.clone())),
            })
        });
        let work = invocation.clone();
        let sender = self.sender.clone();
        let worker_stamp = stamp.clone();
        let worker_authority = Arc::clone(&authority);
        let worker_lease = lease.clone();
        let worker_cancel = cancel.clone();
        let worker_permit = permit.clone();
        #[cfg(test)]
        let start_gate = self.start_gate.clone();
        std::thread::Builder::new()
            .name(format!("diy-ime-job-{}", id.get()))
            .spawn(move || {
                #[cfg(test)]
                if let Some(gate) = start_gate {
                    let _ = gate.lock().unwrap_or_else(|p| p.into_inner()).recv();
                }
                let planned = &work.plan.nodes[work.index];
                // This check claims callback start. A cancellation accepted
                // before it need not call Node code merely to end the worker.
                let outcome = if worker_cancel.is_cancelled() {
                    if let Some(state) = &worker_permit {
                        state.finished.store(true, Ordering::Release);
                        state.release_owned();
                    }
                    WorkerOutcome::NotStarted
                } else {
                    work.trace_event(
                        super::TraceKind::NodeStarted,
                        Some(worker_stamp.job_id.get()),
                    );
                    match &planned.node.implementation {
                        RegisteredImplementation::Deferred { node, .. } => {
                            let state = worker_permit.expect("deferred permit");
                            let completion = CompletionPermit {
                                state: Arc::clone(&state),
                            };
                            let outcome = catch_unwind(AssertUnwindSafe(|| {
                                node.start(work.clone(), completion, worker_cancel)
                            }))
                            .unwrap_or_else(|_| {
                                Err(CoreError::new("node-panic", "deferred start panicked"))
                            });
                            WorkerOutcome::StartReturned(outcome)
                        }
                        _ => WorkerOutcome::Sync(runtime::execute(
                            planned,
                            work.context(),
                            &work.inputs,
                            &work.input_recoveries,
                            work.input_errors.as_ref(),
                        )),
                    }
                };
                // The dispatcher retains its own lease until this authenticated event is consumed.
                drop(work);
                drop(worker_lease);
                let _ = sender.send(WorkerEvent {
                    stamp: worker_stamp,
                    authority: worker_authority,
                    outcome,
                });
            })
            .map_err(|error| CoreError::new("worker-unavailable", error.to_string()))?;
        self.jobs.insert(
            id,
            Job {
                stamp,
                authority,
                invocation,
                _lease: lease,
                cancel,
                permit,
                state: WorkerJobState::Running,
                cancelled_at: None,
                start_returned: false,
                finished: false,
                delivered: false,
            },
        );
        Ok(())
    }
    pub fn cancel(&mut self, id: DecimalId, now: Duration) -> bool {
        let Some(job) = self.jobs.get_mut(&id) else {
            return false;
        };
        // An authenticated producer ending can precede start's return. Keep
        // its lease until that return, but do not cancel the ended producer.
        // A start error alone does not set finished: spawned work still needs
        // cancellation and a genuine producer ending in that case.
        if job.finished {
            return false;
        }
        if job.cancelled_at.is_none() {
            job.cancelled_at = Some(now);
            job.state = WorkerJobState::CancelRequested;
            if let Some(t) = &job.invocation.trace {
                let p = &job.invocation.plan.nodes[job.invocation.index];
                t.emit(
                    super::TraceKind::CallCancelled,
                    Some((
                        job.invocation.index,
                        &p.node.definition.kind,
                        p.node.definition.version,
                    )),
                    Some(id.get()),
                );
            }
            if let Some(permit) = &job.permit {
                permit.host_cancelled.store(true, Ordering::Release);
            }
            job.cancel.cancel();
        }
        true
    }
    pub fn mark_stalled(&mut self, now: Duration, grace: Milliseconds) {
        for job in self.jobs.values_mut() {
            if job
                .cancelled_at
                .is_some_and(|at| now.saturating_sub(at) >= grace.duration())
            {
                if job.state != WorkerJobState::Stalled
                    && let Some(t) = &job.invocation.trace
                {
                    let p = &job.invocation.plan.nodes[job.invocation.index];
                    t.emit(
                        super::TraceKind::CallStalled,
                        Some((
                            job.invocation.index,
                            &p.node.definition.kind,
                            p.node.definition.version,
                        )),
                        Some(job.stamp.job_id.get()),
                    );
                }
                job.state = WorkerJobState::Stalled;
            }
        }
    }
    pub fn states(&self) -> impl Iterator<Item = (&RunKey, WorkerJobState)> {
        self.jobs.values().map(|job| (&job.stamp.key, job.state))
    }
    pub(crate) fn next_grace_deadline(&self, grace: Milliseconds) -> Option<Duration> {
        self.jobs
            .values()
            .filter(|job| job.state == WorkerJobState::CancelRequested)
            .filter_map(|job| {
                job.cancelled_at
                    .and_then(|at| at.checked_add(grace.duration()))
            })
            .min()
    }
    pub fn wake_signal(&self) -> super::CompletionWake {
        self.sender.wake.clone()
    }
    /// Adopt one authenticated public event. Internal start-return notifications
    /// may be consumed first; all later public events remain in FIFO order.
    pub fn poll_one(&mut self) -> Option<WorkerCompletion> {
        self.sender.pending.store(false, Ordering::Release);
        while let Ok(event) = self.receiver.try_recv() {
            if let Some(done) = self.consume(event) {
                return Some(done);
            }
        }
        None
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.sender.pending.load(Ordering::Acquire)
    }
    pub fn poll(&mut self) -> Vec<WorkerCompletion> {
        std::iter::from_fn(|| self.poll_one()).collect()
    }
    fn consume(&mut self, event: WorkerEvent) -> Option<WorkerCompletion> {
        let job = self.jobs.get_mut(&event.stamp.job_id)?;
        if job.stamp != event.stamp || !Arc::ptr_eq(&job.authority, &event.authority) {
            return None;
        }
        let mut execution = None;
        match event.outcome {
            WorkerOutcome::NotStarted => {
                if job.finished || job.cancelled_at.is_none() {
                    return None;
                }
                job.finished = true;
                job.start_returned = true;
                job.delivered = true;
            }
            WorkerOutcome::Sync(mut result) => {
                if job.finished {
                    return None;
                }
                job.finished = true;
                job.start_returned = true;
                job.delivered = true;
                result.input_errors = job.invocation.input_errors.clone();
                execution = Some(result);
            }
            WorkerOutcome::Deferred(result) => {
                if job.finished {
                    return None;
                }
                job.finished = true;
                if !job.delivered {
                    execution = Some(resolve(&job.invocation, result));
                    job.delivered = true;
                }
            }
            WorkerOutcome::Terminated => {
                if job.finished {
                    return None;
                }
                job.finished = true;
            }
            WorkerOutcome::StartReturned(result) => {
                if job.start_returned {
                    return None;
                }
                job.start_returned = true;
                if let Err(error) = result
                    && !job.delivered
                {
                    execution = Some(resolve(&job.invocation, Err(error)));
                    job.delivered = true;
                    // A start error is a logical failure, not proof a spawned worker ended.
                }
            }
        }
        if job.cancelled_at.is_some() {
            execution = None;
        }
        let stamp = job.stamp.clone();
        let ended = job.finished && job.start_returned;
        if ended {
            let trace = job
                .invocation
                .trace
                .as_ref()
                .filter(|t| t.is_recording())
                .cloned()
                .map(|t| {
                    let p = &job.invocation.plan.nodes[job.invocation.index];
                    (
                        t,
                        job.invocation.index,
                        p.node.definition.kind.chars().take(128).collect::<String>(),
                        p.node.definition.version,
                    )
                });
            self.jobs.remove(&stamp.job_id);
            if let Some((t, index, kind, version)) = trace {
                t.emit(
                    super::TraceKind::CallEnded,
                    Some((index, &kind, version)),
                    Some(stamp.job_id.get()),
                );
            }
        }
        if execution.is_some() || ended {
            Some(WorkerCompletion {
                stamp,
                execution,
                physically_finished: ended,
            })
        } else {
            None
        }
    }
}
pub(crate) fn issue_job_id() -> Result<DecimalId, CoreError> {
    Ok(issue_job_ids(1)?.remove(0))
}
pub(crate) fn issue_job_ids(count: usize) -> Result<Vec<DecimalId>, CoreError> {
    static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
    allocate_job_ids(&NEXT_JOB, count)
}
fn allocate_job_ids(counter: &AtomicU64, count: usize) -> Result<Vec<DecimalId>, CoreError> {
    if count == 0 {
        return Ok(vec![]);
    }
    let count = u64::try_from(count)
        .map_err(|_| CoreError::new("id-exhausted", "job count exceeds identifier domain"))?;
    let first = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
            id.checked_add(count)
        })
        .map_err(|_| CoreError::new("id-exhausted", "job ID exhausted"))?;
    (0..count)
        .map(|offset| DecimalId::new(first + offset))
        .collect()
}
impl Drop for WorkerDispatcher {
    fn drop(&mut self) {
        self.sender.wake.close();
        let ids: Vec<_> = self.jobs.keys().copied().collect();
        for id in ids {
            self.cancel(id, Duration::ZERO);
        }
    }
}
fn resolve(
    invocation: &OwnedInvocation,
    outcome: Result<ErrorAwareExecution, CoreError>,
) -> NodeExecution {
    let mut result = runtime::resolve_outcome(
        &invocation.plan.nodes[invocation.index],
        invocation.context(),
        &invocation.inputs,
        &invocation.input_recoveries,
        invocation.input_errors.as_ref(),
        outcome,
    );
    result.input_errors = invocation.input_errors.clone();
    result
}
#[derive(Clone)]
pub struct CompletionPermit {
    pub(crate) state: Arc<PermitState>,
}
impl fmt::Debug for CompletionPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPermit")
            .field("job", &self.state.stamp)
            .finish_non_exhaustive()
    }
}
impl CompletionPermit {
    /// Internal native-Host bridge for attaching an already-running Worker.
    /// The returned pin shares the dispatcher's charge through physical cleanup.
    #[doc(hidden)]
    pub fn pin_physical_reservation(
        &self,
        host: &ExecutionHost,
    ) -> Result<ResourceLease, CoreError> {
        let slot = self.state.lease.lock().unwrap_or_else(|p| p.into_inner());
        slot.as_ref()
            .filter(|lease| {
                host.owns_lease(lease)
                    && lease.usage()
                        == (ResourceUsage {
                            physical_jobs: 1,
                            ..ResourceUsage::default()
                        })
                    && !self.state.finished.load(Ordering::Acquire)
            })
            .cloned()
            .ok_or_else(|| {
                CoreError::new(
                    "invalid-reservation",
                    "execution has no live physical reservation",
                )
            })
    }
    pub fn key(&self) -> &RunKey {
        &self.state.stamp.key
    }
    pub fn job_id(&self) -> DecimalId {
        self.state.stamp.job_id
    }
    pub fn complete(&self, result: Result<ErrorAwareExecution, CoreError>) -> bool {
        self.send(WorkerOutcome::Deferred(result))
    }
    /// A receipt-of-cancel ACK is insufficient; call only after actual termination.
    pub fn finish_cancelled(&self) -> bool {
        if !self.state.host_cancelled.load(Ordering::Acquire) {
            return false;
        }
        self.send(WorkerOutcome::Terminated)
    }
    fn send(&self, outcome: WorkerOutcome) -> bool {
        if self.state.finished.swap(true, Ordering::AcqRel) {
            return false;
        }
        let event = WorkerEvent {
            stamp: self.state.stamp.clone(),
            authority: Arc::clone(&self.state.authority),
            outcome,
        };
        // The dispatcher retains its own lease until authenticated consumption.
        // Release producer ownership before waking it, so reclaimed capacity is
        // already visible when the owner considers its next FIFO handoff.
        self.state.release_owned();
        self.state.sender.send(event).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        builtins::register_demo_nodes,
        execution_control::{ResourcePolicy, RunLedger, SessionEpoch, StartRequest},
        graph::{ExecutionPlan, Graph},
        node::Registry,
    };
    use serde_json::{Value, json};
    struct PrestartCounter(Arc<AtomicU64>);
    fn prestart_definition() -> crate::node::NodeDefinition {
        crate::node::NodeDefinition::new(
            "fixture.prestart",
            1,
            vec![],
            vec![crate::node::Port::packet("out")],
        )
    }
    impl crate::node::Node for PrestartCounter {
        fn definition(&self) -> crate::node::NodeDefinition {
            prestart_definition()
        }
        fn execute(
            &self,
            _: crate::node::RunContext<'_>,
            _: &crate::node::Inputs,
            _: &Value,
        ) -> Result<crate::node::Outputs, CoreError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok([("out".into(), crate::node::OutputValue::NoOutput)].into())
        }
    }
    impl crate::execution_control::DeferredNode for PrestartCounter {
        fn definition(&self) -> crate::node::NodeDefinition {
            prestart_definition()
        }
        fn start(
            &self,
            _: OwnedInvocation,
            permit: CompletionPermit,
            _: CancellationToken,
        ) -> Result<(), CoreError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            permit.complete(Ok(ErrorAwareExecution::new(
                [("out".into(), crate::error_input::ErrorAwareValue::NoOutput)].into(),
            )));
            Ok(())
        }
    }
    #[test]
    fn formal_prestart_fixture_skips_both_sync_and_deferred_callbacks() {
        use crate::execution_control::PhysicalCounts;
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let ids: Vec<String> = serde_json::from_str(include_str!(
            "../../../../tests/execution_prestart_cases.json"
        ))
        .unwrap();
        assert_eq!(ids, ["C05-cancel-before-worker-start"]);
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == ids[0])
            .unwrap();
        assert_eq!(case["initial"]["deferWorkerStart"], true);
        assert_eq!(case["events"].as_array().unwrap().len(), 2);
        for kind in ["sync", "deferred"] {
            let initial = &case["initial"];
            let start = &case["events"][0];
            let cancel = &case["events"][1];
            assert_eq!(start["op"], "start");
            assert_eq!(cancel["op"], "cancel");
            assert_eq!(start["atMs"], 0);
            assert_eq!(cancel["atMs"], 0);
            let host = ExecutionHost::new(
                format!("formal-prestart-{kind}"),
                ResourcePolicy::from_value(
                    fixture["resourceProfiles"][initial["resourceProfile"].as_str().unwrap()]
                        .clone(),
                )
                .unwrap(),
            )
            .unwrap();
            let calls = Arc::new(AtomicU64::new(0));
            let mut registry = Registry::new();
            if kind == "sync" {
                registry
                    .register(PrestartCounter(Arc::clone(&calls)))
                    .unwrap();
            } else {
                registry
                    .register_deferred(PrestartCounter(Arc::clone(&calls)))
                    .unwrap();
            }
            let node = fixture["workloads"]["single"]["jobs"][0].as_str().unwrap();
            let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":node,"kind":"fixture.prestart","version":1}],"edges":[]}).to_string()).unwrap();
            let version: DecimalId = initial["planVersion"].as_str().unwrap().parse().unwrap();
            let mut ledger = RunLedger::new(
                host.clone(),
                SessionEpoch::new(initial["sessionEpoch"].as_str().unwrap()).unwrap(),
                ExecutionPlan::compile(&graph, &registry).unwrap(),
                version,
            );
            ledger
                .invalidate_input(version, initial["latestInputRevision"].as_u64().unwrap())
                .unwrap();
            let targets: Vec<_> = fixture["workloads"]["single"]["targets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|target| {
                    let (node, port) = target.as_str().unwrap().split_once('.').unwrap();
                    json!({"node":node,"port":port})
                })
                .collect();
            let request:StartRequest=serde_json::from_value(json!({"startKey":start["startKey"],"expectedPlanVersion":initial["planVersion"],"targets":targets,"input":start["input"],"runBudgetMs":start["runBudgetMs"],"jobBudgets":start["jobBudgets"]})).unwrap();
            ledger.start_trace().unwrap();
            let key = ledger.start_run(request).unwrap().key;
            ledger.mark_running(&key).unwrap();
            let inv = ledger
                .owned_invocation(&key, node, BTreeMap::new())
                .unwrap();
            let mut dispatcher = WorkerDispatcher::new(host.clone());
            let (release, gate) = mpsc::channel();
            dispatcher.start_gate = Some(Arc::new(Mutex::new(gate)));
            let job = dispatcher.submit(inv).unwrap();
            let producer = dispatcher.jobs[&job].permit.clone();
            ledger
                .update_physical(
                    &key,
                    PhysicalCounts {
                        running: 1,
                        ..Default::default()
                    },
                )
                .unwrap();
            ledger.cancel_run(&key).unwrap();
            assert!(dispatcher.cancel(job, Duration::ZERO));
            assert_eq!(host.usage().physical_jobs, 1);
            assert_eq!(calls.load(Ordering::Acquire), 0);
            // Dropping this sender also releases the test gate if an assertion
            // fails; the test never leaves its own worker blocked forever.
            release.send(()).unwrap();
            drop(release);
            let ended = dispatcher
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert_eq!(host.usage().physical_jobs, 1);
            if let Some(state) = producer {
                assert!(state.finished.load(Ordering::Acquire));
                assert!(state.lease.lock().unwrap().is_none());
                assert!(state.invocation.lock().unwrap().is_none());
            }
            let ended = dispatcher.consume(ended).unwrap();
            assert!(ended.physically_finished);
            assert!(ended.execution.is_none());
            ledger
                .update_physical(&key, PhysicalCounts::default())
                .unwrap();
            assert!(
                !ledger
                    .read_trace(0, 256)
                    .unwrap()
                    .events
                    .iter()
                    .any(|e| e.event == super::super::TraceKind::NodeStarted)
            );
            let snapshot = serde_json::to_value(ledger.get_run(&key).unwrap()).unwrap();
            for (field, want) in case["expected"].as_object().unwrap() {
                match field.as_str() {
                    "r1" => {
                        for (field, want) in want.as_object().unwrap() {
                            match field.as_str() {
                                "state" => assert_eq!(&snapshot["state"], want),
                                "delivery" | "reason" => {
                                    assert_eq!(&snapshot["result"][field], want)
                                }
                                other => panic!("unsupported prestart run expectation {other}"),
                            }
                        }
                    }
                    "physicalJobs" => assert_eq!(json!(host.usage().physical_jobs), *want),
                    "queuedJobs" => assert_eq!(json!(host.usage().queued_jobs), *want),
                    "startedJobs" => assert_eq!(json!(calls.load(Ordering::Acquire)), *want),
                    other => panic!("unsupported prestart expectation {other}"),
                }
            }
        }
    }
    struct HeldFixtureProducer(std::sync::mpsc::Sender<CompletionPermit>);

    #[test]
    fn prepared_worker_pin_shares_only_a_live_reservation_from_the_same_host() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let policy =
            ResourcePolicy::from_value(fixture["resourceProfiles"]["bounded"].clone()).unwrap();
        let host = ExecutionHost::new("prepared-pin-test", policy.clone()).unwrap();
        let foreign = ExecutionHost::new("prepared-pin-foreign", policy).unwrap();
        let dispatcher = WorkerDispatcher::new(host.clone());
        let key: RunKey = serde_json::from_value(json!({
            "sessionEpoch":"11111111111111111111111111111111",
            "planVersion":"1", "runId":"1", "deliveryGeneration":"1"
        }))
        .unwrap();
        let lease = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..Default::default()
            })
            .unwrap();
        let permit = CompletionPermit {
            state: Arc::new(PermitState {
                stamp: JobStamp {
                    key,
                    job_id: issue_job_id().unwrap(),
                },
                authority: Arc::new(()),
                sender: dispatcher.sender.clone(),
                finished: AtomicBool::new(false),
                host_cancelled: AtomicBool::new(false),
                lease: Mutex::new(Some(lease)),
                invocation: Mutex::new(None),
            }),
        };
        assert_eq!(
            permit.pin_physical_reservation(&foreign).unwrap_err().code,
            "invalid-reservation"
        );
        let pin = permit.pin_physical_reservation(&host).unwrap();
        assert_eq!(host.usage().physical_jobs, 1);
        assert!(permit.complete(Err(CoreError::new("fixture", "test completion"))));
        assert_eq!(
            permit.pin_physical_reservation(&host).unwrap_err().code,
            "invalid-reservation"
        );
        assert_eq!(host.usage().physical_jobs, 1);
        drop(pin);
        assert_eq!(host.usage().physical_jobs, 0);
    }

    impl crate::execution_control::DeferredNode for HeldFixtureProducer {
        fn definition(&self) -> crate::node::NodeDefinition {
            crate::node::NodeDefinition::new(
                "fixture.private-held",
                1,
                vec![],
                vec![crate::node::Port::packet("out")],
            )
        }
        fn start(
            &self,
            _: OwnedInvocation,
            permit: CompletionPermit,
            _: CancellationToken,
        ) -> Result<(), CoreError> {
            self.0.send(permit).map_err(|_| {
                CoreError::new("test-producer-dropped", "private fixture producer dropped")
            })
        }
    }
    #[test]
    fn formal_private_fixture_cases() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let ids: Vec<String> = serde_json::from_str(include_str!(
            "../../../../tests/execution_private_cases.json"
        ))
        .unwrap();
        assert_eq!(ids.len(), 6);
        for name in ids {
            eprintln!("private fixture: {name}");
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == name)
                .unwrap();
            run_private_fixture(case, &fixture);
        }
    }
    fn run_private_fixture(case: &Value, fixture: &Value) {
        use crate::execution_control::{PhysicalCounts, RunKey};
        let initial = &case["initial"];
        let events = case["events"].as_array().unwrap();
        let start = &events[0];
        assert_eq!(start["op"], "start");
        assert_eq!(start["atMs"], 0);
        assert_eq!(initial["workload"], "single");
        let spec = &fixture["workloads"]["single"];
        let node = spec["jobs"][0].as_str().unwrap();
        let forbidden_run = events.iter().find_map(|event| {
            event["keyOverride"]["runId"]
                .as_str()
                .map(|id| id.parse::<u64>().unwrap())
        });
        // Opaque IDs may eventually equal a fixture's chosen wrong wire value.
        // Place a fresh isolated fixture before dispatch if that collision occurs.
        let (host,mut ledger,key,held)=(0..2).find_map(|_| {
            let policy=ResourcePolicy::from_value(fixture["resourceProfiles"][initial["resourceProfile"].as_str().unwrap()].clone()).unwrap();
            let host=ExecutionHost::new(format!("formal-private-{}",case["id"].as_str().unwrap()),policy).unwrap();
            let (sender,held)=std::sync::mpsc::channel();
            let mut registry=Registry::new();registry.register_deferred(HeldFixtureProducer(sender)).unwrap();
            let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":node,"kind":"fixture.private-held","version":1}],"edges":[]}).to_string()).unwrap();
            let version:DecimalId=initial["planVersion"].as_str().unwrap().parse().unwrap();
            let mut ledger=RunLedger::new(host.clone(),SessionEpoch::new(initial["sessionEpoch"].as_str().unwrap()).unwrap(),ExecutionPlan::compile(&graph,&registry).unwrap(),version);
            ledger.invalidate_input(version,initial["latestInputRevision"].as_u64().unwrap()).unwrap();
            let targets:Vec<_>=spec["targets"].as_array().unwrap().iter().map(|target|{let (node,port)=target.as_str().unwrap().split_once('.').unwrap();json!({"node":node,"port":port})}).collect();
            let request:StartRequest=serde_json::from_value(json!({"startKey":start["startKey"],"expectedPlanVersion":initial["planVersion"],"targets":targets,"input":start["input"],"runBudgetMs":start["runBudgetMs"],"jobBudgets":start["jobBudgets"]})).unwrap();
            let key=ledger.start_run(request).unwrap().key;
            if Some(key.run_id.get())==forbidden_run {None}else{Some((host,ledger,key,held))}
        }).expect("could not place noncolliding opaque run ID");
        ledger.mark_running(&key).unwrap();
        let invocation = ledger
            .owned_invocation(&key, node, BTreeMap::new())
            .unwrap();
        let mut dispatcher = WorkerDispatcher::new(host.clone());
        let forbidden_job = events.iter().find_map(|event| {
            event["keyOverride"]["jobId"]
                .as_str()
                .map(|id| id.parse::<u64>().unwrap())
        });
        let mut job = issue_job_id().unwrap();
        if Some(job.get()) == forbidden_job {
            job = issue_job_id().unwrap();
        }
        let lease = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..Default::default()
            })
            .unwrap();
        dispatcher
            .submit_reserved(invocation.clone(), job, lease)
            .unwrap();
        let permit = held.recv_timeout(Duration::from_secs(5)).unwrap();
        let returned = dispatcher
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(dispatcher.consume(returned).is_none());
        assert!(dispatcher.jobs[&job].start_returned);
        assert!(!dispatcher.jobs[&job].finished);
        ledger
            .update_physical(
                &key,
                PhysicalCounts {
                    running: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut adopted = false;
        let mut reclaimed = 0;
        let mut notices = vec![];
        for event in events.iter().skip(1) {
            let at = Duration::from_millis(event["atMs"].as_u64().unwrap());
            ledger.advance_clock(at).unwrap();
            dispatcher.mark_stalled(at, host.policy().cancel_grace_ms);
            match event["op"].as_str().unwrap() {
                "cancel" => {
                    ledger.cancel_run(&key).unwrap();
                    assert!(dispatcher.cancel(job, at));
                }
                "complete" => {
                    let correct = dispatcher.jobs[&job].stamp.clone();
                    let mut stamp = correct.clone();
                    if let Some(epoch) = event["sessionEpoch"].as_str() {
                        stamp.key.session_epoch = SessionEpoch::new(epoch).unwrap();
                    }
                    if let Some(overrides) = event["keyOverride"].as_object() {
                        let mut wire = serde_json::to_value(&stamp.key).unwrap();
                        for (field, value) in overrides {
                            if field == "jobId" {
                                stamp.job_id = value.as_str().unwrap().parse().unwrap();
                            } else {
                                assert!(matches!(
                                    field.as_str(),
                                    "planVersion"
                                        | "runId"
                                        | "inputRevision"
                                        | "deliveryGeneration"
                                ));
                                wire[field] = value.clone();
                            }
                        }
                        stamp.key = serde_json::from_value::<RunKey>(wire).unwrap();
                    }
                    assert!(stamp.key != correct.key || stamp.job_id != correct.job_id);
                    assert_eq!(event["value"]["status"], "data");
                    let packet = Arc::new(
                        serde_json::from_value::<crate::Packet>(event["value"]["packet"].clone())
                            .unwrap(),
                    );
                    let execution = ErrorAwareExecution::new(
                        [(
                            "out".into(),
                            crate::error_input::ErrorAwareValue::Data(packet),
                        )]
                        .into(),
                    );
                    let before = host.usage().physical_jobs;
                    let result = dispatcher.consume(WorkerEvent {
                        stamp,
                        authority: Arc::clone(&dispatcher.jobs[&job].authority),
                        outcome: WorkerOutcome::Deferred(Ok(execution)),
                    });
                    adopted |= result
                        .as_ref()
                        .is_some_and(|event| event.execution.is_some());
                    reclaimed += before - host.usage().physical_jobs;
                    if result.is_none() {
                        notices.push(json!({"atMs":ledger.now().as_millis() as u64,"code":"ignored-completion"}));
                    }
                }
                other => panic!("unsupported private fixture operation {other}"),
            }
            let mut physical = PhysicalCounts::default();
            for (owned, state) in dispatcher.states() {
                if owned == &key {
                    match state {
                        WorkerJobState::Running => physical.running += 1,
                        WorkerJobState::CancelRequested => physical.cancel_requested += 1,
                        WorkerJobState::Stalled => physical.stalled += 1,
                    }
                }
            }
            if ledger.get_run(&key).unwrap().physical != physical {
                ledger.update_physical(&key, physical).unwrap();
            }
        }
        let snapshot = serde_json::to_value(ledger.get_run(&key).unwrap()).unwrap();
        for (field, want) in case["expected"].as_object().unwrap() {
            match field.as_str() {
                "r1" => {
                    for (field, want) in want.as_object().unwrap() {
                        match field.as_str() {
                            "state" => assert_eq!(&snapshot["state"], want),
                            "reason" | "delivery" => assert_eq!(&snapshot["result"][field], want),
                            other => panic!("unsupported private run expectation {other}"),
                        }
                    }
                }
                "physicalJobs" => assert_eq!(json!(host.usage().physical_jobs), *want),
                "completionAdopted" => assert_eq!(json!(adopted), *want),
                "reclaimedJobs" => assert_eq!(json!(reclaimed), *want),
                "notices" => assert_eq!(json!(notices), *want),
                other => panic!("unsupported private expectation {other}"),
            }
        }
        // The producer is genuinely still pending. End it only after the forged
        // event checks, using its original private permit and cancellation path.
        ledger.cancel_run(&key).unwrap();
        dispatcher.cancel(job, ledger.now());
        assert!(permit.finish_cancelled());
        let actual = dispatcher
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(dispatcher.consume(actual).unwrap().physically_finished);
        assert_eq!(host.usage().physical_jobs, 0);
        assert!(dispatcher.jobs.is_empty());
        ledger
            .update_physical(&key, PhysicalCounts::default())
            .unwrap();
        assert_eq!(
            ledger.get_run(&key).unwrap().physical,
            PhysicalCounts::default()
        );
    }
    #[test]
    fn batch_id_exhaustion_is_rejected_before_any_partial_issuance() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(
            allocate_job_ids(&counter, 2).unwrap_err().code,
            "id-exhausted"
        );
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX - 1);
        assert_eq!(
            allocate_job_ids(&counter, 1).unwrap()[0].get(),
            u64::MAX - 1
        );
        assert_eq!(
            allocate_job_ids(&counter, 1).unwrap_err().code,
            "id-exhausted"
        );
        assert!(allocate_job_ids(&counter, 0).unwrap().is_empty());
    }

    #[test]
    fn forged_or_old_events_cannot_resolve_or_reclaim_another_job() {
        let profile = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!(
                "../../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        let host = ExecutionHost::new("worker-private-authority-test", profile).unwrap();
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"x"}}}}],"edges":[]}).to_string()).unwrap();
        let mut ledger = RunLedger::new(
            host.clone(),
            SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            DecimalId::new(1).unwrap(),
        );
        let key=ledger.start_run(serde_json::from_value::<StartRequest>(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}]})).unwrap()).unwrap().key;
        let inv = ledger
            .owned_invocation(&key, "source", BTreeMap::new())
            .unwrap();
        let mut d = WorkerDispatcher::new(host.clone());
        let a = d.submit(inv.clone()).unwrap();
        let b = d.submit(inv).unwrap();
        let stamp = d.jobs[&a].stamp.clone();
        let authority = Arc::clone(&d.jobs[&a].authority);
        for field in 0..6 {
            let mut wrong = stamp.clone();
            match field {
                0 => {
                    wrong.key.session_epoch =
                        SessionEpoch::new("1123456789abcdef0123456789abcdef").unwrap()
                }
                1 => wrong.key.plan_version = wrong.key.plan_version.checked_next().unwrap(),
                2 => wrong.key.run_id = wrong.key.run_id.checked_next().unwrap(),
                3 => wrong.key.input_revision = Some(1),
                4 => {
                    wrong.key.delivery_generation =
                        wrong.key.delivery_generation.checked_next().unwrap()
                }
                _ => wrong.job_id = b,
            }
            assert!(
                d.consume(WorkerEvent {
                    stamp: wrong,
                    authority: Arc::clone(&authority),
                    outcome: WorkerOutcome::Terminated
                })
                .is_none()
            );
            assert_eq!(host.usage().physical_jobs, 2);
        }
        assert!(
            d.consume(WorkerEvent {
                stamp: stamp.clone(),
                authority: Arc::new(()),
                outcome: WorkerOutcome::Terminated
            })
            .is_none()
        );
        let first = d.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = d.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        let retired_stamp = first.stamp.clone();
        let retired_authority = Arc::clone(&first.authority);
        assert!(d.consume(first).unwrap().physically_finished);
        assert_eq!(host.usage().physical_jobs, 1);
        // Even the correct old capability cannot free a different, still-owned job.
        assert!(!d.jobs.contains_key(&retired_stamp.job_id));
        assert!(
            d.consume(WorkerEvent {
                stamp: retired_stamp,
                authority: retired_authority,
                outcome: WorkerOutcome::Terminated
            })
            .is_none()
        );
        assert_eq!(host.usage().physical_jobs, 1);
        assert!(d.consume(second).unwrap().physically_finished);
        assert_eq!(host.usage().physical_jobs, 0);
    }
}
