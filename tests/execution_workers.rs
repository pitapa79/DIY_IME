#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Packet,
    error_input::{ErrorAwareExecution, ErrorAwareInputs, ErrorAwareValue},
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlNode, DecimalId,
        DeferredNode, ExecutionHost, Milliseconds, OwnedInvocation, ResourcePolicy, RunLedger,
        SessionEpoch, StartRequest, WorkerCompletion, WorkerDispatcher, WorkerJobState,
    },
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, Outputs, Port, Registry, RunContext,
    },
    runtime::{Engine, NodeStatus, ResolvedValue},
    session::{Request, Session},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use test_nodes::register_demo_nodes;

fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn host() -> ExecutionHost {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut p =
        ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
    p.max_physical_jobs = 1;
    ExecutionHost::new(
        format!("worker-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        p,
    )
    .unwrap()
}
fn graph(kind: &str) -> Graph {
    Graph::from_json(
        &json!({"version":1,"nodes":[{"id":"source","kind":kind,"version":1}],"edges":[]})
            .to_string(),
    )
    .unwrap()
}
fn ledger(host: ExecutionHost, registry: &Registry, kind: &str) -> RunLedger {
    RunLedger::new(
        host,
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        ExecutionPlan::compile(&graph(kind), registry).unwrap(),
        id(1),
    )
}
fn invocation(ledger: &mut RunLedger) -> OwnedInvocation {
    let receipt=ledger.start_run(serde_json::from_value::<StartRequest>(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],"input":{"revision":1,"text":"かな😀","mode":"compose"}})).unwrap()).unwrap();
    ledger
        .owned_invocation(&receipt.key, "source", ErrorAwareInputs::new())
        .unwrap()
}
fn output() -> ErrorAwareExecution {
    ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(
                Packet::from_json(r#"{"version":1,"root":{"type":"text","text":"ok"}}"#).unwrap(),
            )),
        )]
        .into(),
    )
}
fn wait_for(
    dispatcher: &mut WorkerDispatcher,
    mut check: impl FnMut(&[WorkerCompletion]) -> bool,
) -> Vec<WorkerCompletion> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut all = vec![];
    loop {
        all.extend(dispatcher.poll());
        if check(&all) {
            return all;
        }
        assert!(Instant::now() < deadline, "worker event timeout");
        thread::yield_now();
    }
}
struct Deferred {
    send: mpsc::Sender<(OwnedInvocation, CompletionPermit, CancellationToken)>,
    start_gate: Option<Mutex<mpsc::Receiver<()>>>,
    fail: bool,
    panic: bool,
}
impl DeferredNode for Deferred {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.deferred", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        inv: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.send.send((inv, permit, cancel)).unwrap();
        if let Some(gate) = &self.start_gate {
            gate.lock().unwrap().recv().unwrap();
        }
        assert!(!self.panic, "test panic");
        if self.fail {
            Err(CoreError::new("start-failed", "test start failure"))
        } else {
            Ok(())
        }
    }
}
fn deferred(
    gated: bool,
    fail: bool,
    panic: bool,
) -> (
    Registry,
    mpsc::Receiver<(OwnedInvocation, CompletionPermit, CancellationToken)>,
    mpsc::Sender<()>,
) {
    let (tx, rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel();
    let mut registry = Registry::new();
    registry
        .register_deferred(Deferred {
            send: tx,
            start_gate: gated.then(|| Mutex::new(gate_rx)),
            fail,
            panic,
        })
        .unwrap();
    (registry, rx, gate_tx)
}

#[test]
fn deferred_completion_before_start_returns_keeps_physical_slot_until_both_end() {
    let (host, (registry, rx, release)) = (host(), deferred(true, false, false));
    let mut ledger = ledger(host.clone(), &registry, "test.deferred");
    let inv = invocation(&mut ledger);
    let mut d = WorkerDispatcher::new(host.clone());
    let job = d.submit(inv.clone()).unwrap();
    let (owned, permit, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(owned.context().input.unwrap().text, "かな😀");
    assert_eq!(permit.key(), inv.key());
    assert_eq!(permit.job_id(), job);
    assert!(permit.complete(Ok(output())));
    assert!(!permit.complete(Ok(output())));
    let events = wait_for(&mut d, |events| {
        events.iter().any(|event| event.execution().is_some())
    });
    assert!(!events[0].physically_finished());
    assert_eq!(host.usage().physical_jobs, 1);
    assert_eq!(d.submit(inv).err().unwrap().code, "busy");
    release.send(()).unwrap();
    let events = wait_for(&mut d, |events| {
        events.iter().any(|event| event.physically_finished())
    });
    assert!(events.iter().all(|event| event.execution().is_none()));
    assert_eq!(host.usage().physical_jobs, 0);
    assert!(!permit.finish_cancelled());
}

#[test]
fn cancelled_stalled_work_keeps_slot_and_late_result_only_reclaims_it() {
    let host = host();
    let (registry, rx, _) = deferred(false, false, false);
    let mut ledger = ledger(host.clone(), &registry, "test.deferred");
    let inv = invocation(&mut ledger);
    let mut d = WorkerDispatcher::new(host.clone());
    let job = d.submit(inv.clone()).unwrap();
    let (_, permit, cancel) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!permit.finish_cancelled()); // A node cannot manufacture host cancellation.
    cancel.cancel();
    assert!(!permit.finish_cancelled()); // Even self-cancelling the public token is insufficient.
    ledger.cancel_run(inv.key()).unwrap();
    assert!(d.cancel(job, Duration::from_millis(2)));
    assert!(cancel.is_cancelled());
    d.mark_stalled(Duration::from_millis(12), Milliseconds::new(10).unwrap());
    assert_eq!(d.states().next().unwrap().1, WorkerJobState::Stalled);
    assert_eq!(host.usage().physical_jobs, 1);
    assert!(permit.complete(Ok(output())));
    let events = wait_for(&mut d, |e| e.iter().any(|e| e.physically_finished()));
    assert!(events.iter().all(|e| e.execution().is_none()));
    assert_eq!(host.usage().physical_jobs, 0);
    assert_eq!(
        ledger.get_run(inv.key()).unwrap().result.unwrap().delivery,
        diy_ime2_core::execution_control::RunDelivery::Cancelled
    );
    assert!(!permit.complete(Ok(output())));
    assert_eq!(host.usage().physical_jobs, 0);
}

#[test]
fn dispatcher_drop_requests_cancel_but_live_permit_preserves_domain_reservation() {
    let host = host();
    let domain = host.resource_domain().to_owned();
    let policy = host.policy().clone();
    let (registry, rx, _) = deferred(false, false, false);
    let mut ledger = ledger(host.clone(), &registry, "test.deferred");
    let inv = invocation(&mut ledger);
    let mut d = WorkerDispatcher::new(host.clone());
    d.submit(inv).unwrap();
    let (owned, permit, cancel) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(owned);
    drop(d);
    drop(ledger);
    drop(registry);
    drop(host);
    assert!(cancel.is_cancelled());
    assert!(ExecutionHost::new(&domain, policy.clone()).is_err());
    // False means the receiver is gone; physical ownership still ends exactly once.
    assert!(!permit.finish_cancelled());
    drop(permit);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if ExecutionHost::new(&domain, policy.clone()).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::yield_now();
    }
}

#[test]
fn start_failure_and_panic_are_single_logical_errors_without_unproven_reclaim() {
    for panic in [false, true] {
        let host = host();
        let (registry, rx, _) = deferred(false, !panic, panic);
        let mut ledger = ledger(host.clone(), &registry, "test.deferred");
        let inv = invocation(&mut ledger);
        let mut d = WorkerDispatcher::new(host.clone());
        let job = d.submit(inv).unwrap();
        let (_, permit, cancel) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let events = wait_for(&mut d, |e| e.iter().any(|e| e.execution().is_some()));
        let execution = events.into_iter().find_map(|e| e.into_execution()).unwrap();
        assert_eq!(execution.status, NodeStatus::Failed);
        assert_eq!(
            execution.diagnostic.unwrap().error.code,
            if panic { "node-panic" } else { "start-failed" }
        );
        assert_eq!(host.usage().physical_jobs, 1);
        assert!(d.cancel(job, Duration::ZERO));
        assert!(cancel.is_cancelled());
        assert_eq!(host.usage().physical_jobs, 1);
        assert!(permit.complete(Ok(output())));
        let events = wait_for(&mut d, |e| e.iter().any(|e| e.physically_finished()));
        assert!(events.iter().all(|e| e.execution().is_none()));
        assert_eq!(host.usage().physical_jobs, 0);
    }
}

#[test]
fn deferred_outputs_use_the_same_packet_and_declared_port_validation_as_sync_nodes() {
    let host = host();
    let (registry, rx, _) = deferred(false, false, false);
    let mut ledger = ledger(host.clone(), &registry, "test.deferred");
    let inv = invocation(&mut ledger);
    let mut d = WorkerDispatcher::new(host);
    d.submit(inv).unwrap();
    let (_, permit, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(permit.complete(Ok(ErrorAwareExecution::default())));
    let events = wait_for(&mut d, |e| e.iter().any(|e| e.physically_finished()));
    let execution = events.into_iter().find_map(|e| e.into_execution()).unwrap();
    assert_eq!(execution.diagnostic.unwrap().error.code, "missing-output");
}

struct SyncGate {
    entered: mpsc::Sender<String>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Node for SyncGate {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.sync", 1, vec![], vec![Port::packet("out")])
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        _: &Inputs,
        _: &Value,
    ) -> Result<Outputs, CoreError> {
        self.entered
            .send(context.input.unwrap().text.clone())
            .unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok([("out".into(), diy_ime2_core::node::OutputValue::NoOutput)].into())
    }
}
#[test]
fn real_sync_worker_does_not_block_cancel_or_clock_and_keeps_owned_plan() {
    let host = host();
    let (entered_tx, entered) = mpsc::channel();
    let (release_tx, release) = mpsc::channel();
    let mut registry = Registry::new();
    registry
        .register(SyncGate {
            entered: entered_tx,
            release: Mutex::new(release),
        })
        .unwrap();
    let mut ledger = ledger(host.clone(), &registry, "test.sync");
    let inv = invocation(&mut ledger);
    let plan = ledger.plan_snapshot(inv.key()).unwrap();
    let weak = Arc::downgrade(&plan);
    drop(plan);
    let key = inv.key().clone();
    let mut d = WorkerDispatcher::new(host.clone());
    let job = d.submit(inv).unwrap();
    assert_eq!(
        entered.recv_timeout(Duration::from_secs(5)).unwrap(),
        "かな😀"
    );
    ledger.advance_clock(Duration::from_millis(20)).unwrap();
    assert_eq!(ledger.cancel_run(&key).unwrap().elapsed_ms, 20);
    d.cancel(job, Duration::from_millis(20));
    drop(ledger);
    drop(registry);
    assert!(weak.upgrade().is_some());
    assert_eq!(host.usage().physical_jobs, 1);
    release_tx.send(()).unwrap();
    let events = wait_for(&mut d, |e| e.iter().any(|e| e.physically_finished()));
    assert!(events.iter().all(|e| e.execution().is_none()));
    assert!(weak.upgrade().is_none());
    assert_eq!(host.usage().physical_jobs, 0);
}

struct Control;
impl ControlNode for Control {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.control", 1, vec![], vec![Port::packet("out")])
    }
    fn observe(
        &self,
        _: RunContext<'_>,
        _: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        Ok(ControlDecision::Wait {
            wake_after_ms: None,
        })
    }
}
#[test]
fn execution_declarations_are_opt_in_and_legacy_engine_and_session_reject_new_kinds() {
    let (mut registry, _, _) = deferred(false, false, false);
    registry.register_control(Control).unwrap();
    register_demo_nodes(&mut registry).unwrap();
    let defs = registry
        .execution_definitions()
        .map(|d| serde_json::to_value(d).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        defs.iter()
            .find(|d| d["definition"]["kind"] == "test.deferred")
            .unwrap()["executionKind"],
        "deferred"
    );
    assert!(defs.iter().all(|d| d["version"] == 2));
    assert!(!registry.definitions().any(|d| d.kind.starts_with("test.")));
    assert_eq!(registry.error_aware_definitions().count(), 0);
    for kind in ["test.control", "test.deferred"] {
        let mut engine = Engine::new(ExecutionPlan::compile(&graph(kind), &registry).unwrap());
        assert_eq!(engine.run().err().unwrap().code, "unsupported-capability");
    }
    let mut session = Session::new(registry);
    let request: Request =
        serde_json::from_value(json!({"method":"prepare","graph":graph("test.deferred")})).unwrap();
    assert_eq!(
        session.handle(request).err().unwrap().code,
        "unsupported-capability"
    );
}

#[test]
fn existing_demo_node_executes_on_a_real_worker() {
    let host = host();
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"既存😀"}}}}],"edges":[]}).to_string()).unwrap();
    let mut ledger = RunLedger::new(
        host.clone(),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        ExecutionPlan::compile(&graph, &registry).unwrap(),
        id(1),
    );
    let inv = invocation(&mut ledger);
    let mut d = WorkerDispatcher::new(host);
    d.submit(inv).unwrap();
    let events = wait_for(&mut d, |e| e.iter().any(|e| e.physically_finished()));
    let execution = events.into_iter().find_map(|e| e.into_execution()).unwrap();
    assert_eq!(execution.status, NodeStatus::Executed);
    let ResolvedValue::Data(packet) = &execution.outputs["out"] else {
        panic!("expected packet")
    };
    assert_eq!(
        serde_json::to_value(&packet.root).unwrap()["text"],
        "既存😀"
    );
}

struct InvalidDeferred;
impl DeferredNode for InvalidDeferred {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.invalid",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec!["missing".into()]
    }
    fn start(
        &self,
        _: OwnedInvocation,
        _: CompletionPermit,
        _: CancellationToken,
    ) -> Result<(), CoreError> {
        unreachable!()
    }
}
#[test]
fn invalid_error_input_declarations_never_enter_registry() {
    let mut registry = Registry::new();
    assert_eq!(
        registry
            .register_deferred(InvalidDeferred)
            .unwrap_err()
            .code,
        "invalid-definition"
    );
    assert_eq!(registry.execution_definitions().count(), 0);
}
