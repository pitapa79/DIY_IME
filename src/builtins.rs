//! Host composition of system, practical and compatibility Nodes.
use crate::node::{CoreError, Registry};
mod demo;
pub use demo::*;
use demo::{config_keys, output};
pub mod initial_e2e;

/// Canonical Host system adapters only. External execution never falls back to
/// the in-main processing implementations retained by the legacy Editor.
pub fn register_system_nodes(registry: &mut Registry) -> Result<(), CoreError> {
    registry.register(PacketSource)?;
    registry.register(initial_e2e::FixedInput)?;
    registry.register(initial_e2e::DebugOutput)?;
    registry.register(crate::nodes::runtime_input::RuntimeTextInput)?;
    Ok(())
}

/// Standard Main composition contains the four canonical Host system adapters.
/// Processing kinds are loaded as approved external Wasm packages.
pub fn register_builtin_nodes(registry: &mut Registry) -> Result<(), CoreError> {
    register_system_nodes(registry)
}
#[cfg(test)]
mod system_tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn external_registry_starts_with_exactly_the_four_canonical_host_system_adapters() {
        let mut registry = Registry::new();
        register_system_nodes(&mut registry).unwrap();
        let definitions: Vec<_> = registry.execution_definitions().collect();
        assert_eq!(definitions.len(), 4);
        assert_eq!(
            definitions
                .iter()
                .map(|d| d.definition.kind.as_str())
                .collect::<BTreeSet<_>>(),
            ["input.text", "demo.packet", "e2e.input", "e2e.output"].into()
        );
        assert!(definitions.iter().all(|d| d.definition.version == 1));
    }
}
