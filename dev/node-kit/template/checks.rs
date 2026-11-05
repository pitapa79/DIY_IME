// #[path] で読み込む実装の crate:: 参照を、公開コアAPIへ接続する。
// 本体の src/nodes/mod.rs と標準Registryは変更しない。
pub use diy_ime2_core::{Candidate, node};
use diy_ime2_core::{
    Packet, TextMeta,
    builtins::register_builtin_nodes,
    graph::{Edge, Endpoint, ExecutionPlan, Graph, NodeInstance},
    node::{
        CoreError, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RootConstraint, RunContext,
    },
    runtime::{Engine, NodeStatus, ResolvedValue},
};
use serde_json::{Value, json};
use std::sync::Arc;
use subject::{ExampleNode, KIND};

fn context() -> RunContext<'static> {
    RunContext {
        run_id: 1,
        plan_version: 1,
        node_id: "kit",
        input: None,
    }
}
fn data(text: &str) -> OutputValue {
    OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
        text: text.into(),
        meta: None,
    })))
}
fn run(input: OutputValue, config: &Value) -> Result<Outputs, CoreError> {
    ExampleNode.execute(context(), &[("in".into(), input)].into(), config)
}

#[test]
fn contract_cases_execute_the_implementation() {
    let fixture: Value = serde_json::from_str(include_str!("contract.json")).unwrap();
    assert_eq!(fixture["nodeKind"], KIND);
    assert_eq!(fixture["purpose"], "kit-validation-only");
    assert_eq!(
        fixture["defaultConfig"],
        ExampleNode.definition().default_config
    );
    for case in fixture["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let input = match case["input"]["status"].as_str().unwrap() {
            "data" => OutputValue::Data(Arc::new(
                serde_json::from_value(case["input"]["packet"].clone()).unwrap(),
            )),
            "no-output" => OutputValue::NoOutput,
            other => panic!("{id}: unsupported input status {other}"),
        };
        let before = input.clone();
        let result = run(
            input.clone(),
            case.get("config").unwrap_or(&fixture["defaultConfig"]),
        );
        match case["expected"]["status"].as_str().unwrap() {
            "data" => {
                let expected: Packet =
                    serde_json::from_value(case["expected"]["packet"].clone()).unwrap();
                assert_eq!(
                    result.unwrap()["out"],
                    OutputValue::Data(Arc::new(expected)),
                    "{id}"
                );
            }
            "no-output" => assert_eq!(result.unwrap()["out"], OutputValue::NoOutput, "{id}"),
            "error" => assert_eq!(result.unwrap_err().code, case["expected"]["code"], "{id}"),
            other => panic!("{id}: unsupported expected status {other}"),
        }
        assert_eq!(input, before, "{id}: input mutation");
    }
}

#[test]
fn invalid_settings_missing_input_and_metadata_are_rejected() {
    for config in [
        json!(null),
        json!([]),
        json!(0),
        json!(""),
        json!({"extra":true}),
    ] {
        assert_eq!(
            ExampleNode.validate_config(&config).unwrap_err().code,
            "invalid-config"
        );
        assert_eq!(
            run(OutputValue::NoOutput, &config).unwrap_err().code,
            "invalid-config"
        );
    }
    assert_eq!(
        ExampleNode
            .execute(context(), &Inputs::new(), &json!({}))
            .unwrap_err()
            .code,
        "missing-input"
    );
    let invalid = Packet::new(Candidate::Text {
        text: "bad".into(),
        meta: Some(TextMeta {
            reading: None,
            source_span: Some([3, 1]),
        }),
    });
    assert_eq!(
        run(OutputValue::Data(Arc::new(invalid)), &json!({}))
            .unwrap_err()
            .code,
        "invalid-packet"
    );
}

#[test]
fn unicode_meta_immutability_and_repetition() {
    let input = OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
        text: "𠮷e\u{301}👩‍💻\n\t".into(),
        meta: Some(TextMeta {
            reading: Some("よし".into()),
            source_span: Some([0, 2]),
        }),
    })));
    let before = input.clone();
    for _ in 0..3 {
        assert_eq!(run(input.clone(), &json!({})).unwrap()["out"], before);
        assert_eq!(input, before);
    }
}

struct Source {
    kind: &'static str,
    value: OutputValue,
}
impl Node for Source {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            self.kind,
            1,
            vec![],
            vec![
                Port::packet("out")
                    .with_language("ja")
                    .with_root(RootConstraint::Text),
            ],
        )
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        Ok([("out".into(), self.value.clone())].into())
    }
}
fn instance(id: &str, kind: &str) -> NodeInstance {
    NodeInstance {
        id: id.into(),
        kind: kind.into(),
        version: 1,
        config: None,
    }
}
fn edge(from: &str, to: &str) -> Edge {
    Edge {
        from: Endpoint::new(from, "out"),
        to: Endpoint::new(to, "in"),
    }
}

#[test]
fn private_registry_multiple_instances_no_output_and_prepare_validation() {
    let mut registry = Registry::new();
    registry.register(ExampleNode).unwrap();
    registry
        .register(Source {
            kind: "test.source-a",
            value: data("甲"),
        })
        .unwrap();
    registry
        .register(Source {
            kind: "test.source-b",
            value: data("乙"),
        })
        .unwrap();
    registry
        .register(Source {
            kind: "test.empty",
            value: OutputValue::NoOutput,
        })
        .unwrap();
    let mut graph = Graph {
        version: 1,
        nodes: vec![
            instance("source-a", "test.source-a"),
            instance("source-b", "test.source-b"),
            instance("empty", "test.empty"),
            instance("a", KIND),
            instance("b", KIND),
            instance("skipped", KIND),
        ],
        edges: vec![
            edge("source-a", "a"),
            edge("source-b", "b"),
            edge("empty", "skipped"),
        ],
    };
    let mut engine = Engine::new(ExecutionPlan::compile(&graph, &registry).unwrap());
    for _ in 0..3 {
        let report = engine.run().unwrap();
        assert!(report.succeeded());
        assert_eq!(
            report.packet("a", "out").unwrap().root,
            Candidate::Text {
                text: "甲".into(),
                meta: None
            }
        );
        assert_eq!(
            report.packet("b", "out").unwrap().root,
            Candidate::Text {
                text: "乙".into(),
                meta: None
            }
        );
        assert_eq!(report.nodes["skipped"].status, NodeStatus::Skipped);
        assert!(matches!(
            report.nodes["skipped"].outputs["out"],
            ResolvedValue::NoOutput
        ));
    }
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "a")
        .unwrap()
        .config = Some(json!({"extra":1}));
    assert_eq!(
        ExecutionPlan::compile(&graph, &registry)
            .err()
            .unwrap()
            .code,
        "invalid-config"
    );

    let mut standard = Registry::new();
    register_builtin_nodes(&mut standard).unwrap();
    assert!(!standard.definitions().any(|entry| entry.kind == KIND));
}
