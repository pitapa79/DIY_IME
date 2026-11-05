#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use std::sync::Arc;
use test_nodes::register_demo_nodes;

use diy_ime2_core::{
    Candidate, Packet,
    graph::{Edge, Endpoint, ExecutionPlan, Graph, NodeInstance},
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RootConstraint, RunContext,
    },
    runtime::{Engine, NodeStatus, ResolvedValue},
};
use serde_json::{Value, json};

fn registry() -> Registry {
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    registry
}
fn instance(id: &str, kind: &str) -> NodeInstance {
    NodeInstance {
        id: id.into(),
        kind: kind.into(),
        version: 1,
        config: None,
    }
}
fn source(id: &str, text: &str) -> NodeInstance {
    let mut node = instance(id, "demo.packet");
    node.config = Some(json!({"packet":{"version":1,"root":{"type":"text","text":text}}}));
    node
}
fn edge(from: &str, output: &str, to: &str, input: &str) -> Edge {
    Edge {
        from: Endpoint::new(from, output),
        to: Endpoint::new(to, input),
    }
}
fn graph(nodes: Vec<NodeInstance>, edges: Vec<Edge>) -> Graph {
    Graph {
        version: 1,
        nodes,
        edges,
    }
}
fn engine(graph: &Graph, registry: &Registry) -> Engine {
    Engine::new(ExecutionPlan::compile(graph, registry).unwrap())
}
fn text(packet: &Packet) -> &str {
    let Candidate::Text { text, .. } = &packet.root else {
        panic!("expected Text")
    };
    text
}
fn compile_error(graph: &Graph, registry: &Registry) -> String {
    match ExecutionPlan::compile(graph, registry) {
        Ok(_) => panic!("expected failure"),
        Err(error) => error.code,
    }
}

struct FunctionNode {
    definition: NodeDefinition,
    run: fn(&Inputs) -> Result<Outputs, CoreError>,
}
impl Node for FunctionNode {
    fn definition(&self) -> NodeDefinition {
        self.definition.clone()
    }
    fn execute(&self, _: RunContext<'_>, inputs: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        (self.run)(inputs)
    }
}
fn passthrough(inputs: &Inputs) -> Result<Outputs, CoreError> {
    Ok([("out".into(), inputs["in"].clone())].into())
}
fn packet_output(packet: Packet) -> Outputs {
    [("out".into(), OutputValue::Data(Arc::new(packet)))].into()
}

#[test]
fn compiles_dependency_order_and_shares_fanout_without_mutation() {
    let registry = registry();
    let graph = graph(
        vec![
            instance("upper", "demo.uppercase"),
            instance("copy", "demo.identity"),
            source("input", "straße𠮷"),
        ],
        vec![
            edge("input", "out", "upper", "in"),
            edge("input", "out", "copy", "in"),
        ],
    );
    let plan = ExecutionPlan::compile(&graph, &registry).unwrap();
    assert_eq!(plan.node_order().next(), Some("input"));
    drop(registry); // 計画がノードの実体と設定を所有している。
    let report = Engine::new(plan).run().unwrap();
    assert!(report.succeeded());
    assert_eq!(text(report.packet("upper", "out").unwrap()), "STRASSE𠮷");
    assert_eq!(text(report.packet("input", "out").unwrap()), "straße𠮷");
    assert!(Arc::ptr_eq(
        report.packet("input", "out").unwrap(),
        report.packet("copy", "out").unwrap()
    ));
}

#[test]
fn no_output_skips_required_input_but_reaches_optional_input() {
    let mut registry = registry();
    registry
        .register(FunctionNode {
            definition: NodeDefinition::new(
                "test.optional",
                1,
                vec![InputPort::optional(Port::packet("in"))],
                vec![Port::packet("out")],
            ),
            run: passthrough,
        })
        .unwrap();
    let graph = graph(
        vec![
            source("input", ""),
            instance("branch", "demo.branch"),
            instance("required", "demo.identity"),
            instance("optional", "test.optional"),
            instance("unconnected", "test.optional"),
        ],
        vec![
            edge("input", "out", "branch", "in"),
            edge("branch", "no", "required", "in"),
            edge("branch", "no", "optional", "in"),
        ],
    );
    let report = engine(&graph, &registry).run().unwrap();
    assert_eq!(report.nodes["required"].status, NodeStatus::Skipped);
    assert_eq!(report.nodes["optional"].status, NodeStatus::Executed);
    assert_eq!(report.nodes["unconnected"].status, NodeStatus::Executed);
    assert!(matches!(
        report.nodes["optional"].outputs["out"],
        ResolvedValue::NoOutput
    ));
    assert!(report.packet("branch", "yes").is_some()); // 空文字は NoOutput ではない。
}

#[test]
fn multiple_ports_from_one_upstream_node_count_as_one_dependency() {
    let mut registry = registry();
    registry
        .register(FunctionNode {
            definition: NodeDefinition::new(
                "test.merge",
                1,
                vec![
                    InputPort::required(Port::packet("in")),
                    InputPort::optional(Port::packet("other")),
                ],
                vec![Port::packet("out")],
            ),
            run: |inputs| {
                assert!(matches!(inputs["other"], OutputValue::NoOutput));
                passthrough(inputs)
            },
        })
        .unwrap();
    let graph = graph(
        vec![
            source("input", "a"),
            instance("branch", "demo.branch"),
            instance("merge", "test.merge"),
        ],
        vec![
            edge("input", "out", "branch", "in"),
            edge("branch", "yes", "merge", "in"),
            edge("branch", "no", "merge", "other"),
        ],
    );
    let report = engine(&graph, &registry).run().unwrap();
    assert_eq!(text(report.packet("merge", "out").unwrap()), "a");
}

#[test]
fn upstream_errors_preserve_origin_and_do_not_stop_independent_nodes() {
    let mut registry = registry();
    registry
        .register(FunctionNode {
            definition: NodeDefinition::new("test.fail", 1, vec![], vec![Port::packet("out")]),
            run: |_| {
                Err(CoreError::new(
                    "dictionary-missing",
                    "cannot read dictionary",
                ))
            },
        })
        .unwrap();
    let graph = graph(
        vec![
            instance("failure", "test.fail"),
            instance("downstream", "demo.identity"),
            source("independent", "ok"),
        ],
        vec![edge("failure", "out", "downstream", "in")],
    );
    let report = engine(&graph, &registry).run().unwrap();
    assert!(!report.succeeded());
    assert_eq!(report.nodes["failure"].status, NodeStatus::Failed);
    assert_eq!(report.nodes["downstream"].status, NodeStatus::Blocked);
    let diagnostic = report.nodes["downstream"].diagnostic.as_ref().unwrap();
    assert_eq!(diagnostic.node_id, "failure");
    assert_eq!(diagnostic.run_id, report.run_id);
    assert_eq!(diagnostic.plan_version, report.plan_version);
    assert_eq!(diagnostic.error.code, "dictionary-missing");
    let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(json["succeeded"], false);
    assert_eq!(
        json["nodes"]["downstream"]["outputs"]["out"]["diagnostic"]["nodeId"],
        "failure"
    );
    assert_eq!(
        json["nodes"]["downstream"]["diagnostic"]["runId"],
        report.run_id
    );
    assert_eq!(text(report.packet("independent", "out").unwrap()), "ok");
}

#[test]
fn optional_upstream_errors_take_precedence_over_required_no_output() {
    let mut registry = registry();
    registry
        .register(FunctionNode {
            definition: NodeDefinition::new("test.fail", 1, vec![], vec![Port::packet("out")]),
            run: |_| Err(CoreError::new("failure", "failure")),
        })
        .unwrap();
    registry
        .register(FunctionNode {
            definition: NodeDefinition::new(
                "test.merge",
                1,
                vec![
                    InputPort::required(Port::packet("in")),
                    InputPort::optional(Port::packet("other")),
                ],
                vec![Port::packet("out")],
            ),
            run: |_| panic!("blocked node must not execute"),
        })
        .unwrap();
    let graph = graph(
        vec![
            source("input", "a"),
            instance("branch", "demo.branch"),
            instance("fail", "test.fail"),
            instance("merge", "test.merge"),
        ],
        vec![
            edge("input", "out", "branch", "in"),
            edge("branch", "no", "merge", "in"),
            edge("fail", "out", "merge", "other"),
        ],
    );
    let report = engine(&graph, &registry).run().unwrap();
    assert_eq!(report.nodes["merge"].status, NodeStatus::Blocked);
    assert_eq!(
        report.nodes["merge"].diagnostic.as_ref().unwrap().node_id,
        "fail"
    );
}

#[test]
fn checks_node_output_contracts_and_contains_panics() {
    type Run = fn(&Inputs) -> Result<Outputs, CoreError>;
    let cases: [(&str, Run); 5] = [
        ("missing-output", |_| Ok(Outputs::new())),
        ("unknown-output", |_| {
            Ok([("wrong".into(), OutputValue::NoOutput)].into())
        }),
        ("invalid-packet", |_| {
            Ok(packet_output(Packet {
                version: 2,
                root: Candidate::Text {
                    text: "a".into(),
                    meta: None,
                },
            }))
        }),
        ("invalid-root", |_| {
            Ok(packet_output(Packet::new(Candidate::Choice {
                options: vec![],
            })))
        }),
        ("node-panic", |_| panic!("test panic")),
    ];
    for (expected, run) in cases {
        let mut registry = Registry::new();
        registry
            .register(FunctionNode {
                definition: NodeDefinition::new(
                    "test.output",
                    1,
                    vec![],
                    vec![Port::packet("out").with_root(RootConstraint::Text)],
                ),
                run,
            })
            .unwrap();
        let graph = graph(vec![instance("bad", "test.output")], vec![]);
        let report = engine(&graph, &registry).run().unwrap();
        assert_eq!(report.nodes["bad"].status, NodeStatus::Failed);
        let diagnostic = report.nodes["bad"].diagnostic.as_ref().unwrap();
        assert_eq!(diagnostic.error.code, expected);
        if expected != "node-panic" {
            assert!(diagnostic.port.is_some());
        }
        assert!(matches!(
            report.nodes["bad"].outputs["out"],
            ResolvedValue::Error(_)
        ));
    }
}

#[test]
fn graph_validation_rejects_invalid_connections() {
    let registry = registry();
    let cases = [
        (
            graph(vec![instance("x", "unknown")], vec![]),
            "unknown-kind",
        ),
        (
            graph(vec![source("x", "a"), source("x", "b")], vec![]),
            "duplicate-node",
        ),
        (
            graph(vec![instance("x", "demo.identity")], vec![]),
            "missing-input",
        ),
        (
            graph(
                vec![source("x", "a")],
                vec![edge("missing", "out", "x", "in")],
            ),
            "unknown-node",
        ),
        (
            graph(
                vec![source("x", "a"), instance("y", "demo.identity")],
                vec![edge("x", "no", "y", "in")],
            ),
            "unknown-output",
        ),
        (
            graph(
                vec![source("x", "a"), instance("y", "demo.identity")],
                vec![edge("x", "out", "y", "missing")],
            ),
            "unknown-input",
        ),
        (
            graph(
                vec![
                    source("x", "a"),
                    source("z", "b"),
                    instance("y", "demo.identity"),
                ],
                vec![edge("x", "out", "y", "in"), edge("z", "out", "y", "in")],
            ),
            "multiple-connections",
        ),
        (
            graph(
                vec![instance("x", "demo.identity")],
                vec![edge("x", "out", "x", "in")],
            ),
            "cycle",
        ),
        (
            graph(
                vec![
                    instance("x", "demo.identity"),
                    instance("y", "demo.identity"),
                ],
                vec![edge("x", "out", "y", "in"), edge("y", "out", "x", "in")],
            ),
            "cycle",
        ),
    ];
    for (graph, expected) in cases {
        assert_eq!(compile_error(&graph, &registry), expected);
    }
    let mut wrong_version = graph(vec![], vec![]);
    wrong_version.version = 2;
    assert_eq!(compile_error(&wrong_version, &registry), "graph-version");
}

#[test]
fn static_port_checks_require_guaranteed_language_and_root_compatibility() {
    for (language, root, accepted) in [
        (Some("ja"), RootConstraint::Text, true),
        (Some("en"), RootConstraint::Text, false),
        (None, RootConstraint::Text, false),
        (Some("ja"), RootConstraint::Any, false),
    ] {
        let mut registry = Registry::new();
        let mut port = Port::packet("out").with_root(root);
        port.language = language.map(str::to_owned);
        registry
            .register(FunctionNode {
                definition: NodeDefinition::new("test.source", 1, vec![], vec![port]),
                run: |_| Ok(Outputs::new()),
            })
            .unwrap();
        registry
            .register(FunctionNode {
                definition: NodeDefinition::new(
                    "test.target",
                    1,
                    vec![InputPort::required(
                        Port::packet("in")
                            .with_language("ja")
                            .with_root(RootConstraint::Text),
                    )],
                    vec![],
                ),
                run: |_| Ok(Outputs::new()),
            })
            .unwrap();
        let graph = graph(
            vec![instance("a", "test.source"), instance("b", "test.target")],
            vec![edge("a", "out", "b", "in")],
        );
        assert_eq!(ExecutionPlan::compile(&graph, &registry).is_ok(), accepted);
    }
}

#[test]
fn registry_rejects_invalid_definitions_and_duplicate_kinds() {
    let mut registry = registry();
    assert_eq!(
        registry.register(test_nodes::Identity).unwrap_err().code,
        "duplicate-kind"
    );
    let mut unsupported = Port::packet("out");
    unsupported.data_type.version = 2;
    for definition in [
        NodeDefinition::new("", 1, vec![], vec![]),
        NodeDefinition::new("zero", 0, vec![], vec![]),
        NodeDefinition::new(
            "duplicates",
            1,
            vec![],
            vec![Port::packet("out"), Port::packet("out")],
        ),
        NodeDefinition::new("empty", 1, vec![], vec![Port::packet("")]),
        NodeDefinition::new(
            "language",
            1,
            vec![],
            vec![Port::packet("out").with_language("")],
        ),
        NodeDefinition::new("unsupported", 1, vec![], vec![unsupported]),
    ] {
        assert!(
            registry
                .register(FunctionNode {
                    definition,
                    run: |_| Ok(Outputs::new())
                })
                .is_err()
        );
    }
}

#[test]
fn invalid_settings_are_rejected_before_execution() {
    let registry = registry();
    for (kind, config) in [
        (
            "demo.packet",
            json!({"packet":{"version":2,"root":{"type":"text","text":"a"}}}),
        ),
        ("demo.branch", json!({"take":"invalid"})),
        ("demo.uppercase", json!({"typo":true})),
        ("demo.select-text", json!({"unscored":null})),
    ] {
        let mut node = instance("x", kind);
        node.config = Some(config);
        assert_eq!(
            compile_error(&graph(vec![node], vec![]), &registry),
            "invalid-config"
        );
    }
}

#[test]
fn runs_are_isolated_and_prepared_plans_activate_only_at_explicit_boundaries() {
    let registry = registry();
    let mut original = graph(vec![source("input", "old")], vec![]);
    let mut engine = engine(&original, &registry);
    original.nodes[0] = source("input", "edited"); // 編集元の変更で実行計画は変わらない。
    let old = engine.run().unwrap();
    assert_eq!(text(old.packet("input", "out").unwrap()), "old");
    assert!(engine.is_current(&old));
    engine.prepare(&original, &registry).unwrap();
    let still_old = engine.run().unwrap();
    assert_eq!(text(still_old.packet("input", "out").unwrap()), "old");
    assert!(still_old.run_id > old.run_id);
    assert!(!engine.is_current(&old));
    let invalid = graph(vec![instance("broken", "demo.identity")], vec![]);
    assert!(engine.prepare(&invalid, &registry).is_err());
    assert_eq!(engine.plan_version(), 1);
    assert!(engine.activate_pending().unwrap()); // 先に準備した有効な計画は保持。
    assert!(!engine.activate_pending().unwrap());
    let edited = engine.run().unwrap();
    assert_eq!(text(edited.packet("input", "out").unwrap()), "edited");
    assert_eq!(edited.plan_version, 2);
    assert!(!engine.is_current(&still_old));
    assert!(engine.rollback().unwrap());
    let restored = engine.run().unwrap();
    assert_eq!(text(restored.packet("input", "out").unwrap()), "old");
    assert_eq!(restored.plan_version, 3);
    assert!(!engine.is_current(&old));
    assert!(!engine.rollback().unwrap());
}

#[test]
fn reports_from_another_engine_are_not_current() {
    let registry = registry();
    let graph = graph(vec![source("input", "a")], vec![]);
    let mut first = engine(&graph, &registry);
    let mut second = engine(&graph, &registry);
    let one = first.run().unwrap();
    let two = second.run().unwrap();
    assert_ne!(one.run_id, two.run_id);
    assert!(first.is_current(&one));
    assert!(!first.is_current(&two));
}

#[test]
fn context_matches_report_identifiers() {
    struct ContextNode;
    impl Node for ContextNode {
        fn definition(&self) -> NodeDefinition {
            NodeDefinition::new("test.context", 1, vec![], vec![Port::packet("out")])
        }
        fn execute(
            &self,
            context: RunContext<'_>,
            _: &Inputs,
            _: &Value,
        ) -> Result<Outputs, CoreError> {
            Ok(packet_output(Packet::new(Candidate::Text {
                text: format!(
                    "{}:{}:{}",
                    context.run_id, context.plan_version, context.node_id
                ),
                meta: None,
            })))
        }
    }
    let mut registry = Registry::new();
    registry.register(ContextNode).unwrap();
    let graph = graph(vec![instance("context", "test.context")], vec![]);
    let report = engine(&graph, &registry).run().unwrap();
    assert_eq!(
        text(report.packet("context", "out").unwrap()),
        format!("{}:{}:context", report.run_id, report.plan_version)
    );
}
