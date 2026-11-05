//! Hostile mock outputs through the real synchronous Engine.
use diy_ime2_core::{
    Candidate, ChoiceOption, Packet, TextMeta,
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputMode, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port,
        Registry, RootConstraint, RunContext, RuntimeInput,
    },
    runtime::{Engine, NodeStatus, ResolvedValue},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn data(root: Candidate) -> OutputValue {
    OutputValue::Data(Arc::new(Packet::new(root)))
}
fn text(value: &str) -> Candidate {
    Candidate::Text {
        text: value.into(),
        meta: None,
    }
}
struct Producer {
    attack: &'static str,
    calls: Arc<AtomicUsize>,
}
impl Node for Producer {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.producer",
            1,
            vec![],
            vec![
                Port::packet("public").with_root(RootConstraint::Text),
                Port::packet("other").with_root(RootConstraint::Text),
            ],
        )
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut outputs: Outputs = [
            ("public".into(), data(text("must-not-be-delivered"))),
            ("other".into(), data(text("normal"))),
        ]
        .into();
        match self.attack {
            "missing" => {
                outputs.remove("other");
            }
            "unknown" => {
                outputs.insert("undeclared".into(), data(text("canary")));
            }
            "version" => {
                outputs.insert(
                    "other".into(),
                    OutputValue::Data(Arc::new(Packet {
                        version: 99,
                        root: text("canary"),
                    })),
                );
            }
            "root" => {
                outputs.insert("other".into(), data(Candidate::Sequence { items: vec![] }));
            }
            "span" => {
                outputs.insert(
                    "other".into(),
                    data(Candidate::Text {
                        text: "canary".into(),
                        meta: Some(TextMeta {
                            reading: None,
                            source_span: Some([2, 1]),
                        }),
                    }),
                );
            }
            "nan" => {
                outputs.insert(
                    "other".into(),
                    data(Candidate::Choice {
                        options: vec![ChoiceOption {
                            value: text("canary"),
                            score: Some(f64::NAN),
                        }],
                    }),
                );
            }
            "panic" => panic!("fake producer panic"),
            _ => {
                outputs.insert("public".into(), data(text(self.attack)));
            }
        }
        Ok(outputs)
    }
}
struct Sink(Arc<AtomicUsize>);
impl Node for Sink {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.sink",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn execute(&self, _: RunContext<'_>, inputs: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok([("out".into(), inputs["in"].clone())].into())
    }
}
fn setup(attack: &'static str) -> (Engine, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let produces = Arc::new(AtomicUsize::new(0));
    let consumes = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new();
    registry
        .register(Producer {
            attack,
            calls: produces.clone(),
        })
        .unwrap();
    registry.register(Sink(consumes.clone())).unwrap();
    let graph = Graph::from_json(
        &json!({"version":1,"nodes":[
        {"id":"producer","kind":"test.producer","version":1},
        {"id":"sink","kind":"test.sink","version":1}
    ],"edges":[{"from":{"node":"producer","port":"public"},"to":{"node":"sink","port":"in"}}]})
        .to_string(),
    )
    .unwrap();
    (
        Engine::new(ExecutionPlan::compile(&graph, &registry).unwrap()),
        produces,
        consumes,
    )
}
#[test]
fn a_bad_sibling_output_rejects_all_data_and_never_runs_the_downstream_node() {
    for (attack, code) in [
        ("missing", "missing-output"),
        ("unknown", "unknown-output"),
        ("version", "invalid-packet"),
        ("root", "invalid-root"),
        ("span", "invalid-packet"),
        ("nan", "invalid-packet"),
        ("panic", "node-panic"),
    ] {
        let (mut engine, produces, consumes) = setup(attack);
        let report = engine.run().unwrap();
        assert!(!report.succeeded(), "{attack}");
        assert_eq!(report.nodes["producer"].status, NodeStatus::Failed);
        assert_eq!(
            report.nodes["producer"]
                .diagnostic
                .as_ref()
                .unwrap()
                .error
                .code,
            code
        );
        assert!(
            report.nodes["producer"]
                .outputs
                .values()
                .all(|v| matches!(v, ResolvedValue::Error(_)))
        );
        assert!(report.packet("producer", "public").is_none());
        assert_eq!(report.nodes["sink"].status, NodeStatus::Blocked);
        assert_eq!(produces.load(Ordering::SeqCst), 1);
        assert_eq!(consumes.load(Ordering::SeqCst), 0);
        let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(
            json["nodes"]["producer"]["outputs"]["public"]["status"],
            "error"
        );
    }
}
#[test]
fn control_html_and_shell_like_text_remain_literal_data_across_engine_and_json() {
    let payload = "<script>canary</script> $(canary) & canary\u{0}\u{1b}[31m\u{202e}日本語😀";
    let (mut engine, _, consumes) = setup(payload);
    let report = engine.run().unwrap();
    assert!(report.succeeded());
    let packet = report.packet("sink", "out").unwrap();
    assert_eq!(packet.root, text(payload));
    let wire = packet.to_json().unwrap();
    assert!(!wire.contains('\u{0}'));
    assert_eq!(Packet::from_json(&wire).unwrap().root, text(payload));
    assert_eq!(consumes.load(Ordering::SeqCst), 1);
}
#[test]
fn oversized_or_invalid_input_is_rejected_before_any_mock_node_runs() {
    let (mut engine, produces, consumes) = setup("normal");
    for input in [
        RuntimeInput {
            revision: 1,
            text: "😀".repeat(4097),
            mode: InputMode::Compose,
        },
        RuntimeInput {
            revision: 0,
            text: "canary".into(),
            mode: InputMode::Finalize,
        },
    ] {
        assert_eq!(
            engine.run_with_input(&input).unwrap_err().code,
            "invalid-runtime-input"
        );
        assert_eq!(engine.plan_version(), 1);
    }
    assert_eq!(produces.load(Ordering::SeqCst), 0);
    assert_eq!(consumes.load(Ordering::SeqCst), 0);
    assert!(engine.run().unwrap().succeeded());
}
