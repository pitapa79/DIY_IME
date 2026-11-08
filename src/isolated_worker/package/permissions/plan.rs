//! Immutable subjects minted from verified packages and a compiled graph.
use super::super::{VerifiedPackage, format::digest};
use crate::{
    execution_control::{CancellationToken, RunKey},
    graph::{Endpoint, ExecutionPlan, Graph},
    isolated_worker::adapter::{
        DictionaryCompanions, ExecutionGate, NetworkUseDescription, ResourceDescription, WasmNode,
    },
    node::{CoreError, Registry},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
fn err(message: &str) -> CoreError {
    CoreError::new("permission-plan", message)
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ApprovalSubject(pub(super) String);
impl ApprovalSubject {
    pub fn id(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalKind {
    Execute,
    InputFlow,
    ResourceRead,
    NetworkUse,
    SecretUse,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    #[serde(rename = "id")]
    pub(super) subject: ApprovalSubject,
    pub kind: ApprovalKind,
    pub package_id: String,
    pub nodes: Vec<String>,
    pub source: Option<Endpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceDescription>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkUseDescription>,
}
impl ApprovalRequest {
    pub fn subject(&self) -> &ApprovalSubject {
        &self.subject
    }
}
pub(super) struct NamedNode {
    pub id: String,
    pub node: WasmNode,
    pub config: Value,
}
/// No serde Deserialize or string-to-subject constructor. Only actual
/// verified packages plus a validated graph can create execution requests.
pub struct PermissionPlan {
    pub(super) requests: Vec<ApprovalRequest>,
    pub(super) nodes: Vec<NamedNode>,
    pub(super) gate: ExecutionGate,
    pub(super) plan: Option<ExecutionPlan>,
    snapshot_identity: std::sync::Arc<()>,
}
fn subject(value: Value) -> Result<ApprovalSubject, CoreError> {
    Ok(ApprovalSubject(digest(
        &serde_json::to_vec(&value).map_err(|_| err("subject encoding failed"))?,
    )?))
}
fn system(kind: &str) -> bool {
    matches!(
        kind,
        "input.text" | "demo.packet" | "e2e.input" | "e2e.output"
    )
}
type EdgeKey = (String, String, String, String);
fn reach(start: BTreeSet<String>, adjacency: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    let mut reached = start;
    let mut frontier: Vec<_> = reached.iter().cloned().collect();
    while let Some(current) = frontier.pop() {
        if let Some(neighbors) = adjacency.get(&current) {
            for next in neighbors {
                if reached.insert(next.clone()) {
                    frontier.push(next.clone());
                }
            }
        }
    }
    reached
}
fn not_cancelled(cancel: &CancellationToken) -> Result<(), CoreError> {
    if cancel.is_cancelled() {
        Err(err("permission preparation cancelled"))
    } else {
        Ok(())
    }
}
impl PermissionPlan {
    pub fn capture(
        graph: &Graph,
        external: &BTreeMap<String, VerifiedPackage>,
    ) -> Result<Self, CoreError> {
        Self::capture_with_cancel(graph, external, CancellationToken::default())
    }
    pub fn capture_with_cancel(
        graph: &Graph,
        external: &BTreeMap<String, VerifiedPackage>,
        cancel: CancellationToken,
    ) -> Result<Self, CoreError> {
        not_cancelled(&cancel)?;
        // Never accept a caller-composed Registry: a native/deferred Rust
        // implementation could advertise the same definition as a Wasm proof.
        let mut registry = Registry::new();
        crate::builtins::register_system_nodes(&mut registry)?;
        let mut implementations: BTreeMap<String, WasmNode> = BTreeMap::new();
        for package in external.values() {
            not_cancelled(&cancel)?;
            let kind = package.package().manifest().node.kind.clone();
            if let Some(node) = implementations.get_mut(&kind) {
                node.merge_validated_configs(&package.node())?;
            } else {
                implementations.insert(kind, package.node().fresh_preparation());
            }
        }
        for node in implementations.values() {
            registry.register_deferred(node.clone())?;
        }
        let definitions: BTreeMap<_, _> = registry
            .execution_definitions()
            .map(|d| (d.definition.kind.clone(), d.definition))
            .collect();
        let mut nodes = Vec::new();
        let mut requests: Vec<ApprovalRequest> = Vec::new();
        let mut evidence = BTreeMap::new();
        let mut authority: Option<ExecutionGate> = None;
        for instance in &graph.nodes {
            not_cancelled(&cancel)?;
            let definition = definitions
                .get(&instance.kind)
                .ok_or_else(|| err("unknown Node definition"))?;
            let implementation = if let Some(package) = external.get(&instance.id) {
                let manifest = package.package().manifest();
                if manifest.node.kind != instance.kind
                    || manifest.node.version != instance.version
                    || serde_json::to_value(&manifest.node).ok()
                        != serde_json::to_value(definition).ok()
                {
                    return Err(err("verified package differs from graph/registry"));
                }
                let node = implementations
                    .get(&manifest.node.kind)
                    .expect("verified implementation")
                    .clone();
                let gate = node.permission_gate();
                if authority
                    .as_ref()
                    .is_some_and(|known| !known.same_authority(&gate))
                {
                    return Err(err("Nodes must share the Host authority gate"));
                }
                authority = Some(gate);
                let identity = node.permission_identity()?;
                let id = subject(json!({"format":1,"scope":"execute","implementation":identity}))?;
                if let Some(existing) = requests.iter_mut().find(|request| request.subject == id) {
                    existing.nodes.push(instance.id.clone());
                    existing.nodes.sort();
                } else {
                    requests.push(ApprovalRequest {
                        subject: id.clone(),
                        kind: ApprovalKind::Execute,
                        package_id: manifest.package_id.clone(),
                        nodes: vec![instance.id.clone()],
                        source: None,
                        resource: None,
                        network: None,
                    });
                }
                let config = instance
                    .config
                    .as_ref()
                    .unwrap_or(&definition.default_config)
                    .clone();
                for resource in node.resource_descriptions(&config)? {
                    let subject = subject(
                        json!({"format":1,"scope":"resourceRead","implementation":identity,"config":config,"resource":resource}),
                    )?;
                    if let Some(existing) = requests.iter_mut().find(|r| r.subject == subject) {
                        existing.nodes.push(instance.id.clone());
                        existing.nodes.sort();
                    } else {
                        requests.push(ApprovalRequest {
                            subject,
                            kind: ApprovalKind::ResourceRead,
                            package_id: manifest.package_id.clone(),
                            nodes: vec![instance.id.clone()],
                            source: None,
                            resource: Some(resource),
                            network: None,
                        });
                    }
                }
                for network in node.network_descriptions() {
                    for scope in ["networkUse", "secretUse"] {
                        if scope == "secretUse" && network.endpoint.secret.is_none() {
                            continue;
                        }
                        let subject = subject(
                            json!({"format":1,"scope":scope,"implementation":identity,"config":config,"network":network}),
                        )?;
                        if let Some(existing) = requests.iter_mut().find(|r| r.subject == subject) {
                            existing.nodes.push(instance.id.clone());
                            existing.nodes.sort();
                        } else {
                            requests.push(ApprovalRequest {
                                subject,
                                kind: if scope == "networkUse" {
                                    ApprovalKind::NetworkUse
                                } else {
                                    ApprovalKind::SecretUse
                                },
                                package_id: manifest.package_id.clone(),
                                nodes: vec![instance.id.clone()],
                                source: None,
                                resource: None,
                                network: Some(network.clone()),
                            });
                        }
                    }
                }
                nodes.push(NamedNode {
                    id: instance.id.clone(),
                    node,
                    config,
                });
                id.0
            } else if system(&instance.kind) {
                format!("host-system:{}@{}", instance.kind, instance.version)
            } else {
                return Err(err(
                    "processing Nodes require an isolated verified implementation",
                ));
            };
            evidence.insert(instance.id.clone(),json!({"id":instance.id,"kind":instance.kind,"version":instance.version,"definition":definition,"config":instance.config.as_ref().unwrap_or(&definition.default_config),"implementation":implementation}));
        }
        if external
            .keys()
            .any(|id| !graph.nodes.iter().any(|n| &n.id == id))
        {
            return Err(err("unused external package proof"));
        }
        let gate = authority.unwrap_or_default();
        // Core independently validates exact configs, port types, missing inputs and cycles.
        let plan = ExecutionPlan::compile(graph, &registry)?;
        let mut edges: Vec<EdgeKey> = graph
            .edges
            .iter()
            .map(|e| {
                (
                    e.from.node.clone(),
                    e.from.port.clone(),
                    e.to.node.clone(),
                    e.to.port.clone(),
                )
            })
            .collect();
        edges.sort();
        let mut outgoing: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut incoming: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (from, _, to, _) in &edges {
            outgoing.entry(from.clone()).or_default().push(to.clone());
            incoming.entry(to.clone()).or_default().push(from.clone());
        }
        let mut upstream = BTreeMap::new();
        for node in &nodes {
            not_cancelled(&cancel)?;
            upstream.insert(node.id.clone(), reach([node.id.clone()].into(), &incoming));
        }
        for source in graph.nodes.iter().filter(|n| system(&n.kind)) {
            not_cancelled(&cancel)?;
            let definition = &definitions[&source.kind];
            for port in &definition.outputs {
                // A source port is distinct even if its sibling output reaches the same Node.
                let permitted: Vec<_> = edges
                    .iter()
                    .filter(|e| e.0 != source.id || e.1 == port.id)
                    .cloned()
                    .collect();
                let seeds = permitted
                    .iter()
                    .filter(|e| e.0 == source.id)
                    .map(|e| e.2.clone())
                    .collect();
                let mut forward = reach(seeds, &outgoing);
                forward.insert(source.id.clone());
                for destination in &nodes {
                    if !forward.contains(&destination.id) {
                        continue;
                    }
                    not_cancelled(&cancel)?;
                    let backward = &upstream[&destination.id];
                    let included: BTreeSet<_> = forward.intersection(backward).cloned().collect();
                    let path: Vec<_> = permitted
                        .iter()
                        .filter(|e| included.contains(&e.0) && included.contains(&e.2))
                        .collect();
                    let proof: Vec<_> = included.iter().map(|id| &evidence[id]).collect();
                    let endpoint = Endpoint::new(source.id.clone(), port.id.clone());
                    let id = subject(
                        json!({"format":1,"scope":"inputFlow","source":endpoint,"destination":destination.id,"nodes":proof,"edges":path}),
                    )?;
                    requests.push(ApprovalRequest {
                        subject: id,
                        kind: ApprovalKind::InputFlow,
                        package_id: external[&destination.id]
                            .package()
                            .manifest()
                            .package_id
                            .clone(),
                        nodes: vec![destination.id.clone()],
                        source: Some(endpoint),
                        resource: None,
                        network: None,
                    });
                }
            }
        }
        Ok(Self {
            requests,
            nodes,
            gate,
            plan: Some(plan),
            snapshot_identity: std::sync::Arc::new(()),
        })
    }
    pub(super) fn initialize_resources(
        &self,
        cancel: CancellationToken,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
        prepared: Option<(&str, &DictionaryCompanions, &RunKey)>,
    ) -> Result<(), CoreError> {
        let providers: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|n| n.node.dictionary_bindings())
            .collect();
        let Some(provider) = providers.first() else {
            return Ok(());
        };
        if providers
            .iter()
            .any(|p| !std::sync::Arc::ptr_eq(provider, p))
        {
            return Err(err("resource proofs use different Host binding providers"));
        }
        let mut batch = provider.batch();
        // Capture the whole IO batch before isolated validation, so its selected
        // cooperative read budget does not include Worker startup/JIT time.
        for node in &self.nodes {
            node.node
                .capture_resources(&node.config, &mut batch, &cancel, check)?;
        }
        for node in &self.nodes {
            node.node.initialize_resources(
                &node.config,
                &mut batch,
                cancel.clone(),
                check,
                prepared
                    .filter(|(id, _, _)| *id == node.id)
                    .map(|(id, modules, key)| (modules, key, id)),
            )?;
        }
        Ok(())
    }
    pub fn requests(&self) -> &[ApprovalRequest] {
        &self.requests
    }
    pub(in crate::isolated_worker::package) fn snapshot_identity(&self) -> std::sync::Arc<()> {
        self.snapshot_identity.clone()
    }
}
