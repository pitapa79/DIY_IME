#[allow(dead_code)]
#[path = "../tests/support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    graph::{ExecutionPlan, Graph},
    node::Registry,
    runtime::Engine,
};
use test_nodes::register_demo_nodes;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry)?;
    let graph = Graph::from_json(include_str!("graphs/hello.json"))?;
    let mut engine = Engine::new(ExecutionPlan::compile(&graph, &registry)?);
    let old = engine.run()?;
    println!("initial: plan={} run={}", old.plan_version, old.run_id);

    let mut edited = graph.clone();
    edited.nodes[0].config =
        Some(serde_json::json!({"packet":{"version":1,"root":{"type":"text","text":"updated"}}}));
    engine.prepare(&edited, &registry)?;
    println!("prepared, active plan={}", engine.plan_version());

    // 非 IME のこの例では、run の間を安全な切替境界とする。
    engine.activate_pending()?;
    let updated = engine.run()?;
    println!(
        "updated: {}",
        updated.packet("result", "out").unwrap().to_json()?
    );
    assert!(!engine.is_current(&old));

    let mut invalid = edited.clone();
    invalid.nodes[0].kind = "missing-node-kind".into();
    assert!(engine.prepare(&invalid, &registry).is_err());
    println!(
        "invalid edit rejected, active plan={}",
        engine.plan_version()
    );

    engine.rollback()?;
    let restored = engine.run()?;
    println!(
        "restored: {}",
        restored.packet("result", "out").unwrap().to_json()?
    );
    Ok(())
}
