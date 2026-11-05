#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    error_input::{
        ErrorAwareExecution, ErrorAwareInputs, ErrorAwareNode, ErrorAwareValue, RecoveryClaim,
    },
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlInput,
        ControlNode, DecimalId, DeferredNode, ExecutionHost, ExecutionKind, GraphExecution,
        GraphNodePhase, GraphObservation, Milliseconds, OwnedInvocation, ResourcePolicy,
        ResourceUsage, RunDelivery, RunKey, RunLedger, SessionEpoch, TerminalReason, WakeReason,
        WorkerCompletion, WorkerDispatcher,
    },
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RunContext,
    },
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
use test_nodes::register_demo_nodes;
struct Source {
    calls: Arc<Mutex<Vec<String>>>,
}
impl Node for Source {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.source", 1, vec![], vec![Port::packet("out")])
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        _: &Inputs,
        _: &Value,
    ) -> Result<Outputs, CoreError> {
        self.calls.lock().unwrap().push(context.node_id.into());
        let value = if context.node_id == "fail" {
            return Err(CoreError::new("source-failed", "origin"));
        } else if context.node_id.starts_with("no") {
            OutputValue::NoOutput
        } else {
            OutputValue::Data(text(
                context.input.map_or("source", |input| input.text.as_str()),
            ))
        };
        Ok([("out".into(), value)].into())
    }
}
struct Gate {
    calls: Arc<AtomicU64>,
}
impl Node for Gate {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.gate",
            1,
            vec![
                InputPort::required(Port::packet("a")),
                InputPort::optional(Port::packet("b")),
            ],
            vec![Port::packet("out")],
        )
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok([("out".into(), OutputValue::Data(text("gate")))].into())
    }
}
struct Recover;
impl ErrorAwareNode for Recover {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.recover",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec!["in".into()]
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        inputs: &ErrorAwareInputs,
        _: &Value,
    ) -> Result<ErrorAwareExecution, CoreError> {
        let ErrorAwareValue::Error(errors) = &inputs["in"] else {
            panic!("expected error")
        };
        let mut out = ErrorAwareExecution::new(
            [("out".into(), ErrorAwareValue::Data(text("fallback")))].into(),
        );
        out.recoveries.push(RecoveryClaim {
            input: "in".into(),
            output: "out".into(),
            causes: errors
                .causes()
                .iter()
                .map(|cause| cause.id.clone())
                .collect(),
        });
        Ok(out)
    }
}
type AsyncMessage = (CompletionPermit, CancellationToken);
struct AsyncSource {
    sender: mpsc::Sender<AsyncMessage>,
    calls: Arc<AtomicU64>,
}
impl DeferredNode for AsyncSource {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.async", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        _: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.sender.send((permit, cancel)).unwrap();
        Ok(())
    }
}
struct Policy;
impl ControlNode for Policy {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.control",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn observe(
        &self,
        context: RunContext<'_>,
        frame: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        if context.node_id == "fast" || frame.reason == WakeReason::Timer {
            return Ok(ControlDecision::Finish(out(text(context.node_id))));
        }
        match &frame.inputs["in"] {
            ControlInput::Resolved(value) => Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                [("out".into(), value.clone())].into(),
            ))),
            ControlInput::Pending => Ok(ControlDecision::Wait {
                wake_after_ms: Some(Milliseconds::new(5).unwrap()),
            }),
        }
    }
}
fn text(value: &str) -> Arc<Packet> {
    Arc::new(Packet::new(Candidate::Text {
        text: value.into(),
        meta: None,
    }))
}
fn out(packet: Arc<Packet>) -> ErrorAwareExecution {
    ErrorAwareExecution::new([("out".into(), ErrorAwareValue::Data(packet))].into())
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
struct Host {
    ledger: RunLedger,
    key: RunKey,
    graph: GraphExecution,
    workers: WorkerDispatcher,
    host: ExecutionHost,
    calls: Arc<Mutex<Vec<String>>>,
    gate_calls: Arc<AtomicU64>,
    async_calls: Arc<AtomicU64>,
    async_messages: mpsc::Receiver<AsyncMessage>,
}
impl Host {
    fn new(nodes: Value, edges: Value, targets: &[&str]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let profile = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        let host = ExecutionHost::new(
            format!("graph-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            profile,
        )
        .unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let gate_calls = Arc::new(AtomicU64::new(0));
        let async_calls = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        registry
            .register(Source {
                calls: Arc::clone(&calls),
            })
            .unwrap();
        registry
            .register(Gate {
                calls: Arc::clone(&gate_calls),
            })
            .unwrap();
        registry.register_error_aware(Recover).unwrap();
        registry
            .register_deferred(AsyncSource {
                sender: tx,
                calls: Arc::clone(&async_calls),
            })
            .unwrap();
        registry.register_control(Policy).unwrap();
        let graph = Graph::from_json(&json!({"version":1,"nodes":nodes,"edges":edges}).to_string())
            .unwrap();
        let plan = ExecutionPlan::compile(&graph, &registry).unwrap();
        let mut ledger = RunLedger::new(
            host.clone(),
            SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
            plan,
            id(1),
        );
        let key=ledger.start_run(serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":targets.iter().map(|node|json!({"node":node,"port":"out"})).collect::<Vec<_>>(),"input":{"revision":1,"text":"かな😀","mode":"compose"}})).unwrap()).unwrap().key;
        ledger.mark_running(&key).unwrap();
        let graph = ledger.graph_execution(&key).unwrap();
        let workers = WorkerDispatcher::new(host.clone());
        Self {
            ledger,
            key,
            graph,
            workers,
            host,
            calls,
            gate_calls,
            async_calls,
            async_messages: rx,
        }
    }
    fn dispatch(&mut self, node: &str) -> DecimalId {
        let invocation = self.graph.start_work(node).unwrap();
        let job = self.workers.submit(invocation).unwrap();
        self.graph.bind_job(node, job).unwrap();
        job
    }
    fn poll_logical(&mut self) -> Vec<diy_ime2_core::execution_control::ReleasedNode> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut releases = vec![];
        loop {
            let mut logical = false;
            for completion in self.workers.poll() {
                logical |= completion.execution().is_some();
                releases.extend(self.graph.complete_work(completion).unwrap());
            }
            if logical {
                return releases;
            }
            assert!(Instant::now() < deadline, "logical completion timeout");
            std::thread::yield_now();
        }
    }
    fn drain_physical(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.workers.states().next().is_some() {
            for completion in self.workers.poll() {
                self.graph.complete_work(completion).unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn finish(&mut self) -> diy_ime2_core::execution_control::TerminalResult {
        self.graph
            .finish(&mut self.ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .result
            .unwrap()
    }
}
fn node(id: &str, kind: &str) -> Value {
    json!({"id":id,"kind":kind,"version":1})
}
fn edge(from: &str, to: &str, input: &str) -> Value {
    json!({"from":{"node":from,"port":"out"},"to":{"node":to,"port":input}})
}

#[test]
fn real_graph_waits_for_inputs_shares_source_snapshot_and_preserves_ready_order() {
    let mut h = Host::new(
        json!([
            node("unused", "test.source"),
            node("source", "test.source"),
            node("left", "demo.identity"),
            node("right", "demo.identity")
        ]),
        json!([edge("source", "left", "in"), edge("source", "right", "in")]),
        &["right", "left"],
    );
    assert_eq!(
        h.graph
            .ready_nodes()
            .unwrap()
            .iter()
            .map(|n| n.node_id.as_str())
            .collect::<Vec<_>>(),
        ["source"]
    );
    assert!(h.graph.start_work("left").is_err());
    h.dispatch("source");
    assert!(h.graph.start_work("source").is_err());
    h.poll_logical();
    assert_eq!(
        h.graph
            .ready_nodes()
            .unwrap()
            .iter()
            .map(|n| n.node_id.as_str())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );
    let left = h.graph.start_work("left").unwrap();
    let right = h.graph.start_work("right").unwrap();
    let (ErrorAwareValue::Data(a), ErrorAwareValue::Data(b)) =
        (&left.inputs()["in"], &right.inputs()["in"])
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(a, b));
    assert_eq!(left.context().input.unwrap().text, "かな😀");
    let a = h.workers.submit(left).unwrap();
    h.graph.bind_job("left", a).unwrap();
    let b = h.workers.submit(right).unwrap();
    h.graph.bind_job("right", b).unwrap();
    while !h.graph.is_complete() {
        h.poll_logical();
    }
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Complete);
    assert_eq!(*h.calls.lock().unwrap(), ["source"]);
    assert_eq!(result.nodes.len(), 3);
    assert!(h.graph.ready_nodes().unwrap().is_empty());
    assert_eq!(h.host.usage().physical_jobs, 0);
}

#[test]
fn normal_gates_wait_for_all_inputs_and_blocked_error_wins_over_required_no_output() {
    for blocked in [false, true] {
        let other = if blocked { "fail" } else { "no2" };
        let mut h = Host::new(
            json!([
                node("no1", "test.source"),
                node(other, "test.source"),
                node("gate", "test.gate")
            ]),
            json!([edge("no1", "gate", "a"), edge(other, "gate", "b")]),
            &["gate"],
        );
        h.dispatch("no1");
        h.poll_logical();
        assert_eq!(h.graph.phase("gate"), Some(GraphNodePhase::Pending));
        assert!(!h.graph.is_complete());
        h.dispatch(other);
        h.poll_logical();
        assert!(h.graph.is_complete());
        assert_eq!(h.gate_calls.load(Ordering::Relaxed), 0);
        let result = h.finish();
        assert_eq!(
            result.nodes[2]["status"],
            if blocked { "blocked" } else { "skipped" }
        );
        assert_eq!(
            result.delivery,
            if blocked {
                RunDelivery::Failed
            } else {
                RunDelivery::NoOutput
            }
        );
        if blocked {
            assert_eq!(result.causes[0]["paths"][0]["to"]["port"], "b");
        }
    }
}

#[test]
fn error_routes_and_recovery_history_reach_a_later_normal_data_consumer() {
    let mut h = Host::new(
        json!([
            node("fail", "test.source"),
            node("recover", "test.recover"),
            node("sink", "demo.identity")
        ]),
        json!([edge("fail", "recover", "in"), edge("recover", "sink", "in")]),
        &["sink"],
    );
    for name in ["fail", "recover", "sink"] {
        assert_eq!(h.graph.ready_nodes().unwrap()[0].node_id, name);
        h.dispatch(name);
        h.poll_logical();
    }
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Recovered);
    assert_eq!(result.recoveries.len(), 1);
    assert_eq!(result.causes[0]["paths"][0]["to"]["node"], "recover");
    assert_eq!(
        result.ports.last().unwrap().value["packet"]["root"]["text"],
        "fallback"
    );
}

#[test]
fn deferred_source_and_pending_control_frame_connect_without_blocking_host() {
    let mut h = Host::new(
        json!([node("async", "test.async"), node("slow", "test.control")]),
        json!([edge("async", "slow", "in")]),
        &["slow"],
    );
    assert_eq!(
        h.graph
            .ready_nodes()
            .unwrap()
            .iter()
            .map(|n| n.execution_kind)
            .collect::<Vec<_>>(),
        [ExecutionKind::Deferred, ExecutionKind::Control]
    );
    h.dispatch("async");
    let (permit, _) = h
        .async_messages
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let mut observer = h.graph.start_observer("slow").unwrap();
    assert!(
        matches!(h.graph.observe("slow",&mut observer,WakeReason::Started,id(10),Duration::ZERO).unwrap(),GraphObservation::Wait{wake_at:Some(at)}if at==Duration::from_millis(5))
    );
    assert!(h.graph.ready_nodes().unwrap().is_empty());
    assert!(!h.graph.is_complete());
    assert!(permit.complete(Ok(out(text("async result")))));
    h.poll_logical();
    h.ledger.advance_clock(Duration::from_millis(1)).unwrap();
    assert!(matches!(
        h.graph
            .observe(
                "slow",
                &mut observer,
                WakeReason::Input,
                id(11),
                Duration::from_millis(1)
            )
            .unwrap(),
        GraphObservation::Finished { .. }
    ));
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Complete);
    assert_eq!(
        result.ports.last().unwrap().value["packet"]["root"]["text"],
        "async result"
    );
    h.drain_physical();
    assert_eq!(h.host.usage().physical_jobs, 0);
    assert_eq!(h.host.usage().observers, 0);
}

#[test]
fn shared_source_is_released_only_when_the_last_control_consumer_finishes() {
    let mut h = Host::new(
        json!([
            node("async", "test.async"),
            node("fast", "test.control"),
            node("slow", "test.control")
        ]),
        json!([edge("async", "fast", "in"), edge("async", "slow", "in")]),
        &["fast", "slow"],
    );
    let job = h.dispatch("async");
    let (permit, cancel) = h
        .async_messages
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let mut fast = h.graph.start_observer("fast").unwrap();
    let mut slow = h.graph.start_observer("slow").unwrap();
    assert!(matches!(
        h.graph
            .observe(
                "slow",
                &mut slow,
                WakeReason::Started,
                id(10),
                Duration::ZERO
            )
            .unwrap(),
        GraphObservation::Wait { .. }
    ));
    let GraphObservation::Finished { released } = h
        .graph
        .observe(
            "fast",
            &mut fast,
            WakeReason::Started,
            id(11),
            Duration::ZERO,
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(released.is_empty());
    assert_eq!(h.graph.phase("async"), Some(GraphNodePhase::Active));
    assert!(!cancel.is_cancelled());
    assert_eq!(h.async_calls.load(Ordering::Relaxed), 1);
    h.ledger.advance_clock(Duration::from_millis(5)).unwrap();
    let GraphObservation::Finished { released } = h
        .graph
        .observe(
            "slow",
            &mut slow,
            WakeReason::Timer,
            id(12),
            Duration::from_millis(5),
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].node_id, "async");
    assert_eq!(released[0].job_id, Some(job));
    assert_eq!(h.graph.phase("async"), Some(GraphNodePhase::Discarded));
    assert_eq!(h.host.usage().physical_jobs, 1);
    h.workers.cancel(job, Duration::from_millis(5));
    assert!(cancel.is_cancelled());
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Complete);
    assert_eq!(
        result.ports[0].value,
        json!({"status":"discarded","reason":"unneeded"})
    );
    assert!(permit.complete(Ok(out(text("late")))));
    h.drain_physical();
    assert_eq!(h.host.usage().physical_jobs, 0);
    assert_eq!(
        serde_json::to_value(h.ledger.get_run(&h.key).unwrap().result.unwrap()).unwrap(),
        serde_json::to_value(result).unwrap()
    );
}

#[test]
fn graph_cancel_releases_logical_consumers_without_claiming_physical_termination() {
    let mut h = Host::new(
        json!([node("async", "test.async"), node("slow", "test.control")]),
        json!([edge("async", "slow", "in")]),
        &["slow"],
    );
    let job = h.dispatch("async");
    let (permit, cancel) = h
        .async_messages
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let observer = h.graph.start_observer("slow").unwrap();
    let completion = h
        .graph
        .finish(&mut h.ledger, TerminalReason::Cancelled)
        .unwrap();
    assert_eq!(completion.released.len(), 2);
    assert!(h.graph.ready_nodes().unwrap().is_empty());
    assert_eq!(h.host.usage().physical_jobs, 1);
    h.workers.cancel(job, Duration::ZERO);
    assert!(cancel.is_cancelled());
    drop(observer);
    assert_eq!(h.host.usage().observers, 0);
    assert!(permit.finish_cancelled());
    h.drain_physical();
    assert_eq!(h.host.usage().physical_jobs, 0);
    assert_eq!(
        completion.snapshot.result.unwrap().delivery,
        RunDelivery::Cancelled
    );
}

#[test]
fn a_later_ready_work_handoff_failure_is_a_node_error_and_keeps_prior_history() {
    let mut h = Host::new(
        json!([node("source", "test.source"), node("sink", "demo.identity")]),
        json!([edge("source", "sink", "in")]),
        &["sink"],
    );
    h.dispatch("source");
    h.poll_logical();
    let held = h
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 2,
            ..Default::default()
        })
        .unwrap();
    let invocation = h.graph.start_work("sink").unwrap();
    assert_eq!(h.workers.submit(invocation).err().unwrap().code, "busy");
    h.graph
        .fail_work("sink", CoreError::new("resource-busy", "no queue capacity"))
        .unwrap();
    drop(held);
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Failed);
    assert_eq!(
        result.causes[0]["diagnostic"]["error"]["code"],
        "resource-busy"
    );
    assert_eq!(result.nodes[0]["status"], "executed");
}

#[test]
fn wrong_key_unbound_job_and_duplicate_completion_cannot_resolve_graph_work() {
    let mut h = Host::new(
        json!([node("source", "test.source")]),
        json!([]),
        &["source"],
    );
    let job = h.dispatch("source");
    assert!(h.graph.bind_job("source", job).is_err());
    fn next(workers: &mut WorkerDispatcher) -> WorkerCompletion {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(completion) = workers
                .poll()
                .into_iter()
                .find(|event| event.execution().is_some())
            {
                return completion;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    let mut other = Host::new(
        json!([node("source", "test.source")]),
        json!([]),
        &["source"],
    );
    other.dispatch("source");
    assert_eq!(
        h.graph
            .complete_work(next(&mut other.workers))
            .unwrap_err()
            .code,
        "run-key-conflict"
    );
    let mut unbound = WorkerDispatcher::new(h.host.clone());
    unbound
        .submit(
            h.ledger
                .owned_invocation(&h.key, "source", ErrorAwareInputs::new())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        h.graph.complete_work(next(&mut unbound)).unwrap_err().code,
        "stale-completion"
    );
    let completion = next(&mut h.workers);
    let duplicate = completion.clone();
    assert!(!h.graph.is_complete());
    h.graph.complete_work(completion).unwrap();
    assert_eq!(
        h.graph.complete_work(duplicate).unwrap_err().code,
        "stale-completion"
    );
    assert_eq!(h.finish().delivery, RunDelivery::Complete);
}

#[test]
fn same_input_runs_execute_their_own_source_within_the_same_host() {
    let mut h = Host::new(
        json!([node("source", "test.source")]),
        json!([]),
        &["source"],
    );
    let key2=h.ledger.start_run(serde_json::from_value(json!({"startKey":"2","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],"input":{"revision":1,"text":"かな😀","mode":"compose"}})).unwrap()).unwrap().key;
    let mut graph2 = h.ledger.graph_execution(&key2).unwrap();
    h.dispatch("source");
    let job2 = h
        .workers
        .submit(graph2.start_work("source").unwrap())
        .unwrap();
    graph2.bind_job("source", job2).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.graph.is_complete() || !graph2.is_complete() {
        for event in h.workers.poll() {
            if event.stamp().key == h.key {
                h.graph.complete_work(event).unwrap();
            } else {
                assert_eq!(event.stamp().key, key2);
                graph2.complete_work(event).unwrap();
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_ne!(h.key.run_id, key2.run_id);
    assert_eq!(h.calls.lock().unwrap().len(), 2);
    assert_eq!(h.finish().delivery, RunDelivery::Complete);
    assert_eq!(
        graph2
            .finish(&mut h.ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .result
            .unwrap()
            .delivery,
        RunDelivery::Complete
    );
}

#[test]
fn a_direct_output_target_keeps_shared_work_needed_after_control_finishes() {
    let mut h = Host::new(
        json!([node("async", "test.async"), node("fast", "test.control")]),
        json!([edge("async", "fast", "in")]),
        &["fast", "async"],
    );
    h.dispatch("async");
    let (permit, cancel) = h
        .async_messages
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let mut observer = h.graph.start_observer("fast").unwrap();
    let GraphObservation::Finished { released } = h
        .graph
        .observe(
            "fast",
            &mut observer,
            WakeReason::Started,
            id(10),
            Duration::ZERO,
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(released.is_empty());
    assert!(!cancel.is_cancelled());
    assert_eq!(h.graph.phase("async"), Some(GraphNodePhase::Active));
    assert!(!h.graph.is_complete());
    assert!(permit.complete(Ok(out(text("direct target")))));
    h.poll_logical();
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Complete);
    assert_eq!(
        result.ports[0].value["packet"]["root"]["text"],
        "direct target"
    );
    h.drain_physical();
}

#[test]
fn nested_unneeded_observer_is_released_without_freeing_the_underlying_worker() {
    let mut h = Host::new(
        json!([
            node("async", "test.async"),
            node("inner", "test.control"),
            node("fast", "test.control")
        ]),
        json!([edge("async", "inner", "in"), edge("inner", "fast", "in")]),
        &["fast"],
    );
    let job = h.dispatch("async");
    let (permit, _) = h
        .async_messages
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let mut inner = h.graph.start_observer("inner").unwrap();
    let mut fast = h.graph.start_observer("fast").unwrap();
    assert!(matches!(
        h.graph
            .observe(
                "inner",
                &mut inner,
                WakeReason::Started,
                id(10),
                Duration::ZERO
            )
            .unwrap(),
        GraphObservation::Wait { .. }
    ));
    let GraphObservation::Finished { released } = h
        .graph
        .observe(
            "fast",
            &mut fast,
            WakeReason::Started,
            id(11),
            Duration::ZERO,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        released
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        ["async", "inner"]
    );
    assert_eq!(released[1].execution_kind, ExecutionKind::Control);
    assert_eq!(h.host.usage().observers, 1);
    assert!(
        h.graph
            .observe(
                "inner",
                &mut inner,
                WakeReason::Timer,
                id(12),
                Duration::from_millis(5)
            )
            .is_err()
    );
    drop(inner);
    assert_eq!(h.host.usage().observers, 0);
    assert_eq!(h.host.usage().physical_jobs, 1);
    h.workers.cancel(job, Duration::ZERO);
    assert!(permit.finish_cancelled());
    assert_eq!(h.finish().delivery, RunDelivery::Complete);
    h.drain_physical();
}

#[test]
fn inherited_recovery_history_is_also_carried_through_control_data_observation() {
    let mut h = Host::new(
        json!([
            node("fail", "test.source"),
            node("recover", "test.recover"),
            node("slow", "test.control")
        ]),
        json!([edge("fail", "recover", "in"), edge("recover", "slow", "in")]),
        &["slow"],
    );
    let mut observer = h.graph.start_observer("slow").unwrap();
    assert!(matches!(
        h.graph
            .observe(
                "slow",
                &mut observer,
                WakeReason::Started,
                id(10),
                Duration::ZERO
            )
            .unwrap(),
        GraphObservation::Wait { .. }
    ));
    h.dispatch("fail");
    h.poll_logical();
    h.dispatch("recover");
    h.poll_logical();
    h.ledger.advance_clock(Duration::from_millis(1)).unwrap();
    assert!(matches!(
        h.graph
            .observe(
                "slow",
                &mut observer,
                WakeReason::Input,
                id(11),
                Duration::from_millis(1)
            )
            .unwrap(),
        GraphObservation::Finished { .. }
    ));
    let result = h.finish();
    assert_eq!(result.delivery, RunDelivery::Recovered);
    assert_eq!(result.recoveries.len(), 1);
    assert_eq!(
        result.ports.last().unwrap().value["packet"]["root"]["text"],
        "fallback"
    );
}
