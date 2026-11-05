//! Private host-side prototype, not a Node ABI or an OS sandbox.
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
/// Populated by the trusted host, never accepted from a Node request as proof.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub principal: String,
    pub node: String,
    pub artifact: String,
    pub graph: String,
    pub session: String,
    pub run: String,
}
struct Policy {
    epoch: Arc<()>,
    operations: BTreeSet<String>,
}
/// Not serializable. Every use rechecks current policy and host invocation binding.
#[derive(Clone)]
pub struct Permit {
    owner: Arc<()>,
    epoch: Arc<()>,
    binding: Binding,
}
#[derive(Default)]
pub struct Authority {
    owner: Arc<()>,
    policies: HashMap<Binding, Policy>,
}
impl Authority {
    /// Every replacement rotates identity, even on rollback to old values.
    /// Operation strings are test-host vocabulary, not an adopted permission list.
    pub fn replace(&mut self, binding: Binding, requested: &[&str], approved: &[&str]) {
        let operations = requested
            .iter()
            .filter(|name| approved.contains(name))
            .map(|name| (*name).to_owned())
            .collect();
        self.policies.insert(
            binding,
            Policy {
                epoch: Arc::new(()),
                operations,
            },
        );
    }
    pub fn issue(&self, binding: &Binding) -> Option<Permit> {
        let policy = self.policies.get(binding)?;
        Some(Permit {
            owner: self.owner.clone(),
            epoch: policy.epoch.clone(),
            binding: binding.clone(),
        })
    }
    pub fn revoke(&mut self, binding: &Binding) {
        self.policies.remove(binding);
    }
    pub fn allows(&self, binding: &Binding, permit: Option<&Permit>, operation: &str) -> bool {
        let Some(permit) = permit else {
            return false;
        };
        let Some(policy) = self.policies.get(binding) else {
            return false;
        };
        Arc::ptr_eq(&self.owner, &permit.owner)
            && Arc::ptr_eq(&policy.epoch, &permit.epoch)
            && permit.binding == *binding
            && policy.operations.contains(operation)
    }
}
