//! Type/port preflight only, using the real Core compiler. Never executes guest
//! code or substitutes these definition-only nodes into a production Host.
use diy_ime2_core::{
    builtins::register_system_nodes,
    execution_control::{CancellationToken, CompletionPermit, DeferredNode, OwnedInvocation},
    graph::{ExecutionPlan, Graph},
    node::{CoreError, NodeDefinition, Registry},
};
use serde_json::Value;
use std::io::Read;
struct DefinitionOnly(NodeDefinition);
impl DeferredNode for DefinitionOnly {
    fn definition(&self) -> NodeDefinition {
        self.0.clone()
    }
    fn validate_config(&self, _: &Value) -> Result<(), CoreError> {
        // Only port/type compatibility is checked here. Actual config/ABI/code
        // validation still runs in the real isolated package/Host path.
        Ok(())
    }
    fn start(
        &self,
        _: OwnedInvocation,
        _: CompletionPermit,
        _: CancellationToken,
    ) -> Result<(), CoreError> {
        Err(CoreError::new(
            "preflight-execution-forbidden",
            "definition-only graph preflight",
        ))
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("explicit preflight JSON required")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("preflight file too large".into());
    }
    let input: Value = serde_json::from_slice(&bytes)?;
    let cases = input.as_array().ok_or("preflight array required")?;
    for case in cases {
        let mut registry = Registry::new();
        register_system_nodes(&mut registry)?;
        let definitions = case["definitions"]
            .as_array()
            .ok_or("definitions required")?;
        for value in definitions {
            registry.register_deferred(DefinitionOnly(serde_json::from_value(value.clone())?))?;
        }
        let graph = Graph::from_json(&case["graph"].to_string())?;
        ExecutionPlan::compile(&graph, &registry)
            .map_err(|error| format!("{}: {error}", case["name"]))?;
        for target in case["targets"].as_array().ok_or("targets required")? {
            let node = case["graph"]["nodes"]
                .as_array()
                .ok_or("nodes required")?
                .iter()
                .find(|node| node["id"] == target["node"])
                .ok_or("target node absent")?;
            let definition = definitions
                .iter()
                .find(|d| d["kind"] == node["kind"] && d["version"] == node["version"])
                .ok_or("target definition absent")?;
            if !definition["outputs"]
                .as_array()
                .ok_or("outputs required")?
                .iter()
                .any(|port| port["id"] == target["port"])
            {
                return Err("target port absent".into());
            }
        }
    }
    println!(
        "Core type/port preflight passed for {} graphs; no guest execution",
        cases.len()
    );
    Ok(())
}
