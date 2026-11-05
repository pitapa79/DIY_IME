#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue, RecoveryClaim},
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlInput,
        ControlNode, DecimalId, DeferredNode, ExecutionHost, ExecutionPump, Milliseconds,
        OwnedInvocation, ResourcePolicy, RunDelivery, RunKey, RunPhase, SessionEpoch, StartRequest,
        TerminalReason, WakeReason,
    },
    graph::{ExecutionPlan, Graph},
    node::{CoreError, InputPort, NodeDefinition, Port, Registry, RunContext},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn epoch(n: u64) -> SessionEpoch {
    SessionEpoch::new(format!("{n:032x}")).unwrap()
}
fn node(name: &str, kind: &str) -> Value {
    json!({"id":name,"kind":kind,"version":1})
}
fn control(name: &str, mode: &str) -> Value {
    json!({"id":name,"kind":"test.control","version":1,"config":{"mode":mode}})
}
fn edge(a: &str, b: &str, port: &str) -> Value {
    json!({"from":{"node":a,"port":"out"},"to":{"node":b,"port":port}})
}
fn data(text: &str) -> ErrorAwareExecution {
    ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: text.into(),
                meta: None,
            }))),
        )]
        .into(),
    )
}
#[derive(Clone)]
struct Started {
    name: String,
    permit: CompletionPermit,
    cancel: CancellationToken,
}
struct Deferred(mpsc::Sender<Started>);
impl DeferredNode for Deferred {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.delay",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn start(
        &self,
        inv: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.0
            .send(Started {
                name: inv.context().node_id.into(),
                permit,
                cancel,
            })
            .unwrap();
        Ok(())
    }
}
#[derive(Debug, Clone)]
struct Observation {
    node: String,
    reason: WakeReason,
    elapsed: u64,
    seq: DecimalId,
}
struct Policy(Arc<Mutex<Vec<Observation>>>);
impl ControlNode for Policy {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.control",
            1,
            vec![
                InputPort::optional(Port::packet("a")),
                InputPort::optional(Port::packet("b")),
            ],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec!["a".into(), "b".into()]
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if config.is_object() {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "test object"))
        }
    }
    fn observe(
        &self,
        ctx: RunContext<'_>,
        frame: &ControlFrame,
        config: &Value,
    ) -> Result<ControlDecision, CoreError> {
        self.0.lock().unwrap().push(Observation {
            node: ctx.node_id.into(),
            reason: frame.reason,
            elapsed: frame.elapsed_ms,
            seq: frame.event_seq,
        });
        let mode = config["mode"].as_str().unwrap_or("first");
        if mode == "wait" {
            return Ok(ControlDecision::Wait {
                wake_after_ms: None,
            });
        }
        if mode == "repeat" && frame.elapsed_ms < 6 {
            return Ok(ControlDecision::Wait {
                wake_after_ms: Some(Milliseconds::new(2).unwrap()),
            });
        }
        if (mode == "timer" && frame.reason == WakeReason::Timer) || mode == "repeat" {
            return Ok(ControlDecision::Finish(data("fallback")));
        }
        if mode == "timer" {
            return Ok(ControlDecision::Wait {
                wake_after_ms: Some(Milliseconds::new(5).unwrap()),
            });
        }
        for (port, input) in &frame.inputs {
            if let ControlInput::Resolved(value) = input {
                if let ErrorAwareValue::Data(_) = value {
                    return Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                        [("out".into(), value.clone())].into(),
                    )));
                }
                if let ErrorAwareValue::Error(errors) = value {
                    let mut result = data("recovered");
                    result.recoveries.push(RecoveryClaim {
                        input: port.clone(),
                        output: "out".into(),
                        causes: errors
                            .causes()
                            .iter()
                            .map(|cause| cause.id.clone())
                            .collect(),
                    });
                    return Ok(ControlDecision::Finish(result));
                }
            }
        }
        Ok(ControlDecision::Wait {
            wake_after_ms: None,
        })
    }
}
struct Harness {
    host: ExecutionHost,
    pump: ExecutionPump,
    registry: Registry,
    graph: Graph,
    entered: mpsc::Receiver<Started>,
    started: Vec<Started>,
    keys: Vec<RunKey>,
    observations: Arc<Mutex<Vec<Observation>>>,
}
impl Harness {
    fn new(
        nodes: Vec<Value>,
        edges: Vec<Value>,
        slots: u64,
        queued: u64,
        queue_wait: Option<u64>,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut p = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        p.max_physical_jobs = slots;
        p.max_queued_jobs = queued;
        p.queue_wait_ms = queue_wait.map(|n| Milliseconds::new(n).unwrap());
        p.cancel_grace_ms = Milliseconds::new(3).unwrap();
        p.retention_ms = Milliseconds::new(10).unwrap();
        let host = ExecutionHost::new(
            format!("pump-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            p,
        )
        .unwrap();
        let mut pump = ExecutionPump::new(host.clone()).unwrap();
        let (send, entered) = mpsc::channel();
        let observations = Arc::new(Mutex::new(vec![]));
        let mut registry = Registry::new();
        test_nodes::register_demo_nodes(&mut registry).unwrap();
        registry.register_deferred(Deferred(send)).unwrap();
        registry
            .register_control(Policy(Arc::clone(&observations)))
            .unwrap();
        let graph = Graph::from_json(&json!({"version":1,"nodes":nodes,"edges":edges}).to_string())
            .unwrap();
        pump.open_session(
            host.clone(),
            epoch(1),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            id(1),
        )
        .unwrap();
        Self {
            host,
            pump,
            registry,
            graph,
            entered,
            started: vec![],
            keys: vec![],
            observations,
        }
    }
    fn request(start: u64, targets: &[&str]) -> StartRequest {
        serde_json::from_value(json!({"startKey":start.to_string(),"expectedPlanVersion":"1","targets":targets.iter().map(|node|json!({"node":node,"port":"out"})).collect::<Vec<_>>()})).unwrap()
    }
    fn start(&mut self, request: StartRequest) -> RunKey {
        let receipt = self.pump.start(&epoch(1), request).unwrap();
        self.keys.push(receipt.key.clone());
        receipt.key
    }
    fn enter(&mut self) -> Started {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump.advance_to(self.pump.now()).unwrap();
            if let Ok(start) = self.entered.try_recv() {
                self.started.push(start.clone());
                return start;
            }
            assert!(Instant::now() < until, "adapter did not start");
            let _ = self
                .pump
                .wake_signal()
                .wait(Some(Duration::from_millis(1)))
                .unwrap();
        }
    }
    fn advance(&mut self, n: u64) {
        self.pump.advance_to(ms(n)).unwrap();
    }
    fn calls(&self) -> Vec<Observation> {
        self.observations.lock().unwrap().clone()
    }
    fn settle(&mut self, mut done: impl FnMut(&ExecutionPump) -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump.advance_to(self.pump.now()).unwrap();
            if done(&self.pump) {
                return;
            }
            assert!(Instant::now() < until, "pump settlement timed out");
            let _ = self
                .pump
                .wake_signal()
                .wait(Some(Duration::from_millis(1)))
                .unwrap();
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        for key in &self.keys {
            let _ = self.pump.cancel(key);
        }
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.started.extend(self.entered.try_iter());
            for start in &self.started {
                start.permit.finish_cancelled();
            }
            let _ = self.pump.advance_to(self.pump.now());
            if self.host.usage().physical_jobs == 0 {
                break;
            }
            assert!(Instant::now() < until, "test worker cleanup timed out");
            std::thread::yield_now();
        }
    }
}

#[test]
fn drives_real_dependencies_once_and_only_for_requested_ancestors() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            node("b", "test.delay"),
            node("unused", "test.delay"),
        ],
        vec![edge("a", "b", "in")],
        2,
        2,
        None,
    );
    let key = h.start(Harness::request(1, &["b"]));
    let a = h.enter();
    assert_eq!(a.name, "a");
    assert!(a.permit.complete(Ok(data("a"))));
    h.settle(|pump| {
        pump.get_run(&key).unwrap().physical.queued + pump.get_run(&key).unwrap().physical.running
            > 0
    });
    h.advance(0);
    let b = h.enter();
    assert_eq!(b.name, "b");
    assert!(b.permit.complete(Ok(data("b"))));
    h.settle(|pump| pump.get_run(&key).unwrap().state == RunPhase::Terminal);
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
    assert!(h.entered.try_recv().is_err());
}
#[test]
fn first_data_observes_one_completion_before_adopting_the_next_queued_result() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            node("b", "test.delay"),
            control("choose", "first"),
        ],
        vec![edge("a", "choose", "a"), edge("b", "choose", "b")],
        2,
        2,
        None,
    );
    let key = h.start(Harness::request(1, &["choose"]));
    let x = h.enter();
    let y = h.enter();
    let (a, b) = if x.name == "a" { (x, y) } else { (y, x) };
    assert!(b.permit.complete(Ok(data("b"))));
    assert!(a.permit.complete(Ok(data("a"))));
    h.advance(0);
    let report = h.pump.get_run(&key).unwrap().result.unwrap();
    assert_eq!(report.delivery, RunDelivery::Complete);
    assert_eq!(
        report
            .ports
            .iter()
            .find(|p| p.endpoint.node == "choose")
            .unwrap()
            .value["packet"]["root"]["text"],
        "b"
    );
    assert!(a.cancel.is_cancelled());
    assert_eq!(h.calls().len(), 2);
}
#[test]
fn reads_replays_and_unchanged_frames_do_not_reobserve_or_advance_policy() {
    let mut h = Harness::new(
        vec![node("a", "test.delay"), control("wait", "wait")],
        vec![edge("a", "wait", "a")],
        1,
        2,
        None,
    );
    let request = Harness::request(1, &["wait"]);
    let key = h.start(request.clone());
    let a = h.enter();
    let seq = h.pump.get_run(&key).unwrap().event_seq;
    for _ in 0..100 {
        assert_eq!(h.pump.get_run(&key).unwrap().event_seq, seq);
    }
    assert!(h.pump.start(&epoch(1), request).unwrap().replayed);
    h.advance(0);
    assert_eq!(h.calls().len(), 1);
    assert!(a.permit.complete(Ok(data("a"))));
    h.advance(0);
    h.advance(0);
    assert_eq!(h.calls().len(), 2);
    assert_eq!(h.calls()[1].reason, WakeReason::Input);
    assert!(h.calls()[1].seq > h.calls()[0].seq);
}
#[test]
fn input_observation_replaces_the_old_timer_and_uses_the_new_deadline() {
    let mut h = Harness::new(
        vec![node("a", "test.delay"), control("timer", "timer")],
        vec![edge("a", "timer", "a")],
        1,
        2,
        None,
    );
    let key = h.start(Harness::request(1, &["timer"]));
    let a = h.enter();
    h.advance(3);
    assert!(a.permit.complete(Ok(data("a"))));
    h.advance(3);
    assert_eq!(h.pump.next_deadline(), Some(ms(8)));
    h.advance(5);
    assert_eq!(h.calls().len(), 2);
    h.advance(8);
    assert_eq!(
        h.calls()
            .iter()
            .map(|o| (o.reason, o.elapsed))
            .collect::<Vec<_>>(),
        vec![
            (WakeReason::Started, 0),
            (WakeReason::Input, 3),
            (WakeReason::Timer, 8)
        ]
    );
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
}
#[test]
fn clock_jump_delivers_intermediate_observer_deadlines_in_chronological_order() {
    let mut h = Harness::new(vec![control("timer", "repeat")], vec![], 1, 2, None);
    let key = h.start(Harness::request(1, &["timer"]));
    h.advance(9);
    assert_eq!(
        h.calls().iter().map(|o| o.elapsed).collect::<Vec<_>>(),
        vec![0, 2, 4, 6]
    );
    assert_eq!(h.pump.get_run(&key).unwrap().elapsed_ms, 6);
}
#[test]
fn same_time_queue_failure_precedes_run_expiry_and_suppresses_job_and_observer() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            node("b", "test.delay"),
            control("timer", "timer"),
        ],
        vec![edge("a", "timer", "a"), edge("b", "timer", "b")],
        1,
        2,
        Some(5),
    );
    let mut request = Harness::request(1, &["timer"]);
    request.run_budget_ms = Some(Milliseconds::new(5).unwrap());
    request.job_budgets = serde_json::from_value(json!([{"nodeId":"a","budgetMs":5}])).unwrap();
    let key = h.start(request);
    let a = h.enter();
    h.advance(5);
    let snapshot = h.pump.get_run(&key).unwrap();
    let result = snapshot.result.unwrap();
    assert_eq!(result.reason, TerminalReason::Expired);
    assert!(
        result
            .causes
            .iter()
            .any(|cause| cause["diagnostic"]["error"]["code"] == "queue-expired")
    );
    assert!(
        !result
            .causes
            .iter()
            .any(|cause| cause["diagnostic"]["error"]["code"] == "deadline-exceeded")
    );
    assert_eq!(h.calls().len(), 1);
    assert_eq!(h.host.usage().observers, 0);
    assert!(a.cancel.is_cancelled());
    assert_eq!(snapshot.physical.cancel_requested, 1);
    assert!(h.entered.try_recv().is_err());
}
#[test]
fn job_expiry_recovers_but_physical_ownership_reaches_stalled_until_real_termination() {
    let mut h = Harness::new(
        vec![node("a", "test.delay"), control("recover", "first")],
        vec![edge("a", "recover", "a")],
        1,
        2,
        None,
    );
    let mut request = Harness::request(1, &["recover"]);
    request.job_budgets = serde_json::from_value(json!([{"nodeId":"a","budgetMs":5}])).unwrap();
    let key = h.start(request);
    let a = h.enter();
    h.advance(5);
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Recovered
    );
    assert_eq!(h.host.usage().physical_jobs, 1);
    h.advance(8);
    assert_eq!(h.pump.get_run(&key).unwrap().physical.stalled, 1);
    assert!(a.permit.finish_cancelled());
    h.settle(|pump| pump.get_run(&key).unwrap().physical.stalled == 0);
}
#[test]
fn a_large_clock_jump_evicts_results_but_still_closes_graphs_and_cancels_workers() {
    let mut h = Harness::new(
        vec![node("a", "test.delay"), control("wait", "wait")],
        vec![edge("a", "wait", "a")],
        1,
        2,
        None,
    );
    let mut request = Harness::request(1, &["wait"]);
    request.run_budget_ms = Some(Milliseconds::new(5).unwrap());
    let key = h.start(request);
    let a = h.enter();
    h.advance(30);
    assert_eq!(h.pump.get_run(&key).unwrap_err().code, "result-gone");
    assert!(a.cancel.is_cancelled());
    assert_eq!(h.host.usage().observers, 0);
    assert!(a.permit.finish_cancelled());
    h.settle(|_| true);
}
#[test]
fn new_input_cancels_with_observed_history_after_successful_reservation() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            node("b", "test.delay"),
            control("wait", "wait"),
        ],
        vec![edge("a", "wait", "a"), edge("b", "wait", "b")],
        2,
        4,
        None,
    );
    let mut request = Harness::request(1, &["wait"]);
    request.input =
        Some(serde_json::from_value(json!({"revision":1,"text":"a","mode":"compose"})).unwrap());
    let old = h.start(request.clone());
    let a = h.enter();
    let b = h.enter();
    assert!(
        a.permit
            .complete(Err(CoreError::new("test-error", "history")))
    );
    h.advance(0);
    request.start_key = id(2);
    request.input.as_mut().unwrap().revision = 2;
    request.input.as_mut().unwrap().text = "b".into();
    let new = h.start(request);
    let result = h.pump.get_run(&old).unwrap().result.unwrap();
    assert_eq!(result.reason, TerminalReason::Cancelled);
    assert!(
        result
            .causes
            .iter()
            .any(|cause| cause["diagnostic"]["error"]["code"] == "test-error")
    );
    assert!(b.cancel.is_cancelled());
    assert_ne!(old, new);
}
#[test]
fn busy_new_input_does_not_invalidate_the_old_run_or_call_observers() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            node("b", "test.delay"),
            control("wait", "wait"),
        ],
        vec![edge("a", "wait", "a"), edge("b", "wait", "b")],
        2,
        0,
        None,
    );
    let mut request = Harness::request(1, &["wait"]);
    request.input =
        Some(serde_json::from_value(json!({"revision":1,"text":"a","mode":"compose"})).unwrap());
    let old = h.start(request.clone());
    let a = h.enter();
    let b = h.enter();
    request.start_key = id(2);
    request.input.as_mut().unwrap().revision = 2;
    assert_eq!(h.pump.start(&epoch(1), request).unwrap_err().code, "busy");
    assert!(h.pump.get_run(&old).unwrap().current);
    assert!(!a.cancel.is_cancelled());
    assert!(!b.cancel.is_cancelled());
    assert_eq!(h.calls().len(), 1);
}
#[test]
fn input_invalidation_preserves_fixed_runs_and_old_history() {
    let mut h = Harness::new(
        vec![node("a", "test.delay"), control("wait", "wait")],
        vec![edge("a", "wait", "a")],
        2,
        4,
        None,
    );
    let fixed = h.start(Harness::request(1, &["wait"]));
    let _a = h.enter();
    let mut request = Harness::request(2, &["wait"]);
    request.input =
        Some(serde_json::from_value(json!({"revision":1,"text":"a","mode":"compose"})).unwrap());
    let input = h.start(request);
    let b = h.enter();
    assert!(
        b.permit
            .complete(Err(CoreError::new("test-error", "history")))
    );
    h.advance(0);
    h.pump.invalidate_input(&epoch(1), id(1), 2).unwrap();
    assert_eq!(h.pump.get_run(&fixed).unwrap().state, RunPhase::Running);
    let result = h.pump.get_run(&input).unwrap().result.unwrap();
    assert_eq!(result.reason, TerminalReason::Cancelled);
    assert_eq!(result.causes.len(), 1);
}
#[test]
fn prepare_is_inert_and_activate_rollback_do_not_resurrect_cancelled_work() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            node("b", "test.delay"),
            control("wait", "wait"),
        ],
        vec![edge("a", "wait", "a"), edge("b", "wait", "b")],
        2,
        4,
        None,
    );
    let key = h.start(Harness::request(1, &["wait"]));
    let a = h.enter();
    let b = h.enter();
    assert!(
        a.permit
            .complete(Err(CoreError::new("test-error", "history")))
    );
    h.advance(0);
    h.pump
        .prepare(
            &epoch(1),
            ExecutionPlan::compile(&h.graph, &h.registry).unwrap(),
        )
        .unwrap();
    assert_eq!(h.pump.get_run(&key).unwrap().state, RunPhase::Running);
    assert!(!b.cancel.is_cancelled());
    assert!(h.pump.activate(&epoch(1)).unwrap());
    let result = h.pump.get_run(&key).unwrap().result.unwrap();
    assert_eq!(result.causes.len(), 1);
    assert_eq!(result.reason, TerminalReason::Cancelled);
    assert!(b.cancel.is_cancelled());
    assert!(h.pump.rollback(&epoch(1)).unwrap());
    assert_eq!(h.pump.plan_version(&epoch(1)).unwrap(), id(3));
    assert!(!h.pump.get_run(&key).unwrap().current);
    assert!(h.entered.try_recv().is_err());
}
#[test]
fn sessions_share_one_fifo_and_a_queued_run_can_expire_without_starting() {
    let mut h = Harness::new(vec![node("a", "test.delay")], vec![], 1, 4, None);
    h.pump
        .open_session(
            h.host.clone(),
            epoch(2),
            ExecutionPlan::compile(&h.graph, &h.registry).unwrap(),
            id(1),
        )
        .unwrap();
    let first = h.start(Harness::request(1, &["a"]));
    let a = h.enter();
    let mut request = Harness::request(1, &["a"]);
    request.run_budget_ms = Some(Milliseconds::new(3).unwrap());
    let queued = h.pump.start(&epoch(2), request).unwrap().key;
    h.keys.push(queued.clone());
    assert_eq!(h.pump.get_run(&queued).unwrap().physical.queued, 1);
    h.advance(3);
    assert_eq!(
        h.pump.get_run(&queued).unwrap().result.unwrap().reason,
        TerminalReason::Expired
    );
    assert!(h.entered.try_recv().is_err());
    assert_eq!(h.pump.get_run(&first).unwrap().state, RunPhase::Running);
    assert!(a.permit.complete(Ok(data("a"))));
    h.advance(3);
}
#[test]
fn backward_clock_and_wrong_keys_leave_real_work_and_policy_unchanged() {
    let mut h = Harness::new(
        vec![node("a", "test.delay"), control("wait", "wait")],
        vec![edge("a", "wait", "a")],
        1,
        2,
        None,
    );
    let key = h.start(Harness::request(1, &["wait"]));
    let a = h.enter();
    h.advance(2);
    let before = serde_json::to_value(h.pump.get_run(&key).unwrap()).unwrap();
    assert_eq!(h.pump.advance_to(ms(1)).unwrap_err().code, "invalid-clock");
    assert_eq!(
        serde_json::to_value(h.pump.get_run(&key).unwrap()).unwrap(),
        before
    );
    let mut wrong = key.clone();
    wrong.delivery_generation = id(99);
    assert_eq!(h.pump.cancel(&wrong).unwrap_err().code, "run-key-conflict");
    assert!(!a.cancel.is_cancelled());
    assert_eq!(h.calls().len(), 1);
}
#[test]
fn cascading_controls_rebuild_frames_after_earlier_observers_in_the_same_turn() {
    let mut h = Harness::new(
        vec![
            node("a", "test.delay"),
            control("c1", "first"),
            control("c2", "first"),
        ],
        vec![edge("a", "c1", "a"), edge("c1", "c2", "a")],
        1,
        2,
        None,
    );
    let key = h.start(Harness::request(1, &["c2"]));
    let a = h.enter();
    assert!(a.permit.complete(Ok(data("a"))));
    h.advance(0);
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
    let calls = h.calls();
    assert_eq!(calls.iter().filter(|o| o.node == "c1").count(), 2);
    assert_eq!(calls.iter().filter(|o| o.node == "c2").count(), 2);
}

#[test]
fn legacy_sync_workers_feed_the_same_automatic_dependency_driver() {
    let mut source = node("source", "demo.packet");
    source["config"] = json!({"packet":{"version":1,"root":{"type":"text","text":"sync"}}});
    let mut h = Harness::new(
        vec![source, node("a", "test.delay")],
        vec![edge("source", "a", "in")],
        1,
        2,
        None,
    );
    let key = h.start(Harness::request(1, &["a"]));
    let a = h.enter();
    assert!(a.permit.complete(Ok(data("async"))));
    h.advance(0);
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
}

#[test]
fn shared_sessions_dispatch_in_fifo_order_even_when_a_new_start_arrives() {
    let mut h = Harness::new(vec![node("a", "test.delay")], vec![], 1, 4, None);
    h.pump
        .open_session(
            h.host.clone(),
            epoch(2),
            ExecutionPlan::compile(&h.graph, &h.registry).unwrap(),
            id(1),
        )
        .unwrap();
    let first = h.start(Harness::request(1, &["a"]));
    let a = h.enter();
    let second = h
        .pump
        .start(&epoch(2), Harness::request(1, &["a"]))
        .unwrap()
        .key;
    h.keys.push(second.clone());
    let third = h.start(Harness::request(2, &["a"]));
    assert!(a.permit.complete(Ok(data("first"))));
    let b = h.enter();
    assert_eq!(b.permit.key(), &second);
    assert_eq!(h.pump.get_run(&third).unwrap().physical.queued, 1);
    assert!(b.permit.complete(Ok(data("second"))));
    let c = h.enter();
    assert_eq!(c.permit.key(), &third);
    assert_eq!(
        h.pump.get_run(&first).unwrap().result.unwrap().reason,
        TerminalReason::Completed
    );
}

#[test]
fn already_committed_completion_wins_a_later_cancel_in_the_same_display_millisecond() {
    let mut h = Harness::new(vec![node("a", "test.delay")], vec![], 1, 2, None);
    let key = h.start(Harness::request(1, &["a"]));
    let a = h.enter();
    assert!(a.permit.complete(Ok(data("a"))));
    h.advance(0);
    let snapshot = h.pump.cancel(&key).unwrap();
    assert_eq!(snapshot.elapsed_ms, 0);
    assert_eq!(snapshot.result.unwrap().reason, TerminalReason::Completed);
}

#[test]
fn accepted_cancel_discards_a_previously_sent_but_unadopted_result() {
    let mut h = Harness::new(vec![node("a", "test.delay")], vec![], 1, 2, None);
    let key = h.start(Harness::request(1, &["a"]));
    let a = h.enter();
    assert!(a.permit.complete(Ok(data("a"))));
    assert_eq!(
        h.pump.cancel(&key).unwrap().result.unwrap().reason,
        TerminalReason::Cancelled
    );
    h.advance(0);
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Cancelled
    );
}

#[test]
fn submillisecond_deadline_precision_does_not_use_floored_elapsed_display() {
    let mut h = Harness::new(vec![node("a", "test.delay")], vec![], 1, 2, None);
    h.pump.advance_to(Duration::from_nanos(999_999)).unwrap();
    let mut request = Harness::request(1, &["a"]);
    request.job_budgets = serde_json::from_value(json!([{"nodeId":"a","budgetMs":1}])).unwrap();
    let key = h.start(request);
    let _a = h.enter();
    h.pump.advance_to(Duration::from_nanos(1_999_998)).unwrap();
    assert_eq!(h.pump.get_run(&key).unwrap().elapsed_ms, 0);
    assert_eq!(h.pump.get_run(&key).unwrap().state, RunPhase::Running);
    h.pump.advance_to(Duration::from_nanos(1_999_999)).unwrap();
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Failed
    );
}

#[test]
fn duplicate_and_foreign_sessions_are_rejected_before_changing_pump_state() {
    let mut h = Harness::new(vec![control("wait", "wait")], vec![], 1, 2, None);
    let key = h.start(Harness::request(1, &["wait"]));
    assert_eq!(
        h.pump
            .open_session(
                h.host.clone(),
                epoch(1),
                ExecutionPlan::compile(&h.graph, &h.registry).unwrap(),
                id(1)
            )
            .unwrap_err()
            .code,
        "session-conflict"
    );
    let foreign = ExecutionHost::new(
        format!("foreign-{}", h.host.resource_domain()),
        h.host.policy().clone(),
    )
    .unwrap();
    assert_eq!(
        h.pump
            .open_session(
                foreign,
                epoch(2),
                ExecutionPlan::compile(&h.graph, &h.registry).unwrap(),
                id(1)
            )
            .unwrap_err()
            .code,
        "resource-domain-conflict"
    );
    assert_eq!(h.calls().len(), 1);
    assert_eq!(h.pump.get_run(&key).unwrap().state, RunPhase::Running);
}
