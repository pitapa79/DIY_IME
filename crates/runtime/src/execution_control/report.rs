use super::{DecimalId, RunKey};
use crate::{
    Packet,
    graph::{Endpoint, ExecutionPlan},
    node::CoreError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunPhase {
    Accepted,
    Running,
    Terminal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalReason {
    Completed,
    Cancelled,
    Expired,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunDelivery {
    Complete,
    Recovered,
    NoOutput,
    Failed,
    Cancelled,
    Expired,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalCounts {
    pub running: u64,
    pub cancel_requested: u64,
    pub stalled: u64,
    pub queued: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedPort {
    pub endpoint: Endpoint,
    pub value: Value,
}
/// Only trusted Core/host code supplies completion results. Not a v3 request.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalResult {
    pub reason: TerminalReason,
    pub delivery: RunDelivery,
    pub targets: Vec<Endpoint>,
    pub ports: Vec<ResolvedPort>,
    pub nodes: Vec<Value>,
    pub causes: Vec<Value>,
    pub recoveries: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CoreError>,
}
impl TerminalResult {
    pub fn completed(
        targets: Vec<Endpoint>,
        ports: Vec<ResolvedPort>,
        delivery: RunDelivery,
    ) -> Self {
        Self {
            reason: TerminalReason::Completed,
            delivery,
            targets,
            ports,
            nodes: vec![],
            causes: vec![],
            recoveries: vec![],
            error: None,
        }
    }
    pub(crate) fn aborted(targets: &[Endpoint], reason: TerminalReason) -> Self {
        let (delivery, label) = if reason == TerminalReason::Expired {
            (RunDelivery::Expired, "expired")
        } else {
            (RunDelivery::Cancelled, "cancelled")
        };
        Self {
            reason,
            delivery,
            targets: targets.to_vec(),
            ports: targets
                .iter()
                .map(|target| ResolvedPort {
                    endpoint: target.clone(),
                    value: json!({"status":"discarded","reason":label}),
                })
                .collect(),
            nodes: vec![],
            causes: vec![],
            recoveries: vec![],
            error: None,
        }
    }
    pub(crate) fn limited(targets: &[Endpoint]) -> Self {
        Self {
            reason: TerminalReason::Failed,
            delivery: RunDelivery::Failed,
            targets: targets.to_vec(),
            ports: vec![],
            nodes: vec![],
            causes: vec![],
            recoveries: vec![],
            error: Some(CoreError::new(
                "result-limit",
                "result exceeded reserved bytes",
            )),
        }
    }
    pub(crate) fn validate(
        &self,
        plan: &ExecutionPlan,
        targets: &[Endpoint],
    ) -> Result<(), CoreError> {
        let bad = || {
            CoreError::new(
                "invalid-result",
                "terminal result violates targets or declared output contract",
            )
        };
        if self.targets != targets {
            return Err(bad());
        }
        if self.reason != TerminalReason::Completed {
            return Err(bad());
        }
        if self.error.is_some()
            || matches!(self.delivery, RunDelivery::Cancelled | RunDelivery::Expired)
        {
            return Err(bad());
        }
        let mut endpoints = std::collections::BTreeSet::new();
        for output in &self.ports {
            if !endpoints.insert((&output.endpoint.node, &output.endpoint.port)) {
                return Err(bad());
            }
            let declared = plan
                .nodes
                .iter()
                .find(|node| node.id == output.endpoint.node)
                .and_then(|node| {
                    node.node
                        .definition
                        .outputs
                        .iter()
                        .find(|port| port.id == output.endpoint.port)
                })
                .ok_or_else(bad)?;
            match output.value["status"].as_str() {
                Some("data") => {
                    let packet = Packet::from_json(&output.value["packet"].to_string())
                        .map_err(|_| bad())?;
                    if !declared.root.accepts(&packet.root) {
                        return Err(bad());
                    }
                }
                Some("error") => {
                    if output.value["causes"]
                        .as_array()
                        .is_none_or(|items| items.is_empty())
                    {
                        return Err(bad());
                    }
                }
                Some("no-output") => {}
                Some("discarded") if !targets.contains(&output.endpoint) => {}
                _ => return Err(bad()),
            }
        }
        let mut has_error = false;
        let mut no_output = false;
        for target in targets {
            let value = &self
                .ports
                .iter()
                .find(|port| &port.endpoint == target)
                .ok_or_else(bad)?
                .value;
            has_error |= value["status"] == "error";
            no_output |= value["status"] == "no-output";
        }
        let valid = if has_error {
            self.delivery == RunDelivery::Failed
        } else if no_output {
            self.delivery == RunDelivery::NoOutput
        } else {
            matches!(
                self.delivery,
                RunDelivery::Complete | RunDelivery::Recovered
            )
        };
        if !valid || (self.delivery == RunDelivery::Recovered && self.recoveries.is_empty()) {
            return Err(bad());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub key: RunKey,
    pub state: RunPhase,
    pub event_seq: DecimalId,
    pub elapsed_ms: u64,
    pub current: bool,
    pub physical: PhysicalCounts,
    pub poll_after_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TerminalResult>,
}
#[derive(Debug, Clone, Serialize)]
pub struct StartReceipt {
    pub state: RunPhase,
    pub replayed: bool,
    pub key: RunKey,
}

pub(crate) fn retained_bytes(
    key: &RunKey,
    elapsed_ms: u64,
    result: &TerminalResult,
) -> Result<u64, CoreError> {
    serde_json::to_vec(&json!({"key":key,"elapsedMs":elapsed_ms,"result":result}))
        .map(|data| data.len() as u64)
        .map_err(|error| CoreError::new("invalid-result", error.to_string()))
}
