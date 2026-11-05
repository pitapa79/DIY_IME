//! Verify and print the captured migration inventory; processing definitions are now owned by Node repo.
use diy_ime2_core::{builtins::register_system_nodes, node::Registry};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let captured: serde_json::Value =
        serde_json::from_str(include_str!("../docs/node-migration-inventory.json"))?;
    let nodes = captured["nodes"]
        .as_array()
        .ok_or("captured nodes missing")?;
    if nodes.len() != 54 {
        return Err("captured inventory count changed".into());
    }
    let mut registry = Registry::new();
    register_system_nodes(&mut registry)?;
    let definitions: Vec<_> = registry.definitions().collect();
    if definitions.len() != 4 {
        return Err("Main must contain exactly four system adapters".into());
    }
    for definition in definitions {
        let entry = nodes
            .iter()
            .find(|n| n["definition"]["kind"] == definition.kind)
            .ok_or("system adapter absent from captured inventory")?;
        if entry["definition"] != serde_json::to_value(definition)? {
            return Err("system definition changed".into());
        }
    }
    // This is a historical snapshot, not regeneration of processing implementation metadata.
    println!("{}", serde_json::to_string_pretty(&captured)?);
    Ok(())
}
