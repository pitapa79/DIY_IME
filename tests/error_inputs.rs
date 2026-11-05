use diy_ime2_core::{
    Candidate, Packet,
    error_input::{
        Delivery, ErrorAwareExecution, ErrorAwareInputs, ErrorAwareNode, ErrorAwareValue as V,
        RecoveryClaim,
    },
    graph::{Edge, Endpoint, ExecutionPlan, Graph, NodeInstance},
    node::{
        CoreError, InputMode, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port,
        Registry, RootConstraint, RunContext, RuntimeInput,
    },
    runtime::{Engine, NodeStatus, ResolvedValue},
    session::{Request, Session},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Action =
    dyn Fn(&ErrorAwareInputs, &Value) -> Result<ErrorAwareExecution, CoreError> + Send + Sync;
struct Aware {
    definition: NodeDefinition,
    receive: Vec<String>,
    action: Box<Action>,
    recovery_config: bool,
}
impl ErrorAwareNode for Aware {
    fn definition(&self) -> NodeDefinition {
        self.definition.clone()
    }
    fn error_inputs(&self) -> Vec<String> {
        self.receive.clone()
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if (!self.recovery_config && config == &json!({}))
            || (self.recovery_config
                && config.as_object().is_some_and(|object| {
                    object.len() == 1 && object.get("onNoOutput").is_some_and(Value::is_boolean)
                }))
        {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "invalid test config"))
        }
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        inputs: &ErrorAwareInputs,
        config: &Value,
    ) -> Result<ErrorAwareExecution, CoreError> {
        (self.action)(inputs, config)
    }
}
fn port(id: &str) -> Port {
    Port::packet(id).with_language("ja")
}
fn aware(
    kind: &str,
    inputs: &[(&str, bool, bool)],
    action: impl Fn(&ErrorAwareInputs, &Value) -> Result<ErrorAwareExecution, CoreError>
    + Send
    + Sync
    + 'static,
) -> Aware {
    Aware {
        definition: NodeDefinition::new(
            kind,
            1,
            inputs
                .iter()
                .map(|(id, required, _)| {
                    if *required {
                        InputPort::required(port(id))
                    } else {
                        InputPort::optional(port(id))
                    }
                })
                .collect(),
            vec![port("out")],
        ),
        receive: inputs
            .iter()
            .filter(|(_, _, receive)| *receive)
            .map(|(id, _, _)| (*id).into())
            .collect(),
        action: Box::new(action),
        recovery_config: false,
    }
}
fn output(value: V) -> ErrorAwareExecution {
    ErrorAwareExecution::new([("out".into(), value)].into())
}
fn text(value: &str) -> Arc<Packet> {
    Arc::new(Packet::new(Candidate::Text {
        text: value.into(),
        meta: None,
    }))
}
struct Source {
    kind: String,
    value: OutputValue,
    fail: bool,
    outputs: Vec<Port>,
}
impl Node for Source {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(&self.kind, 1, vec![], self.outputs.clone())
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        if self.fail {
            Err(CoreError::new("test-failure", &self.kind))
        } else {
            Ok(self
                .outputs
                .iter()
                .map(|p| (p.id.clone(), self.value.clone()))
                .collect())
        }
    }
}
fn source(registry: &mut Registry, id: &str, value: OutputValue, fail: bool) {
    registry
        .register(Source {
            kind: id.into(),
            value,
            fail,
            outputs: vec![port("out")],
        })
        .unwrap();
}
fn node(id: &str) -> NodeInstance {
    NodeInstance {
        id: id.into(),
        kind: id.into(),
        version: 1,
        config: None,
    }
}
fn edge(from: &str, to: &str, input: &str) -> Edge {
    Edge {
        from: Endpoint::new(from, "out"),
        to: Endpoint::new(to, input),
    }
}
fn graph(nodes: &[&str], edges: Vec<Edge>) -> Graph {
    Graph {
        version: 1,
        nodes: nodes.iter().map(|id| node(id)).collect(),
        edges,
    }
}
fn engine(g: &Graph, r: &Registry) -> Engine {
    Engine::new(ExecutionPlan::compile(g, r).unwrap())
}
fn compile_code(g: &Graph, r: &Registry) -> String {
    match ExecutionPlan::compile(g, r) {
        Ok(_) => panic!("expected rejection"),
        Err(e) => e.code,
    }
}
fn recover(inputs: &ErrorAwareInputs, config: &Value) -> Result<ErrorAwareExecution, CoreError> {
    let primary = &inputs["primary"];
    let original = &inputs["original"];
    if let V::Data(packet) = primary {
        let mut result = output(V::Data(packet.clone()));
        result
            .dependencies
            .insert("out".into(), vec!["primary".into()]);
        return Ok(result);
    }
    if matches!(primary, V::NoOutput) && config["onNoOutput"] == false {
        return Ok(output(V::NoOutput));
    }
    match (primary, original) {
        (_, V::Data(packet)) => {
            let mut result = output(V::Data(packet.clone()));
            result
                .dependencies
                .insert("out".into(), vec!["original".into()]);
            if let V::Error(errors) = primary {
                result.recoveries.push(RecoveryClaim {
                    input: "primary".into(),
                    output: "out".into(),
                    causes: errors
                        .causes()
                        .iter()
                        .map(|cause| cause.id.clone())
                        .collect(),
                });
            }
            Ok(result)
        }
        (V::Error(e), V::Error(f)) => Ok(output(V::Error(e.merge(f)))),
        (V::Error(e), _) | (_, V::Error(e)) => Ok(output(V::Error(e.clone()))),
        _ => Ok(output(V::NoOutput)),
    }
}
fn recovery() -> Aware {
    let mut n = aware(
        "recover",
        &[("primary", false, true), ("original", false, true)],
        recover,
    );
    n.recovery_config = true;
    n.definition.default_config = json!({"onNoOutput":false});
    n
}

#[test]
fn formal_recovery_fixture_exercises_the_boundary_not_a_product_node() {
    let fixtures: Value = serde_json::from_str(include_str!("contracts/recovery.json")).unwrap();
    assert_eq!(fixtures["kind"], "text.recover-original");
    for case in fixtures["cases"].as_array().unwrap() {
        let mut registry = Registry::new();
        let mut names = vec![];
        let mut edges = vec![];
        let mut expected_packets = std::collections::BTreeMap::new();
        for id in ["primary", "original"] {
            let value = &case["inputs"][id];
            if value["status"] == "missing" {
                continue;
            }
            let out = if value["status"] == "data" {
                let packet = Arc::new(
                    Packet::from_json(&serde_json::to_string(&value["packet"]).unwrap()).unwrap(),
                );
                expected_packets.insert(id, packet.clone());
                OutputValue::Data(packet)
            } else {
                OutputValue::NoOutput
            };
            source(&mut registry, id, out, value["status"] == "error");
            names.push(id);
            edges.push(edge(id, "recover", id));
        }
        registry.register_error_aware(recovery()).unwrap();
        names.push("recover");
        let mut g = graph(&names, edges);
        g.nodes.last_mut().unwrap().config = Some(case["config"].clone());
        let report = engine(&g, &registry)
            .run_with_targets(&[Endpoint::new("recover", "out")], None)
            .unwrap();
        let expected = &case["expected"];
        let result = &report.report.nodes["recover"];
        match expected["status"].as_str().unwrap() {
            "data" => {
                let selected = expected["sameAs"].as_str().unwrap();
                assert!(
                    Arc::ptr_eq(
                        report.report.packet("recover", "out").unwrap(),
                        &expected_packets[selected]
                    ),
                    "{}",
                    case["id"]
                );
            }
            "no-output" => assert!(matches!(result.outputs["out"], ResolvedValue::NoOutput)),
            "error" => {
                assert!(matches!(result.outputs["out"], ResolvedValue::Error(_)));
                assert_eq!(
                    result.error_outputs["out"].causes().len(),
                    expected["causes"].as_u64().unwrap() as usize
                );
            }
            _ => panic!("unknown fixture status"),
        }
        assert_eq!(
            result.recoveries.len(),
            expected["recoveries"].as_u64().unwrap() as usize,
            "{}",
            case["id"]
        );
        assert_eq!(
            serde_json::to_value(report.delivery).unwrap(),
            expected["delivery"],
            "{}",
            case["id"]
        );
        assert_eq!(
            report.report.succeeded(),
            expected["legacySucceeded"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
        for (id, packet) in expected_packets {
            assert!(Arc::ptr_eq(
                report.report.packet(id, "out").unwrap(),
                &packet
            ));
        }
    }
}

#[test]
fn opt_in_required_error_runs_but_required_no_output_skips_and_block_wins() {
    for required in [false, true] {
        for (status, expected) in [
            ("error", NodeStatus::Executed),
            (
                "no-output",
                if required {
                    NodeStatus::Skipped
                } else {
                    NodeStatus::Executed
                },
            ),
            ("data", NodeStatus::Executed),
        ] {
            let mut r = Registry::new();
            source(
                &mut r,
                "source",
                if status == "data" {
                    OutputValue::Data(text(""))
                } else {
                    OutputValue::NoOutput
                },
                status == "error",
            );
            r.register_error_aware(aware("aware", &[("in", required, true)], |inputs, _| {
                Ok(output(inputs["in"].clone()))
            }))
            .unwrap();
            let g = graph(&["source", "aware"], vec![edge("source", "aware", "in")]);
            let report = engine(&g, &r).run().unwrap();
            assert_eq!(report.nodes["aware"].status, expected);
        }
    }
    let mut r = Registry::new();
    source(&mut r, "a", OutputValue::NoOutput, true);
    source(&mut r, "b", OutputValue::NoOutput, true);
    source(&mut r, "none", OutputValue::NoOutput, false);
    r.register_error_aware(aware(
        "aware",
        &[
            ("receive", true, true),
            ("block", false, false),
            ("required", true, true),
        ],
        |_, _| panic!("must not execute"),
    ))
    .unwrap();
    let mut g = graph(
        &["a", "b", "none", "aware"],
        vec![
            edge("a", "aware", "receive"),
            edge("b", "aware", "block"),
            edge("none", "aware", "required"),
        ],
    );
    let report = engine(&g, &r).run().unwrap();
    assert_eq!(report.nodes["aware"].status, NodeStatus::Blocked);
    assert_eq!(
        report.nodes["aware"]
            .input_errors
            .as_ref()
            .unwrap()
            .causes()
            .len(),
        2
    );
    g.edges.retain(|edge| edge.to.port != "block");
    let report = engine(&g, &r).run().unwrap();
    assert_eq!(report.nodes["aware"].status, NodeStatus::Skipped);
    assert_eq!(
        report.nodes["aware"]
            .input_errors
            .as_ref()
            .unwrap()
            .causes()
            .len(),
        1
    );
    assert!(report.nodes["aware"].recoveries.is_empty());
}

#[test]
fn multiple_causes_fanout_and_repropagation_keep_origins_and_each_hop() {
    let mut r = Registry::new();
    source(&mut r, "a", OutputValue::NoOutput, true);
    source(&mut r, "b", OutputValue::NoOutput, true);
    for id in ["left", "right"] {
        r.register_error_aware(aware(id, &[("in", true, true)], |inputs, _| {
            Ok(output(inputs["in"].clone()))
        }))
        .unwrap();
    }
    r.register_error_aware(aware(
        "merge",
        &[("a", true, true), ("b", true, true), ("c", true, true)],
        |inputs, _| {
            let (V::Error(a), V::Error(b), V::Error(c)) =
                (&inputs["a"], &inputs["b"], &inputs["c"])
            else {
                panic!()
            };
            Ok(output(V::Error(a.merge(b).merge(c))))
        },
    ))
    .unwrap();
    let g = graph(
        &["a", "b", "left", "right", "merge"],
        vec![
            edge("a", "left", "in"),
            edge("a", "right", "in"),
            edge("left", "merge", "a"),
            edge("right", "merge", "b"),
            edge("b", "merge", "c"),
        ],
    );
    let report = engine(&g, &r)
        .run_with_targets(&[Endpoint::new("merge", "out")], None)
        .unwrap();
    let errors = &report.report.nodes["merge"].error_outputs["out"];
    assert_eq!(errors.causes().len(), 2);
    assert_eq!(errors.causes()[0].id.node_id, "a");
    assert_eq!(errors.causes()[0].paths.len(), 4);
    assert_eq!(errors.causes()[1].paths.len(), 1);
    for cause in errors.causes() {
        assert_eq!(cause.diagnostic.port, None);
        assert_eq!(cause.id.run_id, report.report.run_id);
        assert_eq!(cause.diagnostic.error.code, "test-failure");
    }
    assert_eq!(report.delivery, Delivery::Failed);
    assert!(!report.report.succeeded());
}

#[test]
fn stale_errors_are_rejected_and_new_failure_preserves_input_history() {
    let mut r = Registry::new();
    source(&mut r, "source", OutputValue::NoOutput, true);
    let previous = Arc::new(Mutex::new(None));
    let cache = previous.clone();
    r.register_error_aware(aware("cached", &[("in", true, true)], move |inputs, _| {
        let V::Error(current) = &inputs["in"] else {
            panic!()
        };
        let mut saved = cache.lock().unwrap();
        let result = saved.get_or_insert_with(|| current.clone()).clone();
        Ok(output(V::Error(result)))
    }))
    .unwrap();
    let g = graph(&["source", "cached"], vec![edge("source", "cached", "in")]);
    let mut e = engine(&g, &r);
    assert_eq!(
        e.run().unwrap().nodes["cached"].status,
        NodeStatus::Executed
    );
    let report = e
        .run_with_targets(&[Endpoint::new("cached", "out")], None)
        .unwrap();
    assert_eq!(report.report.nodes["cached"].status, NodeStatus::Failed);
    assert_eq!(
        report.report.nodes["cached"]
            .diagnostic
            .as_ref()
            .unwrap()
            .error
            .code,
        "invalid-error-propagation"
    );
    let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(json["causes"].as_array().unwrap().len(), 2);
}

#[test]
fn invalid_recovery_and_dependency_claims_fail_atomically() {
    for bad in [
        "input",
        "output",
        "empty",
        "duplicate",
        "foreign",
        "double-claim",
        "no-output",
        "error-output",
        "dep-output",
        "dep-input",
        "dep-error",
        "dep-duplicate",
    ] {
        let mut r = Registry::new();
        source(&mut r, "error", OutputValue::NoOutput, true);
        source(&mut r, "original", OutputValue::Data(text("原文")), false);
        r.register_error_aware(aware(
            "bad",
            &[("error", false, true), ("original", false, true)],
            move |inputs, _| {
                let V::Error(errors) = &inputs["error"] else {
                    panic!()
                };
                let mut result = output(inputs["original"].clone());
                let mut claim = RecoveryClaim {
                    input: "error".into(),
                    output: "out".into(),
                    causes: errors.causes().iter().map(|c| c.id.clone()).collect(),
                };
                match bad {
                    "input" => claim.input = "original".into(),
                    "output" => claim.output = "unknown".into(),
                    "empty" => claim.causes.clear(),
                    "duplicate" => claim.causes.push(claim.causes[0].clone()),
                    "foreign" => claim.causes[0].node_id = "foreign".into(),
                    "double-claim" => result.recoveries.push(claim.clone()),
                    "no-output" => result
                        .outputs
                        .insert("out".into(), V::NoOutput)
                        .map(|_| ())
                        .unwrap(),
                    "error-output" => result
                        .outputs
                        .insert("out".into(), V::Error(errors.clone()))
                        .map(|_| ())
                        .unwrap(),
                    "dep-output" => {
                        result.dependencies.insert("unknown".into(), vec![]);
                    }
                    "dep-input" => {
                        result
                            .dependencies
                            .insert("out".into(), vec!["unknown".into()]);
                    }
                    "dep-error" => {
                        result
                            .dependencies
                            .insert("out".into(), vec!["error".into()]);
                    }
                    "dep-duplicate" => {
                        result
                            .dependencies
                            .insert("out".into(), vec!["original".into(), "original".into()]);
                    }
                    _ => unreachable!(),
                }
                result.recoveries.push(claim);
                Ok(result)
            },
        ))
        .unwrap();
        let g = graph(
            &["error", "original", "bad"],
            vec![
                edge("error", "bad", "error"),
                edge("original", "bad", "original"),
            ],
        );
        let report = engine(&g, &r).run().unwrap();
        assert_eq!(report.nodes["bad"].status, NodeStatus::Failed, "{bad}");
        assert_eq!(
            report.nodes["bad"].diagnostic.as_ref().unwrap().error.code,
            if bad.starts_with("dep-") {
                "invalid-dependencies"
            } else {
                "invalid-recovery"
            }
        );
        assert!(report.packet("bad", "out").is_none());
        assert!(report.nodes["bad"].recoveries.is_empty());
    }
}

#[test]
fn output_contract_violations_and_panics_are_still_failures() {
    for bad in ["missing", "unknown", "packet", "root", "panic"] {
        let mut r = Registry::new();
        let mut n = aware("bad", &[], move |_, _| match bad {
            "missing" => Ok(ErrorAwareExecution::default()),
            "unknown" => Ok(ErrorAwareExecution::new(
                [("extra".into(), V::NoOutput), ("out".into(), V::NoOutput)].into(),
            )),
            "packet" => Ok(output(V::Data(Arc::new(Packet {
                version: 9,
                root: Candidate::Sequence { items: vec![] },
            })))),
            "root" => Ok(output(V::Data(Arc::new(Packet::new(Candidate::Choice {
                options: vec![],
            }))))),
            _ => panic!("test panic"),
        });
        n.definition.outputs[0].root = RootConstraint::Text;
        r.register_error_aware(n).unwrap();
        let report = engine(&graph(&["bad"], vec![]), &r).run().unwrap();
        assert_eq!(report.nodes["bad"].status, NodeStatus::Failed);
        assert_eq!(
            report.nodes["bad"].diagnostic.as_ref().unwrap().error.code,
            match bad {
                "missing" => "missing-output",
                "unknown" => "unknown-output",
                "packet" => "invalid-packet",
                "root" => "invalid-root",
                _ => "node-panic",
            }
        );
    }
}

#[test]
fn target_delivery_tracks_recovery_and_unselected_failure_separately() {
    let mut r = Registry::new();
    source(&mut r, "error", OutputValue::NoOutput, true);
    source(&mut r, "original", OutputValue::Data(text("かな😀")), false);
    r.register_error_aware(recovery()).unwrap();
    r.register_error_aware(aware("pass", &[("in", true, false)], |inputs, _| {
        Ok(output(inputs["in"].clone()))
    }))
    .unwrap();
    r.register_error_aware(aware("independent", &[("ignored", false, true)], |_, _| {
        let mut result = output(V::Data(text("別の出力")));
        result.dependencies.insert("out".into(), vec![]);
        Ok(result)
    }))
    .unwrap();
    let g = graph(
        &["error", "original", "recover", "pass", "independent"],
        vec![
            edge("error", "recover", "primary"),
            edge("original", "recover", "original"),
            edge("recover", "pass", "in"),
            edge("recover", "independent", "ignored"),
        ],
    );
    let mut e = engine(&g, &r);
    let report = e
        .run_with_targets(&[Endpoint::new("pass", "out")], None)
        .unwrap();
    assert_eq!(report.delivery, Delivery::Recovered);
    assert!(!report.report.succeeded());
    let legacy: Value = serde_json::from_str(&report.report.to_json().unwrap()).unwrap();
    assert!(legacy.get("delivery").is_none());
    assert!(legacy.get("causes").is_none());
    assert_eq!(legacy["nodes"]["error"]["status"], "failed");
    let extended: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(extended["executionControlVersion"], 1);
    assert_eq!(extended["recoveries"].as_array().unwrap().len(), 1);
    assert_eq!(extended["causes"].as_array().unwrap().len(), 1);
    assert_eq!(
        e.run_with_targets(&[Endpoint::new("independent", "out")], None)
            .unwrap()
            .delivery,
        Delivery::Complete
    );
    assert_eq!(
        e.run_with_targets(
            &[Endpoint::new("pass", "out"), Endpoint::new("error", "out")],
            None
        )
        .unwrap()
        .delivery,
        Delivery::Failed
    );
}

#[test]
fn invalid_targets_and_runtime_input_do_not_advance_the_engine() {
    let mut r = Registry::new();
    source(&mut r, "source", OutputValue::Data(text("")), false);
    let mut e = engine(&graph(&["source"], vec![]), &r);
    let report = e.run().unwrap();
    for targets in [
        vec![],
        vec![Endpoint::new("unknown", "out")],
        vec![Endpoint::new("source", "unknown")],
        vec![
            Endpoint::new("source", "out"),
            Endpoint::new("source", "out"),
        ],
    ] {
        assert_eq!(
            e.run_with_targets(&targets, None).err().unwrap().code,
            "invalid-targets"
        );
        assert!(e.is_current(&report));
    }
    let bad = RuntimeInput {
        revision: 0,
        text: "".into(),
        mode: InputMode::Compose,
    };
    assert_eq!(
        e.run_with_targets(&[Endpoint::new("source", "out")], Some(&bad))
            .err()
            .unwrap()
            .code,
        "invalid-runtime-input"
    );
    assert!(e.is_current(&report));
    assert_eq!(e.plan_version(), 1);
}

#[test]
fn registration_and_connection_validation_do_not_bypass_legacy_types() {
    let mut r = Registry::new();
    for receive in [vec!["missing".into()], vec!["in".into(), "in".into()]] {
        let mut n = aware("bad", &[("in", false, true)], |_, _| {
            Ok(output(V::NoOutput))
        });
        n.receive = receive;
        assert_eq!(
            r.register_error_aware(n).unwrap_err().code,
            "invalid-definition"
        );
    }
    r.register_error_aware(aware("aware", &[("in", true, true)], |inputs, _| {
        Ok(output(inputs["in"].clone()))
    }))
    .unwrap();
    assert_eq!(r.definitions().count(), 0);
    let defs = r.error_aware_definitions().collect::<Vec<_>>();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].version, 1);
    assert_eq!(defs[0].error_inputs, vec!["in"]);
    assert_eq!(
        r.register_error_aware(aware("aware", &[], |_, _| Ok(output(V::NoOutput))))
            .unwrap_err()
            .code,
        "duplicate-kind"
    );
    let g = graph(&["aware"], vec![]);
    assert_eq!(compile_code(&g, &r), "missing-input");
    let mut output_port = port("out");
    output_port.language = Some("ja-Latn".into());
    r.register(Source {
        kind: "source".into(),
        value: OutputValue::Data(text("a")),
        fail: false,
        outputs: vec![output_port],
    })
    .unwrap();
    let g = graph(&["source", "aware"], vec![edge("source", "aware", "in")]);
    assert_eq!(compile_code(&g, &r), "incompatible-ports");
    let mut invalid = aware("invalid", &[], |_, _| Ok(output(V::NoOutput)));
    invalid.definition.outputs[0].data_type.version = 99;
    assert_eq!(
        r.register_error_aware(invalid).unwrap_err().code,
        "unsupported-data-type"
    );
}

#[test]
fn session_v2_hides_new_definitions_and_rejects_new_plans_without_state_changes() {
    let mut r = Registry::new();
    source(&mut r, "source", OutputValue::Data(text("a")), false);
    r.register_error_aware(aware("aware", &[], |_, _| Ok(output(V::NoOutput))))
        .unwrap();
    let mut session = Session::new(r);
    let definitions = session.handle(Request::Definitions {}).unwrap();
    assert_eq!(definitions.as_array().unwrap().len(), 1);
    let g = graph(&["source"], vec![]);
    session.handle(Request::Prepare { graph: g }).unwrap();
    let token = session.state().prepared.unwrap().token;
    session.handle(Request::Activate { token }).unwrap();
    let before = serde_json::to_value(session.state()).unwrap();
    let g = graph(&["aware"], vec![]);
    for request in [
        Request::Validate { graph: g.clone() },
        Request::Prepare { graph: g },
    ] {
        assert_eq!(
            session.handle(request).unwrap_err().code,
            "unsupported-capability"
        );
        assert_eq!(serde_json::to_value(session.state()).unwrap(), before);
    }
}

#[test]
fn repeated_runs_and_instances_share_packets_but_never_cause_ids() {
    let mut r = Registry::new();
    source(&mut r, "failure", OutputValue::NoOutput, true);
    let original = text("かな😀");
    source(
        &mut r,
        "original",
        OutputValue::Data(original.clone()),
        false,
    );
    r.register_error_aware(recovery()).unwrap();
    let g = graph(
        &["failure", "original", "recover"],
        vec![
            edge("failure", "recover", "primary"),
            edge("original", "recover", "original"),
        ],
    );
    let mut a = engine(&g, &r);
    let mut b = engine(&g, &r);
    let mut previous_id = None;
    for index in 0..3 {
        let engine = if index == 1 { &mut b } else { &mut a };
        let report = engine
            .run_with_targets(&[Endpoint::new("recover", "out")], None)
            .unwrap();
        assert!(Arc::ptr_eq(
            report.report.packet("recover", "out").unwrap(),
            &original
        ));
        assert_eq!(report.delivery, Delivery::Recovered);
        let id = report.report.nodes["recover"].recoveries[0].causes[0].clone();
        assert_ne!(previous_id, Some(id.clone()));
        previous_id = Some(id);
        assert_eq!(
            original.root,
            Candidate::Text {
                text: "かな😀".into(),
                meta: None
            }
        );
    }
}

#[test]
fn no_output_delivery_has_priority_over_recovered_data_in_either_target_order() {
    let mut r = Registry::new();
    source(&mut r, "error", OutputValue::NoOutput, true);
    source(&mut r, "original", OutputValue::Data(text("a")), false);
    source(&mut r, "none", OutputValue::NoOutput, false);
    r.register_error_aware(recovery()).unwrap();
    let g = graph(
        &["error", "original", "none", "recover"],
        vec![
            edge("error", "recover", "primary"),
            edge("original", "recover", "original"),
        ],
    );
    let mut e = engine(&g, &r);
    for targets in [
        vec![
            Endpoint::new("none", "out"),
            Endpoint::new("recover", "out"),
        ],
        vec![
            Endpoint::new("recover", "out"),
            Endpoint::new("none", "out"),
        ],
    ] {
        assert_eq!(
            e.run_with_targets(&targets, None).unwrap().delivery,
            Delivery::NoOutput
        );
    }
}

#[test]
fn malformed_recovery_config_and_invalid_outputless_failure_are_observable() {
    let mut r = Registry::new();
    r.register_error_aware(recovery()).unwrap();
    for config in [
        json!({}),
        json!({"onNoOutput": null}),
        json!({"onNoOutput": 1}),
        json!({"onNoOutput": false, "extra": true}),
        json!(null),
    ] {
        let mut g = graph(&["recover"], vec![]);
        g.nodes[0].config = Some(config);
        assert_eq!(compile_code(&g, &r), "invalid-config");
    }
    r.register(Source {
        kind: "failed-sink".into(),
        value: OutputValue::NoOutput,
        fail: true,
        outputs: vec![],
    })
    .unwrap();
    source(&mut r, "ok", OutputValue::Data(text("")), false);
    let g = graph(&["failed-sink", "ok"], vec![]);
    let report = engine(&g, &r)
        .run_with_targets(&[Endpoint::new("ok", "out")], None)
        .unwrap();
    assert_eq!(report.delivery, Delivery::Complete);
    assert!(!report.report.succeeded());
    let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(json["causes"].as_array().unwrap().len(), 1);
    assert_eq!(json["causes"][0]["id"]["nodeId"], "failed-sink");
}
