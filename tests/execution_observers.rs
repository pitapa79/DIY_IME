#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue},
    execution_control::{
        ControlDecision, ControlFrame, ControlInput, ControlNode, ControlObservation, DecimalId,
        ExecutionHost, Milliseconds, ResourcePolicy, RunLedger, SessionEpoch, StartRequest,
        WakeReason, WorkerDispatcher,
    },
    graph::{ExecutionPlan, Graph},
    node::{CoreError, InputPort, NodeDefinition, Port, Registry, RunContext},
    runtime::NodeStatus,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
use test_nodes::register_demo_nodes;
type Seen = (bool, u64, WakeReason);
struct Waiter {
    seen: mpsc::Sender<Seen>,
    panic: bool,
}
impl ControlNode for Waiter {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.waiter",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn observe(
        &self,
        _: RunContext<'_>,
        frame: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        assert!(!self.panic, "observer test panic");
        let pending = matches!(frame.inputs["in"], ControlInput::Pending);
        self.seen
            .send((pending, frame.elapsed_ms, frame.reason))
            .unwrap();
        match &frame.inputs["in"] {
            ControlInput::Pending => Ok(ControlDecision::Wait {
                wake_after_ms: Some(Milliseconds::new(5).unwrap()),
            }),
            ControlInput::Resolved(value) => Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                [("out".into(), value.clone())].into(),
            ))),
        }
    }
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn host() -> ExecutionHost {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut profile =
        ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
    profile.max_observers = 1;
    ExecutionHost::new(
        format!("observer-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        profile,
    )
    .unwrap()
}
fn plan(registry: &Registry, kind: &str, inputs: &[&str]) -> ExecutionPlan {
    let failing = registry.definitions().any(|d| d.kind == "test.fail");
    let source = if failing {
        json!({"id":"source","kind":"test.fail","version":1})
    } else {
        json!({"id":"source","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"data"}}}})
    };
    let graph=Graph::from_json(&json!({"version":1,"nodes":[source,{"id":"control","kind":kind,"version":1}],"edges":inputs.iter().map(|port|json!({"from":{"node":"source","port":"out"},"to":{"node":"control","port":port}})).collect::<Vec<_>>()} ).to_string()).unwrap();
    ExecutionPlan::compile(&graph, registry).unwrap()
}
fn ledger(
    host: ExecutionHost,
    plan: ExecutionPlan,
) -> (RunLedger, diy_ime2_core::execution_control::RunKey) {
    let mut ledger = RunLedger::new(
        host,
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan,
        id(1),
    );
    let key=ledger.start_run(serde_json::from_value::<StartRequest>(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"control","port":"out"}]})).unwrap()).unwrap().key;
    (ledger, key)
}
fn frame(value: ControlInput) -> BTreeMap<String, ControlInput> {
    [("in".into(), value)].into()
}

#[test]
fn pending_is_observed_separately_and_wait_deadline_does_not_fabricate_data() {
    let host = host();
    let (tx, rx) = mpsc::channel();
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    registry
        .register_control(Waiter {
            seen: tx,
            panic: false,
        })
        .unwrap();
    let (ledger, key) = ledger(host.clone(), plan(&registry, "test.waiter", &["in"]));
    let mut observer = ledger.control_observer(&key, "control").unwrap();
    assert!(ledger.control_observer(&key, "control").is_err());
    assert_eq!(host.usage().observers, 1);
    let next = observer
        .observe(
            frame(ControlInput::Pending),
            WakeReason::Started,
            id(1),
            Duration::from_millis(10),
        )
        .unwrap();
    assert!(
        matches!(next,ControlObservation::Wait{wake_at:Some(at)} if at==Duration::from_millis(15))
    );
    assert_eq!(rx.recv().unwrap(), (true, 10, WakeReason::Started));
    assert_eq!(host.usage().observers, 1);
    // Reading a run does not wake policy or manufacture a pending Packet.
    ledger.get_run(&key).unwrap();
    assert!(rx.try_recv().is_err());
    assert!(
        observer
            .observe(
                frame(ControlInput::Pending),
                WakeReason::Timer,
                id(1),
                Duration::from_millis(15)
            )
            .is_err()
    );
    assert!(rx.try_recv().is_err());
    let packet = Arc::new(
        Packet::from_json(r#"{"version":1,"root":{"type":"text","text":"確定😀"}}"#).unwrap(),
    );
    let ControlObservation::Finished(execution) = observer
        .observe(
            frame(ControlInput::Resolved(ErrorAwareValue::Data(packet))),
            WakeReason::Input,
            id(2),
            Duration::from_millis(15),
        )
        .unwrap()
    else {
        panic!("finish expected")
    };
    assert_eq!(execution.status, NodeStatus::Executed);
    assert_eq!(rx.recv().unwrap(), (false, 15, WakeReason::Input));
    assert_eq!(host.usage().observers, 0);
    assert!(
        observer
            .observe(
                frame(ControlInput::Pending),
                WakeReason::Timer,
                id(3),
                Duration::from_millis(20)
            )
            .is_err()
    );
}

#[test]
fn observer_panic_is_a_node_error_and_releases_observer_reservation() {
    let host = host();
    let (tx, _) = mpsc::channel();
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    registry
        .register_control(Waiter {
            seen: tx,
            panic: true,
        })
        .unwrap();
    let (ledger, key) = ledger(host.clone(), plan(&registry, "test.waiter", &["in"]));
    let mut observer = ledger.control_observer(&key, "control").unwrap();
    let ControlObservation::Finished(execution) = observer
        .observe(
            frame(ControlInput::Pending),
            WakeReason::Started,
            id(1),
            Duration::ZERO,
        )
        .unwrap()
    else {
        panic!("node failure expected")
    };
    assert_eq!(execution.status, NodeStatus::Failed);
    assert_eq!(
        execution.diagnostic.as_ref().unwrap().error.code,
        "node-panic"
    );
    assert_eq!(host.usage().observers, 0);
}

#[test]
fn control_job_budgets_are_rejected_before_admission_even_when_null() {
    let host = host();
    let (tx, _) = mpsc::channel();
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    registry
        .register_control(Waiter {
            seen: tx,
            panic: false,
        })
        .unwrap();
    let (mut ledger, _) = ledger(host.clone(), plan(&registry, "test.waiter", &["in"]));
    let before = host.usage();
    let request=serde_json::from_value::<StartRequest>(json!({"startKey":"2","expectedPlanVersion":"1","targets":[{"node":"control","port":"out"}],"jobBudgets":[{"nodeId":"control","budgetMs":null}]})).unwrap();
    assert_eq!(
        ledger.start_run(request).err().unwrap().code,
        "invalid-deadline"
    );
    assert_eq!(host.usage(), before);
    assert_eq!(ledger.get_start(id(2)).unwrap_err().code, "start-not-seen");
}

#[test]
fn wait_deadline_overflow_becomes_node_error_without_wrapping_clock() {
    let host = host();
    let (tx, _rx) = mpsc::channel();
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    registry
        .register_control(Waiter {
            seen: tx,
            panic: false,
        })
        .unwrap();
    let mut ledger = RunLedger::new(
        host.clone(),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan(&registry, "test.waiter", &["in"]),
        id(1),
    );
    ledger.advance_clock(Duration::MAX).unwrap();
    let key=ledger.start_run(serde_json::from_value::<StartRequest>(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"control","port":"out"}]})).unwrap()).unwrap().key;
    let mut observer = ledger.control_observer(&key, "control").unwrap();
    let ControlObservation::Finished(execution) = observer
        .observe(
            frame(ControlInput::Pending),
            WakeReason::Started,
            id(1),
            Duration::MAX,
        )
        .unwrap()
    else {
        panic!("overflow must resolve to error")
    };
    assert_eq!(
        execution.diagnostic.as_ref().unwrap().error.code,
        "invalid-deadline"
    );
    assert_eq!(host.usage().observers, 0);
}

struct Gate {
    calls: Arc<AtomicU64>,
}
struct Fail;
impl diy_ime2_core::node::Node for Fail {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.fail", 1, vec![], vec![Port::packet("out")])
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        _: &diy_ime2_core::node::Inputs,
        _: &Value,
    ) -> Result<diy_ime2_core::node::Outputs, CoreError> {
        Err(CoreError::new("test-failure", "source failure"))
    }
}
impl ControlNode for Gate {
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
    fn observe(
        &self,
        _: RunContext<'_>,
        _: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        unreachable!("required NoOutput/blocked error must gate policy")
    }
}
#[test]
fn required_no_output_waits_for_other_inputs_and_preserves_blocked_error_priority() {
    for blocked in [false, true] {
        let host = host();
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        if blocked {
            registry.register(Fail).unwrap();
        }
        let calls = Arc::new(AtomicU64::new(0));
        registry
            .register_control(Gate {
                calls: Arc::clone(&calls),
            })
            .unwrap();
        let (mut ledger, key) = ledger(host.clone(), plan(&registry, "test.gate", &["a", "b"]));
        let mut observer = ledger.control_observer(&key, "control").unwrap();
        let first = [
            (
                "a".into(),
                ControlInput::Resolved(ErrorAwareValue::NoOutput),
            ),
            ("b".into(), ControlInput::Pending),
        ]
        .into();
        assert!(matches!(
            observer
                .observe(first, WakeReason::Started, id(1), Duration::ZERO)
                .unwrap(),
            ControlObservation::Wait { wake_at: None }
        ));
        let other = if blocked {
            // Generate a genuine Core error in this run, rather than constructing ErrorSet.
            let inv = ledger
                .owned_invocation(&key, "source", BTreeMap::new())
                .unwrap();
            let mut d = WorkerDispatcher::new(host.clone());
            d.submit(inv).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let execution = loop {
                if let Some(execution) = d
                    .poll()
                    .into_iter()
                    .find_map(|event| event.into_execution())
                {
                    break execution;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            };
            ErrorAwareValue::Error(execution.error_outputs["out"].clone())
        } else {
            ErrorAwareValue::NoOutput
        };
        let second = [
            (
                "a".into(),
                ControlInput::Resolved(ErrorAwareValue::NoOutput),
            ),
            ("b".into(), ControlInput::Resolved(other)),
        ]
        .into();
        let ControlObservation::Finished(execution) = observer
            .observe(second, WakeReason::Input, id(2), Duration::from_millis(1))
            .unwrap()
        else {
            panic!("terminal gate expected")
        };
        assert_eq!(
            execution.status,
            if blocked {
                NodeStatus::Blocked
            } else {
                NodeStatus::Skipped
            }
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(host.usage().observers, 0);
        ledger.cancel_run(&key).unwrap();
    }
}
