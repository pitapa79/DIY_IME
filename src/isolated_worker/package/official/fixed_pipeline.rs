//! Static code/topology evidence only. Never an execution grant, prepared
//! dictionary, resource-API compatibility claim, or permission to retain a Worker.
use super::{Bundle, cancelled};
use crate::{
    execution_control::CancellationToken,
    graph::Graph,
    isolated_worker::package::{
        Package, PackageLimits, VerifiedPackage, permissions::PermissionPlan,
    },
    node::CoreError,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

const KINDS: [&str; 5] = [
    "input.text",
    "text.romaji-to-hiragana",
    "text.dictionary-exact",
    "text.select-candidates",
    "text.stringify",
];

pub struct OfficialStage {
    id: String,
    config: Value,
    package: VerifiedPackage,
}
impl OfficialStage {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn config(&self) -> &Value {
        &self.config
    }
    pub fn package(&self) -> &VerifiedPackage {
        &self.package
    }
}

/// Tied to one actual compiled PermissionPlan, not equivalent JSON or a name.
/// No Deserialize, constructor from hashes, or way to approve a grant here.
///
/// ```compile_fail
/// let _: diy_ime2_core::isolated_worker::package::StaticOfficialPipeline =
///     serde_json::from_str("{}").unwrap();
/// ```
pub struct StaticOfficialPipeline {
    input_id: String,
    stages: [OfficialStage; 4],
    plan_identity: Arc<()>,
}
impl StaticOfficialPipeline {
    pub fn input_id(&self) -> &str {
        &self.input_id
    }
    pub fn stages(&self) -> &[OfficialStage; 4] {
        &self.stages
    }
    pub fn matches_plan(&self, plan: &PermissionPlan) -> bool {
        Arc::ptr_eq(&self.plan_identity, &plan.snapshot_identity())
    }
}

/// Normal permission review plus optional static evidence. A missing match
/// leaves the normal compiled plan available, without changing its contracts.
pub struct OfficialGraphReview {
    plan: PermissionPlan,
    pipeline: Option<StaticOfficialPipeline>,
}
impl OfficialGraphReview {
    /// Preparation-thread operation. Uses already isolated, verified packages;
    /// validates the graph with the existing Core and captures the Host-pinned
    /// bundle only for the exact shape. No dictionary bytes or grants are read.
    /// The compiled distribution pin, not the directory name, creates identity.
    pub fn capture(
        graph: &Graph,
        external: &BTreeMap<String, VerifiedPackage>,
        official_directory: &Path,
        limits: &PackageLimits,
        cancel: CancellationToken,
    ) -> Result<Self, CoreError> {
        let plan = PermissionPlan::capture_with_cancel(graph, external, cancel.clone())?;
        let Some(ids) = shape(graph) else {
            return Ok(Self {
                plan,
                pipeline: None,
            });
        };
        let bundle = Bundle::capture(official_directory, limits, &cancel)?;
        let mut stages = Vec::new();
        for (index, id) in ids[1..].iter().enumerate() {
            cancelled(&cancel)?;
            // PermissionPlan already proved each external map entry corresponds
            // to that instance and the actual Wasm definition/configuration.
            let Some(verified) = external.get(id) else {
                return Ok(Self {
                    plan,
                    pipeline: None,
                });
            };
            let kind = KINDS[index + 1];
            let Some(expected) = bundle
                .packages
                .iter()
                .find(|p| p.manifest().node.kind == kind)
            else {
                return Ok(Self {
                    plan,
                    pipeline: None,
                });
            };
            if !same_code(verified.package(), expected, index == 1) {
                return Ok(Self {
                    plan,
                    pipeline: None,
                });
            }
            let instance = graph.nodes.iter().find(|node| node.id == *id).unwrap();
            stages.push(OfficialStage {
                id: id.clone(),
                config: instance
                    .config
                    .as_ref()
                    .unwrap_or(&expected.manifest().node.default_config)
                    .clone(),
                package: verified.clone(),
            });
        }
        cancelled(&cancel)?;
        let pipeline = StaticOfficialPipeline {
            input_id: ids[0].clone(),
            stages: stages
                .try_into()
                .unwrap_or_else(|_| unreachable!("four fixed stages")),
            plan_identity: plan.snapshot_identity(),
        };
        Ok(Self {
            plan,
            pipeline: Some(pipeline),
        })
    }
    pub fn permission_plan(&self) -> &PermissionPlan {
        &self.plan
    }
    pub fn pipeline(&self) -> Option<&StaticOfficialPipeline> {
        self.pipeline.as_ref()
    }
    pub fn into_parts(self) -> (PermissionPlan, Option<StaticOfficialPipeline>) {
        (self.plan, self.pipeline)
    }
}

fn same_code(actual: &Package, expected: &Package, dictionary: bool) -> bool {
    actual.digest() == expected.digest()
        && actual.bytes() == expected.bytes()
        && if dictionary {
            actual.manifest().capabilities.len() == 1
                && actual.manifest().capabilities[0].id == "dictionary.read"
                && actual.manifest().capabilities[0].binding == "dictionary"
        } else {
            actual.manifest().capabilities.is_empty()
        }
}

fn shape(graph: &Graph) -> Option<[String; 5]> {
    if graph.version != 1 || graph.nodes.len() != 5 || graph.edges.len() != 4 {
        return None;
    }
    let mut ids = Vec::new();
    for kind in KINDS {
        let mut nodes = graph
            .nodes
            .iter()
            .filter(|node| node.kind == kind && node.version == 1);
        let node = nodes.next()?;
        if nodes.next().is_some() {
            return None;
        }
        ids.push(node.id.clone());
    }
    if ids.iter().collect::<BTreeSet<_>>().len() != 5 {
        return None;
    }
    let edges: BTreeSet<_> = graph
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from.node.as_str(),
                edge.from.port.as_str(),
                edge.to.node.as_str(),
                edge.to.port.as_str(),
            )
        })
        .collect();
    if edges.len() != 4
        || !ids
            .windows(2)
            .all(|pair| edges.contains(&(pair[0].as_str(), "out", pair[1].as_str(), "in")))
    {
        return None;
    }
    ids.try_into().ok()
}

#[cfg(test)]
mod tests;
