//! The compatibility facade must re-export the same Rust types, not duplicate them.
use diy_ime2_core::{Candidate, Packet, graph::Graph, node::Registry};
#[test]
fn facade_and_node_independent_core_have_identical_types() {
    let packet = Packet::new(Candidate::Text {
        text: "core-boundary".into(),
        meta: None,
    });
    let packet: diy_ime2_runtime::Packet = packet;
    packet.validate().unwrap();
    let registry = Registry::new();
    let graph = Graph::from_json(r#"{"version":1,"nodes":[],"edges":[]}"#).unwrap();
    let plan = diy_ime2_runtime::graph::ExecutionPlan::compile(&graph, &registry).unwrap();
    let _engine: diy_ime2_core::runtime::Engine = diy_ime2_runtime::runtime::Engine::new(plan);
}
