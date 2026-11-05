//! 編集用グラフの検証と実行計画の生成。
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::node::{CoreError, RegisteredNode, Registry, RootConstraint};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub version: u32,
    pub nodes: Vec<NodeInstance>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeInstance {
    pub id: String,
    pub kind: String,
    pub version: u32,
    /// 省略時はノード宣言の default_config を利用します。暗黙の部分マージはしません。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::json::optional_non_null"
    )]
    pub config: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub node: String,
    pub port: String,
}

impl Endpoint {
    pub fn new(node: impl Into<String>, port: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            port: port.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: Endpoint,
    pub to: Endpoint,
}

impl Graph {
    /// JSON 構文の読み込み。型・接続・設定の最終検査は ExecutionPlan::compile で行います。
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        serde_json::from_str(json).map_err(|error| CoreError::new("graph-json", error.to_string()))
    }
    pub fn to_json(&self) -> Result<String, CoreError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| CoreError::new("graph-json", error.to_string()))
    }
}

pub(crate) struct PlannedNode {
    pub id: String,
    pub node: RegisteredNode,
    pub config: Value,
    pub sources: BTreeMap<String, Endpoint>,
    pub prepared: Option<std::sync::Arc<dyn crate::node::PreparedNode>>,
}

/// 作成後は編集できないスナップショット。登録情報・設定・接続をすべて保持します。
pub struct ExecutionPlan {
    pub(crate) nodes: Vec<PlannedNode>,
}

impl ExecutionPlan {
    pub(crate) fn require_protocol_v2(&self) -> Result<(), CoreError> {
        if self.nodes.iter().any(|node| {
            !matches!(
                node.node.implementation,
                crate::node::RegisteredImplementation::Legacy(_)
            )
        }) {
            return Err(CoreError::new(
                "unsupported-capability",
                "error-aware/deferred/control nodes require the opt-in Rust boundary; session v2 cannot advertise or run them",
            ));
        }
        Ok(())
    }
    pub(crate) fn require_synchronous(&self) -> Result<(), CoreError> {
        if self.nodes.iter().any(|node| {
            matches!(
                node.node.implementation,
                crate::node::RegisteredImplementation::Deferred { .. }
                    | crate::node::RegisteredImplementation::Control { .. }
            )
        }) {
            return Err(CoreError::new(
                "unsupported-capability",
                "deferred/control nodes require the opt-in execution controller",
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_targets(&self, targets: &[Endpoint]) -> Result<(), CoreError> {
        let mut seen = BTreeSet::new();
        if targets.is_empty()
            || targets.iter().any(|target| {
                !seen.insert((target.node.clone(), target.port.clone()))
                    || !self.nodes.iter().any(|node| {
                        node.id == target.node
                            && node
                                .node
                                .definition
                                .outputs
                                .iter()
                                .any(|port| port.id == target.port)
                    })
            })
        {
            return Err(CoreError::new(
                "invalid-targets",
                "targets must be nonempty, unique, existing output endpoints",
            ));
        }
        Ok(())
    }

    pub fn node_order(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().map(|node| node.id.as_str())
    }

    pub fn compile(graph: &Graph, registry: &Registry) -> Result<Self, CoreError> {
        Self::compile_with_resources(graph, registry, &mut crate::node::NoPrepareResources)
    }

    /// Hostが明示したprepareバッチからだけ資源を取得する。
    /// 全構造/設定検査を終えてから準備し、失敗時には未完成計画を公開しない。
    pub fn compile_with_resources(
        graph: &Graph,
        registry: &Registry,
        resources: &mut dyn crate::node::PrepareResources,
    ) -> Result<Self, CoreError> {
        if graph.version != 1 {
            return Err(CoreError::new("graph-version", "expected graph version 1"));
        }
        let mut planned = Vec::with_capacity(graph.nodes.len());
        let mut indices = BTreeMap::new();
        for instance in &graph.nodes {
            if instance.id.trim().is_empty()
                || indices.insert(instance.id.clone(), planned.len()).is_some()
            {
                return Err(CoreError::new(
                    "duplicate-node",
                    format!("empty or duplicate node ID: {}", instance.id),
                ));
            }
            let node = registry
                .nodes
                .get(&(instance.kind.clone(), instance.version))
                .ok_or_else(|| {
                    CoreError::new(
                        "unknown-kind",
                        format!("{}: {}@{}", instance.id, instance.kind, instance.version),
                    )
                })?
                .clone();
            let config = instance
                .config
                .clone()
                .unwrap_or_else(|| node.definition.default_config.clone());
            node.implementation
                .validate_config(&config)
                .map_err(|error| {
                    CoreError::new("invalid-config", format!("{}: {error}", instance.id))
                })?;
            planned.push(PlannedNode {
                id: instance.id.clone(),
                node,
                config,
                sources: BTreeMap::new(),
                prepared: None,
            });
        }
        let mut outgoing = vec![BTreeSet::new(); planned.len()];
        let mut indegree = vec![0; planned.len()];
        for edge in &graph.edges {
            let from = *indices
                .get(&edge.from.node)
                .ok_or_else(|| CoreError::new("unknown-node", edge.from.node.clone()))?;
            let to = *indices
                .get(&edge.to.node)
                .ok_or_else(|| CoreError::new("unknown-node", edge.to.node.clone()))?;
            let output = planned[from]
                .node
                .definition
                .outputs
                .iter()
                .find(|port| port.id == edge.from.port)
                .ok_or_else(|| {
                    CoreError::new(
                        "unknown-output",
                        format!("{}.{}", edge.from.node, edge.from.port),
                    )
                })?;
            let input = planned[to]
                .node
                .definition
                .inputs
                .iter()
                .find(|input| input.port.id == edge.to.port)
                .ok_or_else(|| {
                    CoreError::new(
                        "unknown-input",
                        format!("{}.{}", edge.to.node, edge.to.port),
                    )
                })?;
            if output.data_type != input.port.data_type
                || (input.port.language.is_some() && output.language != input.port.language)
                || (input.port.root != RootConstraint::Any && output.root != input.port.root)
            {
                return Err(CoreError::new(
                    "incompatible-ports",
                    format!(
                        "{}.{} -> {}.{}",
                        edge.from.node, edge.from.port, edge.to.node, edge.to.port
                    ),
                ));
            }
            if planned[to]
                .sources
                .insert(edge.to.port.clone(), edge.from.clone())
                .is_some()
            {
                return Err(CoreError::new(
                    "multiple-connections",
                    format!("{}.{}", edge.to.node, edge.to.port),
                ));
            }
            // 同じノード間の複数ポート接続も、実行依存としては1本と数える。
            if outgoing[from].insert(to) {
                indegree[to] += 1;
            }
        }
        for node in &planned {
            for input in &node.node.definition.inputs {
                if input.required && !node.sources.contains_key(&input.port.id) {
                    return Err(CoreError::new(
                        "missing-input",
                        format!("{}.{}", node.id, input.port.id),
                    ));
                }
            }
        }
        let mut ready: VecDeque<_> = indegree
            .iter()
            .enumerate()
            .filter_map(|(index, &count)| (count == 0).then_some(index))
            .collect();
        let mut order = Vec::with_capacity(planned.len());
        while let Some(index) = ready.pop_front() {
            order.push(index);
            for &next in &outgoing[index] {
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    ready.push_back(next);
                }
            }
        }
        if order.len() != planned.len() {
            return Err(CoreError::new("cycle", "graph contains a cycle"));
        }
        for node in &mut planned {
            if let crate::node::RegisteredImplementation::Legacy(implementation) =
                &node.node.implementation
            {
                node.prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    implementation.prepare(&node.config, resources)
                }))
                .unwrap_or_else(|_| {
                    Err(CoreError::new(
                        "node-prepare-panic",
                        "node panicked during preparation",
                    ))
                })
                .map_err(|error| {
                    CoreError::new(error.code, format!("{}: {}", node.id, error.message))
                })?;
            }
        }
        let mut slots: Vec<_> = planned.into_iter().map(Some).collect();
        Ok(Self {
            nodes: order
                .into_iter()
                .map(|index| {
                    slots[index]
                        .take()
                        .expect("topological order contains each node once")
                })
                .collect(),
        })
    }
}
