#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, PrepareResources,
        PreparedNode, Registry, RootConstraint, RunContext,
    },
    runtime::{Engine, NodeStatus},
    session::{Request, Session},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_nodes::register_demo_nodes;

#[derive(Default)]
struct Counts {
    prepares: AtomicUsize,
    executes: AtomicUsize,
    drops: AtomicUsize,
}
struct Source(Arc<Counts>);
struct Snapshot {
    value: String,
    behavior: String,
    counts: Arc<Counts>,
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        self.counts.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl PreparedNode for Snapshot {
    fn execute(&self, _: RunContext<'_>, _: &Inputs) -> Result<Outputs, CoreError> {
        self.counts.executes.fetch_add(1, Ordering::SeqCst);
        let value = match self.behavior.as_str() {
            "panic-run" => panic!("prepared panic"),
            "error-run" => return Err(CoreError::new("test-run", "prepared error")),
            "missing" => return Ok(Outputs::new()),
            "unknown" => return Ok([("other".into(), OutputValue::NoOutput)].into()),
            "none" => OutputValue::NoOutput,
            "bad-packet" => OutputValue::Data(Arc::new(Packet {
                version: 2,
                root: Candidate::Text {
                    text: self.value.clone(),
                    meta: None,
                },
            })),
            "bad-root" => {
                OutputValue::Data(Arc::new(Packet::new(Candidate::Sequence { items: vec![] })))
            }
            _ => OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: self.value.clone(),
                meta: None,
            }))),
        };
        Ok([("out".into(), value)].into())
    }
}
impl Node for Source {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.prepared",
            1,
            vec![],
            vec![Port::packet("out").with_root(RootConstraint::Text)],
        )
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if config.is_object() {
            Ok(())
        } else {
            Err(CoreError::new("test-config", "expected object"))
        }
    }
    fn prepare(
        &self,
        config: &Value,
        resources: &mut dyn PrepareResources,
    ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        self.0.prepares.fetch_add(1, Ordering::SeqCst);
        let behavior = config["behavior"].as_str().unwrap_or("");
        match behavior {
            "panic-prepare" => panic!("prepare panic"),
            "fail-prepare" => return Err(CoreError::new("test-prepare", "reject")),
            _ => {}
        }
        let value = if let Some(reference) = config["resource"].as_str() {
            String::from_utf8(resources.read(reference, 32)?.to_vec()).unwrap()
        } else {
            config["value"].as_str().unwrap_or("default").into()
        };
        Ok(Some(Arc::new(Snapshot {
            value,
            behavior: behavior.into(),
            counts: Arc::clone(&self.0),
        })))
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        Err(CoreError::new("unprepared", "legacy execute must not run"))
    }
}
fn registry(counts: &Arc<Counts>) -> Registry {
    let mut r = Registry::new();
    r.register(Source(Arc::clone(counts))).unwrap();
    register_demo_nodes(&mut r).unwrap();
    r
}
fn graph(config: Value) -> Graph {
    Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"test.prepared","version":1,"config":config}],"edges":[]}).to_string()).unwrap()
}
fn compile(config: Value, r: &Registry) -> ExecutionPlan {
    ExecutionPlan::compile(&graph(config), r).unwrap()
}
fn value(e: &mut Engine) -> String {
    let report = e.run().unwrap();
    assert!(report.succeeded());
    let Candidate::Text { text, .. } = &report.packet("source", "out").unwrap().root else {
        panic!("not Text")
    };
    text.clone()
}

#[test]
fn immutable_snapshots_survive_switch_and_rollback_without_preparing_again() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let mut e = Engine::new(compile(json!({"value":"old"}), &r));
    assert_eq!(value(&mut e), "old");
    assert_eq!(value(&mut e), "old");
    e.prepare(&graph(json!({"value":"new"})), &r).unwrap();
    assert_eq!(value(&mut e), "old");
    e.activate_pending().unwrap();
    assert_eq!(value(&mut e), "new");
    e.rollback().unwrap();
    assert_eq!(value(&mut e), "old");
    assert_eq!(e.plan_version(), 3);
    assert_eq!(c.prepares.load(Ordering::SeqCst), 2);
    assert_eq!(c.drops.load(Ordering::SeqCst), 1);
    drop(e);
    assert_eq!(c.drops.load(Ordering::SeqCst), 2);
}
#[test]
fn failed_and_panicking_preparation_preserve_active_pending_and_previous() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let mut e = Engine::new(compile(json!({"value":"A"}), &r));
    e.prepare(&graph(json!({"value":"B"})), &r).unwrap();
    e.activate_pending().unwrap();
    e.prepare(&graph(json!({"value":"C"})), &r).unwrap();
    for (behavior, code) in [
        ("fail-prepare", "test-prepare"),
        ("panic-prepare", "node-prepare-panic"),
    ] {
        let err = e
            .prepare(&graph(json!({"behavior":behavior})), &r)
            .unwrap_err();
        assert_eq!(err.code, code);
        assert!(err.message.contains("source"));
        assert_eq!(value(&mut e), "B");
        assert_eq!(e.plan_version(), 2);
    }
    e.activate_pending().unwrap();
    assert_eq!(value(&mut e), "C");
    e.rollback().unwrap();
    assert_eq!(value(&mut e), "B");
}
#[test]
fn all_structure_and_config_checks_precede_any_prepare() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let mut g = graph(json!({}));
    g.nodes.push(g.nodes[0].clone());
    assert!(ExecutionPlan::compile(&g, &r).is_err());
    let mut g = graph(json!({}));
    g.nodes.push(
        serde_json::from_value(
            json!({"id":"bad","kind":"test.prepared","version":1,"config":false}),
        )
        .unwrap(),
    );
    assert!(ExecutionPlan::compile(&g, &r).is_err());
    let mut g = graph(json!({}));
    g.edges.push(
        serde_json::from_value(
            json!({"from":{"node":"source","port":"out"},"to":{"node":"absent","port":"in"}}),
        )
        .unwrap(),
    );
    assert!(ExecutionPlan::compile(&g, &r).is_err());
    let cyclic=Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"test.prepared","version":1},{"id":"cycle","kind":"demo.identity","version":1}],"edges":[{"from":{"node":"cycle","port":"out"},"to":{"node":"cycle","port":"in"}}]}).to_string()).unwrap();
    assert_eq!(
        ExecutionPlan::compile(&cyclic, &r).err().unwrap().code,
        "cycle"
    );
    assert_eq!(c.prepares.load(Ordering::SeqCst), 0);
}
#[test]
fn partial_prepare_and_pending_replacement_release_owned_resources() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let mut g = graph(json!({"value":"first"}));
    g.nodes.push(serde_json::from_value(json!({"id":"second","kind":"test.prepared","version":1,"config":{"behavior":"fail-prepare"}})).unwrap());
    assert!(ExecutionPlan::compile(&g, &r).is_err());
    assert_eq!(c.drops.load(Ordering::SeqCst), 1);
    let mut e = Engine::new(compile(json!({}), &r));
    e.prepare(&graph(json!({})), &r).unwrap();
    e.prepare(&graph(json!({})), &r).unwrap();
    assert_eq!(c.drops.load(Ordering::SeqCst), 2);
    drop(e);
    assert_eq!(c.drops.load(Ordering::SeqCst), 4);
}
#[test]
fn prepared_outputs_use_existing_panic_error_packet_root_and_port_checks() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    for (behavior, code) in [
        ("panic-run", "node-panic"),
        ("error-run", "test-run"),
        ("missing", "missing-output"),
        ("unknown", "unknown-output"),
        ("bad-packet", "invalid-packet"),
        ("bad-root", "invalid-root"),
    ] {
        let mut e = Engine::new(compile(json!({"behavior":behavior}), &r));
        let report = e.run().unwrap();
        assert_eq!(
            report.nodes["source"].status,
            NodeStatus::Failed,
            "{behavior}"
        );
        assert_eq!(
            report.nodes["source"]
                .diagnostic
                .as_ref()
                .unwrap()
                .error
                .code,
            code,
            "{behavior}"
        );
    }
    let mut e = Engine::new(compile(json!({"behavior":"none"}), &r));
    let report = e.run().unwrap();
    assert!(report.succeeded());
    assert!(report.packet("source", "out").is_none());
}
struct Bytes {
    value: Vec<u8>,
    reads: usize,
}
impl PrepareResources for Bytes {
    fn read(&mut self, reference: &str, max: usize) -> Result<Arc<[u8]>, CoreError> {
        assert_eq!(reference, "sample");
        assert_eq!(max, 32);
        self.reads += 1;
        Ok(Arc::from(self.value.clone()))
    }
}
#[test]
fn explicit_memory_resources_are_captured_once_and_default_source_is_denied() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let g = graph(json!({"resource":"sample"}));
    assert_eq!(
        ExecutionPlan::compile(&g, &r).err().unwrap().code,
        "resource-unavailable"
    );
    let mut bytes = Bytes {
        value: b"old".to_vec(),
        reads: 0,
    };
    let mut e = Engine::new(ExecutionPlan::compile_with_resources(&g, &r, &mut bytes).unwrap());
    bytes.value = b"new".to_vec();
    assert_eq!(value(&mut e), "old");
    assert_eq!(bytes.reads, 1);
    e.prepare_plan(ExecutionPlan::compile_with_resources(&g, &r, &mut bytes).unwrap());
    e.activate_pending().unwrap();
    assert_eq!(value(&mut e), "new");
    e.rollback().unwrap();
    assert_eq!(value(&mut e), "old");
    assert_eq!(bytes.reads, 2);
}
#[test]
fn same_registry_separates_instances_plans_and_sessions() {
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let mut g = graph(json!({"value":"A"}));
    g.nodes.push(
        serde_json::from_value(
            json!({"id":"other","kind":"test.prepared","version":1,"config":{"value":"B"}}),
        )
        .unwrap(),
    );
    let mut e = Engine::new(ExecutionPlan::compile(&g, &r).unwrap());
    let report = e.run().unwrap();
    assert_ne!(
        report.packet("source", "out"),
        report.packet("other", "out")
    );
    let mut other = Engine::new(compile(json!({"value":"C"}), &r));
    assert_eq!(value(&mut other), "C");
    assert_eq!(value(&mut e), "A");
    let mut a = Session::new(registry(&c));
    let mut b = Session::new(registry(&c));
    a.handle(Request::Prepare {
        graph: graph(json!({"value":"A"})),
    })
    .unwrap();
    b.handle(Request::Prepare {
        graph: graph(json!({"value":"B"})),
    })
    .unwrap();
    assert_eq!(
        a.state().prepared.as_ref().unwrap().token,
        b.state().prepared.as_ref().unwrap().token
    );
    a.handle(Request::Activate { token: "1".into() }).unwrap();
    b.handle(Request::Activate { token: "1".into() }).unwrap();
    let a = a
        .handle(Request::Run {
            expected_plan_version: 1,
            input: None,
        })
        .unwrap();
    let b = b
        .handle(Request::Run {
            expected_plan_version: 1,
            input: None,
        })
        .unwrap();
    assert_ne!(a, b);
}
#[test]
fn session_validate_discards_temporary_resources_and_failed_prepare_keeps_token() {
    let c = Arc::new(Counts::default());
    let mut s = Session::new(registry(&c));
    s.handle(Request::Prepare {
        graph: graph(json!({"value":"A"})),
    })
    .unwrap();
    let before = serde_json::to_value(s.state()).unwrap();
    s.handle(Request::Validate {
        graph: graph(json!({"value":"validation"})),
    })
    .unwrap();
    assert_eq!(serde_json::to_value(s.state()).unwrap(), before);
    assert_eq!(c.drops.load(Ordering::SeqCst), 1);
    assert!(
        s.handle(Request::Prepare {
            graph: graph(json!({"behavior":"fail-prepare"}))
        })
        .is_err()
    );
    assert_eq!(serde_json::to_value(s.state()).unwrap(), before);
    s.handle(Request::Activate { token: "1".into() }).unwrap();
    assert_eq!(c.prepares.load(Ordering::SeqCst), 3);
}

struct Sink(Arc<Counts>);
impl Node for Sink {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.prepared-sink",
            1,
            vec![diy_ime2_core::node::InputPort::required(
                Port::packet("in").with_root(RootConstraint::Text),
            )],
            vec![Port::packet("out").with_root(RootConstraint::Text)],
        )
    }
    fn prepare(
        &self,
        config: &Value,
        resources: &mut dyn PrepareResources,
    ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        Source(Arc::clone(&self.0)).prepare(config, resources)
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        panic!("unprepared sink")
    }
}
#[test]
fn missing_or_failed_required_inputs_do_not_execute_prepared_sink() {
    for (behavior, status) in [
        ("none", NodeStatus::Skipped),
        ("error-run", NodeStatus::Blocked),
    ] {
        let c = Arc::new(Counts::default());
        let mut r = registry(&c);
        r.register(Sink(Arc::clone(&c))).unwrap();
        let mut g = graph(json!({"behavior":behavior}));
        g.nodes.push(
            serde_json::from_value(json!({"id":"sink","kind":"test.prepared-sink","version":1}))
                .unwrap(),
        );
        g.edges.push(
            serde_json::from_value(
                json!({"from":{"node":"source","port":"out"},"to":{"node":"sink","port":"in"}}),
            )
            .unwrap(),
        );
        let mut e = Engine::new(ExecutionPlan::compile(&g, &r).unwrap());
        let report = e.run().unwrap();
        assert_eq!(report.nodes["sink"].status, status);
        assert_eq!(c.prepares.load(Ordering::SeqCst), 2);
        assert_eq!(c.executes.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn asynchronous_worker_executes_the_owned_snapshot_and_releases_physical_job() {
    use diy_ime2_core::execution_control::{
        DecimalId, ExecutionHost, ResourcePolicy, RunLedger, SessionEpoch, TerminalReason,
        WorkerDispatcher,
    };
    use diy_ime2_core::runtime::ResolvedValue;
    use std::time::{Duration, Instant};
    let c = Arc::new(Counts::default());
    let r = registry(&c);
    let profile = serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
        .unwrap()["resourceProfiles"]["bounded"]
        .clone();
    let host = ExecutionHost::new(
        "prepared-worker-test",
        ResourcePolicy::from_value(profile).unwrap(),
    )
    .unwrap();
    let mut ledger = RunLedger::new(
        host.clone(),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        compile(json!({"value":"snapshot"}), &r),
        DecimalId::new(1).unwrap(),
    );
    let key=ledger.start_run(serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}]})).unwrap()).unwrap().key;
    ledger.mark_running(&key).unwrap();
    let mut graph = ledger.graph_execution(&key).unwrap();
    let mut workers = WorkerDispatcher::new(host.clone());
    let job = workers.submit(graph.start_work("source").unwrap()).unwrap();
    graph.bind_job("source", job).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    let mut saw_data = false;
    while !graph.is_complete() || workers.states().next().is_some() {
        for completion in workers.poll() {
            if let Some(execution) = completion.execution() {
                assert_eq!(execution.status, NodeStatus::Executed);
                let ResolvedValue::Data(packet) = &execution.outputs["out"] else {
                    panic!("missing data")
                };
                assert_eq!(
                    packet.root,
                    Candidate::Text {
                        text: "snapshot".into(),
                        meta: None
                    }
                );
                saw_data = true;
            }
            graph.complete_work(completion).unwrap();
        }
        assert!(Instant::now() < end, "prepared worker completion timed out");
        std::thread::yield_now();
    }
    assert!(saw_data);
    graph
        .finish(&mut ledger, TerminalReason::Completed)
        .unwrap();
    assert_eq!(host.usage().physical_jobs, 0);
    assert_eq!(c.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(c.executes.load(Ordering::SeqCst), 1);
    drop(workers);
    drop(graph);
    drop(ledger);
    assert_eq!(c.drops.load(Ordering::SeqCst), 1);
}
