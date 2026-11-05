#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet, TextMeta,
    error_input::{
        ErrorAwareExecution, ErrorAwareInputs, ErrorAwareNode, ErrorAwareValue, RecoveryClaim,
    },
    execution_control::{
        DecimalId, ExecutionHost, ResourcePolicy, RunDelivery, RunKey, RunLedger, RunPhase,
        TerminalReason, WorkerDispatcher,
    },
    graph::{Endpoint, ExecutionPlan, Graph},
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RunContext,
    },
    runtime::{NodeExecution, ResolvedValue},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
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
        let value = match context.node_id {
            "fail" => return Err(CoreError::new("source-failed", "original failure")),
            "no" => OutputValue::NoOutput,
            "deep" => {
                let mut root = Candidate::Text {
                    text: "deep".into(),
                    meta: None,
                };
                for _ in 0..80 {
                    root = Candidate::Sequence { items: vec![root] };
                }
                OutputValue::Data(Arc::new(Packet::new(root)))
            }
            "large" => OutputValue::Data(text(&"x".repeat(10_000))),
            _ => OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: "かな😀 runId:9007199254740993".into(),
                meta: Some(TextMeta {
                    reading: Some("planVersion:1".into()),
                    source_span: Some([0, 3]),
                }),
            }))),
        };
        Ok([("out".into(), value)].into())
    }
}
struct Recover;
impl ErrorAwareNode for Recover {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.recover",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("out"), Port::packet("unused")],
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
            panic!("expected source error")
        };
        let mut result = ErrorAwareExecution::new(
            [
                ("out".into(), ErrorAwareValue::Data(text("fallback"))),
                ("unused".into(), ErrorAwareValue::NoOutput),
            ]
            .into(),
        );
        result.recoveries.push(RecoveryClaim {
            input: "in".into(),
            output: "out".into(),
            causes: errors
                .causes()
                .iter()
                .map(|cause| cause.id.clone())
                .collect(),
        });
        Ok(result)
    }
}
fn text(value: &str) -> Arc<Packet> {
    Arc::new(Packet::new(Candidate::Text {
        text: value.into(),
        meta: None,
    }))
}
fn id(value: u64) -> DecimalId {
    DecimalId::new(value).unwrap()
}
fn policy() -> ResourcePolicy {
    ResourcePolicy::from_value(serde_json::from_str::<Value>(include_str!("contracts/execution_control.json")).unwrap()["resourceProfiles"]["bounded"].clone()).unwrap()
}
type Setup = (
    RunLedger,
    RunKey,
    WorkerDispatcher,
    ExecutionHost,
    Arc<Mutex<Vec<String>>>,
);
fn setup(graph: Value, targets: Value, budget: Value, profile: ResourcePolicy) -> Setup {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let host = ExecutionHost::new(
        format!("result-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        profile,
    )
    .unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    registry
        .register(Source {
            calls: Arc::clone(&calls),
        })
        .unwrap();
    registry.register_error_aware(Recover).unwrap();
    let graph = Graph::from_json(&graph.to_string()).unwrap();
    let plan = ExecutionPlan::compile(&graph, &registry).unwrap();
    let mut ledger = RunLedger::new(
        host.clone(),
        diy_ime2_core::execution_control::SessionEpoch::new("0123456789abcdef0123456789abcdef")
            .unwrap(),
        plan,
        id(3),
    );
    let key=ledger.start_run(serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"3","targets":targets,"runBudgetMs":budget})).unwrap()).unwrap().key;
    ledger.mark_running(&key).unwrap();
    let dispatcher = WorkerDispatcher::new(host.clone());
    (ledger, key, dispatcher, host, calls)
}
fn nodes(names: &[&str]) -> Value {
    json!({"version":1,"nodes":names.iter().map(|id|json!({"id":id,"kind":"test.source","version":1})).collect::<Vec<_>>(),"edges":[]})
}
fn targets(names: &[&str]) -> Value {
    json!(
        names
            .iter()
            .map(|name| json!({"node":name,"port":"out"}))
            .collect::<Vec<_>>()
    )
}
fn execute(
    ledger: &RunLedger,
    key: &RunKey,
    d: &mut WorkerDispatcher,
    node: &str,
    inputs: ErrorAwareInputs,
) -> NodeExecution {
    d.submit(ledger.owned_invocation(key, node, inputs).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(execution) = d
            .poll()
            .into_iter()
            .find_map(|event| event.into_execution())
        {
            return execution;
        }
        assert!(Instant::now() < deadline, "worker timeout");
        std::thread::yield_now();
    }
}
fn recovery_graph() -> Value {
    json!({"version":1,"nodes":[{"id":"fail","kind":"test.source","version":1},{"id":"recover","kind":"test.recover","version":1}],"edges":[{"from":{"node":"fail","port":"out"},"to":{"node":"recover","port":"in"}}]})
}

#[test]
fn target_ancestors_keep_stable_order_share_source_and_exclude_unrelated_work() {
    let graph = json!({"version":1,"nodes":[{"id":"unused","kind":"test.source","version":1},{"id":"source","kind":"test.source","version":1},{"id":"left","kind":"demo.identity","version":1},{"id":"right","kind":"demo.identity","version":1}],"edges":[{"from":{"node":"source","port":"out"},"to":{"node":"left","port":"in"}},{"from":{"node":"source","port":"out"},"to":{"node":"right","port":"in"}}]});
    let (mut ledger, key, mut d, _, calls) =
        setup(graph, targets(&["right", "left"]), Value::Null, policy());
    let mut results = ledger.result_collector(&key).unwrap();
    assert_eq!(
        results.required_nodes().collect::<Vec<_>>(),
        ["source", "left", "right"]
    );
    let source = execute(&ledger, &key, &mut d, "source", BTreeMap::new());
    let ResolvedValue::Data(packet) = &source.outputs["out"] else {
        panic!()
    };
    for name in ["left", "right"] {
        let execution = execute(
            &ledger,
            &key,
            &mut d,
            name,
            [("in".into(), ErrorAwareValue::Data(Arc::clone(packet)))].into(),
        );
        results.record(name, execution).unwrap();
    }
    results.record("source", source.clone()).unwrap();
    assert_eq!(
        results.record("unused", source).unwrap_err().code,
        "unrequested-node"
    );
    let snapshot = ledger
        .finish_collected(&mut results, TerminalReason::Completed)
        .unwrap();
    let result = snapshot.result.unwrap();
    assert_eq!(result.delivery, RunDelivery::Complete);
    assert_eq!(*calls.lock().unwrap(), ["source"]);
    assert_eq!(result.nodes.len(), 3);
    assert_eq!(
        result.targets,
        [Endpoint::new("right", "out"), Endpoint::new("left", "out")]
    );
    let value = serde_json::to_value(&result).unwrap();
    assert!(value.get("succeeded").is_none());
    assert_eq!(
        value["ports"][0]["value"]["packet"]["root"]["meta"]["sourceSpan"],
        json!([0, 3])
    );
}

#[test]
fn unresolved_targets_never_commit_and_completed_results_are_immutable() {
    let (mut ledger, key, mut d, _, _) = setup(
        nodes(&["source"]),
        targets(&["source"]),
        Value::Null,
        policy(),
    );
    let mut results = ledger.result_collector(&key).unwrap();
    let before = serde_json::to_value(ledger.get_run(&key).unwrap()).unwrap();
    assert_eq!(
        ledger
            .finish_collected(&mut results, TerminalReason::Completed)
            .unwrap_err()
            .code,
        "run-pending"
    );
    assert_eq!(
        serde_json::to_value(ledger.get_run(&key).unwrap()).unwrap(),
        before
    );
    let execution = execute(&ledger, &key, &mut d, "source", BTreeMap::new());
    results.record("source", execution.clone()).unwrap();
    assert_eq!(
        results
            .record("source", execution.clone())
            .unwrap_err()
            .code,
        "duplicate-result"
    );
    let done = ledger
        .finish_collected(&mut results, TerminalReason::Completed)
        .unwrap();
    assert_eq!(
        results.record("source", execution).unwrap_err().code,
        "run-terminal"
    );
    assert_eq!(
        serde_json::to_value(results.preview(TerminalReason::Cancelled).unwrap()).unwrap(),
        serde_json::to_value(done.result.unwrap()).unwrap()
    );
}

#[test]
fn delivery_priority_depends_only_on_requested_ports_in_either_order() {
    for requested in [
        vec!["source", "no"],
        vec!["no", "source"],
        vec!["no", "fail"],
        vec!["fail", "no"],
    ] {
        let (mut ledger, key, mut d, _, _) = setup(
            nodes(&["source", "no", "fail"]),
            targets(&requested),
            Value::Null,
            policy(),
        );
        let mut results = ledger.result_collector(&key).unwrap();
        for name in &requested {
            results
                .record(name, execute(&ledger, &key, &mut d, name, BTreeMap::new()))
                .unwrap();
        }
        let result = ledger
            .finish_collected(&mut results, TerminalReason::Completed)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.reason, TerminalReason::Completed);
        assert_eq!(
            result.delivery,
            if requested.contains(&"fail") {
                RunDelivery::Failed
            } else {
                RunDelivery::NoOutput
            }
        );
    }
}

#[test]
fn recovery_keeps_origin_and_epoch_qualified_decimal_ids_without_legacy_success() {
    let (mut ledger, key, mut d, _, _) = setup(
        recovery_graph(),
        targets(&["recover"]),
        Value::Null,
        policy(),
    );
    let mut results = ledger.result_collector(&key).unwrap();
    let failed = execute(&ledger, &key, &mut d, "fail", BTreeMap::new());
    let errors = failed.error_outputs["out"].clone();
    results.record("fail", failed).unwrap();
    let recovered = execute(
        &ledger,
        &key,
        &mut d,
        "recover",
        [("in".into(), ErrorAwareValue::Error(errors))].into(),
    );
    results.record("recover", recovered).unwrap();
    let result = ledger
        .finish_collected(&mut results, TerminalReason::Completed)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result.delivery, RunDelivery::Recovered);
    assert_eq!(result.causes.len(), 1);
    assert_eq!(result.recoveries.len(), 1);
    assert_eq!(
        result.causes[0]["id"],
        json!({"sessionEpoch":key.session_epoch,"runId":key.run_id.to_string(),"planVersion":"3","nodeId":"fail"})
    );
    assert_eq!(
        result.causes[0]["diagnostic"]["error"]["code"],
        "source-failed"
    );
    assert_eq!(result.recoveries[0]["causes"][0], result.causes[0]["id"]);
    assert_eq!(
        result
            .ports
            .iter()
            .find(|p| p.endpoint == Endpoint::new("recover", "unused"))
            .unwrap()
            .value["status"],
        "no-output"
    );
}

#[test]
fn cancellation_keeps_observed_failure_and_discards_uncommitted_data() {
    let (mut ledger, key, mut d, _, _) = setup(
        recovery_graph(),
        targets(&["recover"]),
        Value::Null,
        policy(),
    );
    let mut results = ledger.result_collector(&key).unwrap();
    let failed = execute(&ledger, &key, &mut d, "fail", BTreeMap::new());
    results.record("fail", failed).unwrap();
    let snapshot = ledger
        .finish_collected(&mut results, TerminalReason::Cancelled)
        .unwrap();
    let result = snapshot.result.unwrap();
    assert_eq!(result.reason, TerminalReason::Cancelled);
    assert_eq!(result.causes.len(), 1);
    assert!(
        result
            .ports
            .iter()
            .all(|port| port.value == json!({"status":"discarded","reason":"cancelled"}))
    );
    assert_eq!(result.nodes[0]["status"], "failed");
    assert_eq!(result.nodes[1]["status"], "discarded");
}

#[test]
fn no_output_target_has_priority_over_recovered_data_in_both_target_orders() {
    for ports in [["out", "unused"], ["unused", "out"]] {
        let requested = json!(
            ports
                .iter()
                .map(|port| json!({"node":"recover","port":port}))
                .collect::<Vec<_>>()
        );
        let (mut ledger, key, mut d, _, _) =
            setup(recovery_graph(), requested, Value::Null, policy());
        let mut results = ledger.result_collector(&key).unwrap();
        let failed = execute(&ledger, &key, &mut d, "fail", BTreeMap::new());
        let errors = failed.error_outputs["out"].clone();
        results.record("fail", failed).unwrap();
        let recovered = execute(
            &ledger,
            &key,
            &mut d,
            "recover",
            [("in".into(), ErrorAwareValue::Error(errors))].into(),
        );
        results.record("recover", recovered).unwrap();
        let result = ledger
            .finish_collected(&mut results, TerminalReason::Completed)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.delivery, RunDelivery::NoOutput);
        assert_eq!(result.recoveries.len(), 1);
    }
}

#[test]
fn an_error_from_an_unrequested_branch_cannot_be_recorded_as_target_failure() {
    let (mut ledger, key, mut d, _, _) = setup(
        nodes(&["source", "fail"]),
        targets(&["source"]),
        Value::Null,
        policy(),
    );
    let mut results = ledger.result_collector(&key).unwrap();
    let unrelated = execute(&ledger, &key, &mut d, "fail", BTreeMap::new());
    assert_eq!(
        results.record("source", unrelated).unwrap_err().code,
        "invalid-result"
    );
    results
        .record(
            "source",
            execute(&ledger, &key, &mut d, "source", BTreeMap::new()),
        )
        .unwrap();
    assert_eq!(
        ledger
            .finish_collected(&mut results, TerminalReason::Completed)
            .unwrap()
            .result
            .unwrap()
            .delivery,
        RunDelivery::Complete
    );
}

#[test]
fn deadline_projection_preserves_history_and_wins_over_uncommitted_success_preview() {
    for failure in [false, true] {
        let graph = if failure {
            recovery_graph()
        } else {
            nodes(&["source"])
        };
        let requested = if failure {
            targets(&["recover"])
        } else {
            targets(&["source"])
        };
        let (mut ledger, key, mut d, _, _) = setup(graph, requested, json!(10), policy());
        let mut results = ledger.result_collector(&key).unwrap();
        let name = if failure { "fail" } else { "source" };
        results
            .record(name, execute(&ledger, &key, &mut d, name, BTreeMap::new()))
            .unwrap();
        if !failure {
            assert_eq!(
                results.preview(TerminalReason::Completed).unwrap().delivery,
                RunDelivery::Complete
            );
        }
        ledger
            .advance_clock_with_results(Duration::from_millis(10), &mut [&mut results])
            .unwrap();
        let snapshot = ledger.get_run(&key).unwrap();
        assert_eq!(snapshot.elapsed_ms, 10);
        let result = snapshot.result.unwrap();
        assert_eq!(result.reason, TerminalReason::Expired);
        assert_eq!(result.causes.len(), usize::from(failure));
        assert!(
            result
                .ports
                .iter()
                .all(|port| port.value["status"] == "discarded")
        );
        assert_eq!(
            results.preview(TerminalReason::Completed).unwrap().delivery,
            RunDelivery::Expired
        );
        ledger.advance_clock(Duration::from_millis(11)).unwrap();
        assert_eq!(ledger.get_run(&key).unwrap().elapsed_ms, 10);
    }
}

#[test]
fn foreign_run_diagnostics_are_rejected_without_consuming_result_slot() {
    let (ledger_a, key_a, mut d_a, _, _) =
        setup(nodes(&["fail"]), targets(&["fail"]), Value::Null, policy());
    let (mut ledger_b, key_b, mut d_b, _, _) =
        setup(nodes(&["fail"]), targets(&["fail"]), Value::Null, policy());
    let mut results = ledger_b.result_collector(&key_b).unwrap();
    let foreign = execute(&ledger_a, &key_a, &mut d_a, "fail", BTreeMap::new());
    assert_eq!(
        results.record("fail", foreign).unwrap_err().code,
        "invalid-result"
    );
    results
        .record(
            "fail",
            execute(&ledger_b, &key_b, &mut d_b, "fail", BTreeMap::new()),
        )
        .unwrap();
    assert_eq!(
        ledger_b
            .finish_collected(&mut results, TerminalReason::Completed)
            .unwrap()
            .result
            .unwrap()
            .delivery,
        RunDelivery::Failed
    );
}

#[test]
fn actual_retained_result_limit_summary_seals_the_collector_too() {
    let mut profile = policy();
    profile.max_result_bytes_per_run = 1024;
    let (mut ledger, key, mut d, _, _) =
        setup(nodes(&["large"]), targets(&["large"]), Value::Null, profile);
    let mut results = ledger.result_collector(&key).unwrap();
    results
        .record(
            "large",
            execute(&ledger, &key, &mut d, "large", BTreeMap::new()),
        )
        .unwrap();
    assert_eq!(
        results.preview(TerminalReason::Completed).unwrap().delivery,
        RunDelivery::Complete
    );
    let snapshot = ledger
        .finish_collected(&mut results, TerminalReason::Completed)
        .unwrap();
    let result = snapshot.result.unwrap();
    assert_eq!(result.reason, TerminalReason::Failed);
    assert_eq!(result.error.as_ref().unwrap().code, "result-limit");
    assert!(result.ports.is_empty());
    assert_eq!(
        serde_json::to_value(results.preview(TerminalReason::Completed).unwrap()).unwrap(),
        serde_json::to_value(result).unwrap()
    );
}

#[test]
fn rust_valid_but_json_too_deep_output_becomes_explicit_report_failure() {
    let (mut ledger, key, mut d, _, _) =
        setup(nodes(&["deep"]), targets(&["deep"]), Value::Null, policy());
    let mut results = ledger.result_collector(&key).unwrap();
    results
        .record(
            "deep",
            execute(&ledger, &key, &mut d, "deep", BTreeMap::new()),
        )
        .unwrap();
    let result = ledger
        .finish_collected(&mut results, TerminalReason::Completed)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result.error.as_ref().unwrap().code, "report-failed");
    assert_eq!(result.reason, TerminalReason::Failed);
    assert!(result.ports.is_empty());
}

#[test]
fn discarded_target_and_premature_expiry_are_not_success_or_clock_progress() {
    let (mut ledger, key, _, _, _) = setup(
        nodes(&["source"]),
        targets(&["source"]),
        json!(10),
        policy(),
    );
    let mut results = ledger.result_collector(&key).unwrap();
    results.discard_unneeded("source").unwrap();
    assert_eq!(
        results.preview(TerminalReason::Completed).unwrap_err().code,
        "run-pending"
    );
    assert_eq!(
        ledger
            .finish_collected(&mut results, TerminalReason::Expired)
            .unwrap_err()
            .code,
        "invalid-deadline"
    );
    assert_eq!(ledger.now(), Duration::ZERO);
    assert_eq!(ledger.get_run(&key).unwrap().state, RunPhase::Running);
}
