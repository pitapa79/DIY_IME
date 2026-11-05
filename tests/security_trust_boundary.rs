//! Executable contracts for today's trusted, in-process Rust boundary.
//! These tests do not certify arbitrary Rust code or OS isolation.
#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, PrepareResources,
        PreparedNode, Registry, RunContext,
    },
    runtime::Engine,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_nodes::register_demo_nodes;

struct ResourceProbe(Arc<AtomicUsize>);
impl Node for ResourceProbe {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.resource-probe", 1, vec![], vec![Port::packet("out")])
    }
    fn validate_config(&self, value: &Value) -> Result<(), CoreError> {
        if value.is_object() {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "object required"))
        }
    }
    fn prepare(
        &self,
        _: &Value,
        resources: &mut dyn PrepareResources,
    ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        resources.read("canary.txt", 32)?;
        Ok(None)
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        Ok([("out".into(), OutputValue::NoOutput)].into())
    }
}
fn graph(node: Value) -> Graph {
    Graph::from_json(&json!({"version":1,"nodes":[node],"edges":[]}).to_string()).unwrap()
}
fn instance(kind: &str, config: Value) -> Value {
    json!({"id":"probe","kind":kind,"version":1,"config":config})
}
#[test]
fn graph_names_and_self_declared_permissions_do_not_load_or_authorize_code() {
    let registry = Registry::new();
    for kind in [
        "third-party.node",
        "C:\\canary.dll",
        "https://invalid.example/node.wasm",
    ] {
        let g = graph(instance(
            kind,
            json!({"trusted":true,"permissions":["*"],"runtime":"native"}),
        ));
        assert_eq!(
            ExecutionPlan::compile(&g, &registry).err().unwrap().code,
            "unknown-kind"
        );
    }
}
#[test]
fn unknown_security_claims_in_graph_or_definition_are_rejected_as_data() {
    let mut node = instance("demo.packet", json!({}));
    node["permissions"] = json!(["*"]);
    assert!(Graph::from_json(&json!({"version":1,"nodes":[node],"edges":[]}).to_string()).is_err());
    assert!(
        Graph::from_json(&json!({"version":1,"nodes":[],"edges":[],"trusted":true}).to_string())
            .is_err()
    );
    let mut definition =
        serde_json::to_value(NodeDefinition::new("test.fake", 1, vec![], vec![])).unwrap();
    definition["trusted"] = json!(true);
    assert!(serde_json::from_value::<NodeDefinition>(definition).is_err());
}
#[test]
fn config_claims_cannot_enable_prepare_resources_without_an_explicit_host() {
    let prepares = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new();
    registry.register(ResourceProbe(prepares.clone())).unwrap();
    for config in [
        json!({}),
        json!({"permissions":["file.read"],"resourceRoot":"C:\\","trusted":true}),
    ] {
        let g = graph(instance("test.resource-probe", config));
        assert_eq!(
            ExecutionPlan::compile(&g, &registry).err().unwrap().code,
            "resource-unavailable"
        );
    }
    assert_eq!(prepares.load(Ordering::SeqCst), 2);
}
#[test]
fn malformed_graph_is_rejected_before_any_resource_access() {
    let prepares = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new();
    registry.register(ResourceProbe(prepares.clone())).unwrap();
    let mut g = graph(instance("test.resource-probe", json!({})));
    g.nodes.push(g.nodes[0].clone());
    assert_eq!(
        ExecutionPlan::compile(&g, &registry).err().unwrap().code,
        "duplicate-node"
    );
    let mut g = graph(instance("test.resource-probe", json!({})));
    g.nodes
        .push(serde_json::from_value(instance("unregistered.node", json!({}))).unwrap());
    g.nodes[1].id = "unknown".into();
    assert_eq!(
        ExecutionPlan::compile(&g, &registry).err().unwrap().code,
        "unknown-kind"
    );
    assert_eq!(prepares.load(Ordering::SeqCst), 0);
}
#[test]
fn registry_rejects_replacement_and_graph_versions_without_breaking_legacy_execution() {
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    assert_eq!(
        registry
            .register(diy_ime2_core::builtins::PacketSource)
            .unwrap_err()
            .code,
        "duplicate-kind"
    );
    let packet = Packet::new(Candidate::Text {
        text: "canary日本語".into(),
        meta: None,
    });
    let g = graph(instance("demo.packet", json!({"packet":packet})));
    let report = Engine::new(ExecutionPlan::compile(&g, &registry).unwrap())
        .run()
        .unwrap();
    assert_eq!(report.packet("probe", "out").unwrap().as_ref(), &packet);
    let mut unknown_version = g.clone();
    unknown_version.nodes[0].version = 999;
    assert_eq!(
        ExecutionPlan::compile(&unknown_version, &registry)
            .err()
            .unwrap()
            .code,
        "unknown-kind"
    );
    let mut old_graph = g;
    old_graph.version = 0;
    assert_eq!(
        ExecutionPlan::compile(&old_graph, &registry)
            .err()
            .unwrap()
            .code,
        "graph-version"
    );
}
