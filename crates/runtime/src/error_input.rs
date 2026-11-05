//! Opt-in synchronous error boundary. Not transported by session protocol 2.
use crate::{
    Packet,
    graph::Endpoint,
    node::{CoreError, NodeDefinition, RunContext},
    runtime::Diagnostic,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CauseId {
    pub run_id: u64,
    pub plan_version: u64,
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorHop {
    pub from: Endpoint,
    pub to: Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorCause {
    pub id: CauseId,
    pub diagnostic: Diagnostic,
    pub paths: Vec<ErrorHop>,
}

/// Nonempty; callers can only retain or merge errors actually issued by Engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ErrorSet(Vec<ErrorCause>);

impl ErrorSet {
    pub fn causes(&self) -> &[ErrorCause] {
        &self.0
    }
    pub fn merge(&self, other: &Self) -> Self {
        let mut result = self.clone();
        for cause in &other.0 {
            if let Some(existing) = result.0.iter_mut().find(|item| item.id == cause.id) {
                for hop in &cause.paths {
                    if !existing.paths.contains(hop) {
                        existing.paths.push(hop.clone());
                    }
                }
            } else {
                result.0.push(cause.clone());
            }
        }
        result
    }
    pub(crate) fn new(diagnostic: Diagnostic) -> Self {
        Self(vec![ErrorCause {
            id: CauseId {
                run_id: diagnostic.run_id,
                plan_version: diagnostic.plan_version,
                node_id: diagnostic.node_id.clone(),
            },
            diagnostic,
            paths: vec![],
        }])
    }
    pub(crate) fn routed(&self, from: Endpoint, to: Endpoint) -> Self {
        let mut result = self.clone();
        let hop = ErrorHop { from, to };
        for cause in &mut result.0 {
            if !cause.paths.contains(&hop) {
                cause.paths.push(hop.clone());
            }
        }
        result
    }
    pub(crate) fn is_subset_of(&self, other: &Self) -> bool {
        self.0.iter().all(|cause| {
            other.0.iter().any(|allowed| {
                allowed.id == cause.id
                    && allowed.diagnostic == cause.diagnostic
                    && cause.paths.iter().all(|hop| allowed.paths.contains(hop))
            })
        })
    }
}

pub(crate) fn merge_errors(target: &mut Option<ErrorSet>, errors: &ErrorSet) {
    *target = Some(match target.take() {
        Some(previous) => previous.merge(errors),
        None => errors.clone(),
    });
}

#[derive(Debug, Clone)]
pub enum ErrorAwareValue {
    Data(Arc<Packet>),
    NoOutput,
    Error(ErrorSet),
}
pub type ErrorAwareInputs = BTreeMap<String, ErrorAwareValue>;
pub type ErrorAwareOutputs = BTreeMap<String, ErrorAwareValue>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryClaim {
    pub input: String,
    pub output: String,
    pub causes: Vec<CauseId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRecord {
    pub node_id: String,
    pub input: String,
    pub output: String,
    pub causes: Vec<CauseId>,
}

#[derive(Debug, Clone, Default)]
pub struct ErrorAwareExecution {
    pub outputs: ErrorAwareOutputs,
    pub recoveries: Vec<RecoveryClaim>,
    /// Omitted output: conservative dependence on all Data inputs. Explicit []: none.
    pub dependencies: BTreeMap<String, Vec<String>>,
}
impl ErrorAwareExecution {
    pub fn new(outputs: ErrorAwareOutputs) -> Self {
        Self {
            outputs,
            ..Self::default()
        }
    }
}

pub trait ErrorAwareNode: Send + Sync {
    fn definition(&self) -> NodeDefinition;
    fn error_inputs(&self) -> Vec<String>;
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if config == &json!({}) {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "expected empty config"))
        }
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        inputs: &ErrorAwareInputs,
        config: &Value,
    ) -> Result<ErrorAwareExecution, CoreError>;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorAwareDefinition {
    pub version: u32,
    pub definition: NodeDefinition,
    pub error_inputs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    Complete,
    Recovered,
    NoOutput,
    Failed,
}

pub struct TargetedRunReport {
    pub report: crate::runtime::RunReport,
    pub targets: Vec<Endpoint>,
    pub delivery: Delivery,
}
impl TargetedRunReport {
    pub fn to_json(&self) -> Result<String, CoreError> {
        let mut value: Value = serde_json::from_str(&self.report.to_json()?)
            .map_err(|error| CoreError::new("report-json", error.to_string()))?;
        let mut causes = None;
        let mut recoveries = vec![];
        for node in self.report.nodes.values() {
            if let (crate::runtime::NodeStatus::Failed, Some(diagnostic)) =
                (node.status, &node.diagnostic)
            {
                merge_errors(&mut causes, &ErrorSet::new(diagnostic.clone()));
            }
            if let Some(errors) = &node.input_errors {
                merge_errors(&mut causes, errors);
            }
            for errors in node.error_outputs.values() {
                merge_errors(&mut causes, errors);
            }
            for record in &node.recoveries {
                if !recoveries.contains(record) {
                    recoveries.push(record.clone());
                }
            }
        }
        value["executionControlVersion"] = json!(1);
        value["targets"] = json!(self.targets);
        value["delivery"] = json!(self.delivery);
        value["causes"] = json!(causes.map(|errors| errors.0).unwrap_or_default());
        value["recoveries"] = json!(recoveries);
        serde_json::to_string_pretty(&value)
            .map_err(|error| CoreError::new("report-json", error.to_string()))
    }
}
