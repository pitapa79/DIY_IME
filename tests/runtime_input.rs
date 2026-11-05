#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputMode, Inputs, MAX_INPUT_REVISION, Node, NodeDefinition, OutputValue,
        Outputs, Port, Registry, RunContext, RuntimeInput,
    },
    runtime::Engine,
    session::{Request, Session},
};
use serde_json::{Value, json};
use std::sync::Arc;
use test_nodes::register_builtin_nodes;

fn registry() -> Registry {
    let mut value = Registry::new();
    register_builtin_nodes(&mut value).unwrap();
    value
}
fn graph() -> Graph {
    Graph::from_json(include_str!("../examples/graphs/runtime-input.json")).unwrap()
}
fn input(text: &str, revision: u64) -> RuntimeInput {
    RuntimeInput {
        text: text.into(),
        revision,
        mode: InputMode::Compose,
    }
}
fn session() -> Session {
    let mut session = Session::new(registry());
    send(&mut session, json!({"method":"prepare","graph":graph()})).unwrap();
    let state = serde_json::to_value(session.state()).unwrap();
    send(
        &mut session,
        json!({"method":"activate","token":state["prepared"]["token"]}),
    )
    .unwrap();
    session
}
fn send(session: &mut Session, request: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(request).unwrap();
    session.handle(request)
}

#[test]
fn contract_cases_run_against_the_real_engine_without_changing_plan_state() {
    let fixture: Value =
        serde_json::from_str(include_str!("contracts/runtime_input.json")).unwrap();
    let mut session = session();
    for case in fixture["cases"].as_array().unwrap() {
        let before = serde_json::to_value(session.state()).unwrap();
        let mut request = json!({"method":"run","expectedPlanVersion":case.get("expectedPlanVersion").unwrap_or(&json!(1))});
        if let Some(value) = case.get("input") {
            request["input"] = value.clone();
        }
        let outcome = send(&mut session, request);
        let expected = &case["expected"];
        match expected["status"].as_str().unwrap() {
            "data" => {
                let report = outcome.unwrap();
                assert_eq!(report["succeeded"], true, "{}", case["id"]);
                assert_eq!(report["planVersion"], 1);
                assert_eq!(report["inputRevision"], expected["inputRevision"]);
                assert_eq!(
                    report["nodes"]["input"]["outputs"]["out"]["packet"],
                    expected["packet"]
                );
                assert_eq!(
                    report["nodes"]["result"]["outputs"]["out"]["packet"],
                    expected["packet"]
                );
            }
            "error" => {
                let report = outcome.unwrap();
                assert_eq!(report["succeeded"], false);
                assert_eq!(
                    report["nodes"]["input"]["diagnostic"]["error"]["code"],
                    expected["code"]
                );
                assert_eq!(report["nodes"]["result"]["status"], "blocked");
                assert!(report.get("inputRevision").is_none());
            }
            "request-error" => assert_eq!(
                outcome.unwrap_err().code,
                expected["code"].as_str().unwrap()
            ),
            other => panic!("unexpected fixture status {other}"),
        }
        assert_eq!(
            serde_json::to_value(session.state()).unwrap(),
            before,
            "{}",
            case["id"]
        );
    }
}

#[test]
fn snapshot_validation_checks_types_unicode_and_safe_integer_limits() {
    let mut valid = input(&"𠮷".repeat(4096), MAX_INPUT_REVISION);
    assert!(valid.validate().is_ok());
    valid.text.push('a');
    assert_eq!(valid.validate().unwrap_err().code, "invalid-runtime-input");
    for invalid in [
        json!(null),
        json!([]),
        json!({"revision":1,"text":"a"}),
        json!({"revision":-1,"text":"a","mode":"compose"}),
        json!({"revision":1.5,"text":"a","mode":"compose"}),
        json!({"revision":MAX_INPUT_REVISION+1,"text":"a","mode":"compose"}),
        json!({"revision":1,"text":null,"mode":"compose"}),
    ] {
        assert_eq!(
            RuntimeInput::from_value(invalid).unwrap_err().code,
            "invalid-runtime-input"
        );
    }
    let mut engine = Engine::new(ExecutionPlan::compile(&graph(), &registry()).unwrap());
    let first = engine.run_with_input(&input("first", 1)).unwrap();
    assert!(engine.run_with_input(&input("invalid", 0)).is_err());
    assert!(engine.is_current(&first));
    assert_eq!(engine.plan_version(), 1);
}

#[test]
fn new_inputs_preserve_pending_plan_and_rollback_and_never_reuse_a_snapshot() {
    let mut session = session();
    let fixed: Graph = Graph::from_json(include_str!("../examples/graphs/hello.json")).unwrap();
    send(&mut session, json!({"method":"prepare","graph":fixed})).unwrap();
    let before = serde_json::to_value(session.state()).unwrap();
    for (revision, text) in [(1, "a"), (2, "ka"), (3, "kya")] {
        let report=send(&mut session,json!({"method":"run","expectedPlanVersion":1,"input":{"revision":revision,"text":text,"mode":"finalize"}})).unwrap();
        assert_eq!(
            report["nodes"]["input"]["outputs"]["out"]["packet"]["root"]["text"],
            text
        );
        assert_eq!(serde_json::to_value(session.state()).unwrap(), before);
    }
    send(
        &mut session,
        json!({"method":"activate","token":before["prepared"]["token"]}),
    )
    .unwrap();
    let fixed_report = send(
        &mut session,
        json!({"method":"run","expectedPlanVersion":2}),
    )
    .unwrap();
    assert_eq!(fixed_report["succeeded"], true);
    assert!(fixed_report.get("inputRevision").is_none());
    let rollback_before = serde_json::to_value(session.state()).unwrap();
    send(&mut session,json!({"method":"run","expectedPlanVersion":2,"input":{"revision":4,"text":"other","mode":"compose"}})).unwrap();
    assert_eq!(
        serde_json::to_value(session.state()).unwrap(),
        rollback_before
    );
    send(
        &mut session,
        json!({"method":"rollback","expectedPlanVersion":2}),
    )
    .unwrap();
    let missing = send(
        &mut session,
        json!({"method":"run","expectedPlanVersion":3}),
    )
    .unwrap();
    assert_eq!(
        missing["nodes"]["input"]["diagnostic"]["error"]["code"],
        "runtime-input-missing"
    );
    assert_eq!(
        send(
            &mut session,
            json!({"method":"run","expectedPlanVersion":2,"input":null})
        )
        .unwrap_err()
        .code,
        "plan-conflict"
    );
    let mut empty = Session::new(registry());
    assert_eq!(
        send(
            &mut empty,
            json!({"method":"run","expectedPlanVersion":1,"input":null})
        )
        .unwrap_err()
        .code,
        "no-active-plan"
    );
}

struct ModeProbe;
impl Node for ModeProbe {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.mode", 1, vec![], vec![Port::packet("out")])
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        _: &Inputs,
        _: &Value,
    ) -> Result<Outputs, CoreError> {
        let input = context.input.unwrap();
        Ok([(
            "out".into(),
            OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: format!("{:?}:{}:{}", input.mode, input.revision, input.text),
                meta: None,
            }))),
        )]
        .into())
    }
}

#[test]
fn context_carries_mode_and_engines_do_not_share_mutable_input_state() {
    let mut registry = registry();
    registry.register(ModeProbe).unwrap();
    let mut graph = graph();
    let mut second = graph.nodes[0].clone();
    second.id = "other".into();
    graph.nodes.push(second);
    let mut probe = graph.nodes[0].clone();
    probe.id = "mode".into();
    probe.kind = "test.mode".into();
    graph.nodes.push(probe);
    let mut first = Engine::new(ExecutionPlan::compile(&graph, &registry).unwrap());
    let mut second = Engine::new(ExecutionPlan::compile(&graph, &registry).unwrap());
    let snapshot = input("k", 1);
    let copy = snapshot.clone();
    let a = first.run_with_input(&snapshot).unwrap();
    let mut snapshot2 = input("𠮷", 2);
    snapshot2.mode = InputMode::Finalize;
    let b = second.run_with_input(&snapshot2).unwrap();
    assert_eq!(snapshot, copy);
    assert_eq!(a.packet("input", "out"), a.packet("other", "out"));
    let Candidate::Text { text, .. } = &b.packet("mode", "out").unwrap().root else {
        panic!()
    };
    assert_eq!(text, "Finalize:2:𠮷");
    let Candidate::Text { text, .. } = &a.packet("mode", "out").unwrap().root else {
        panic!()
    };
    assert_eq!(text, "Compose:1:k");
}
