//! Single-owner protocol-v3 methods. Stream and process ownership are separate.
use super::{
    DecimalId, ExecutionController, ExecutionService, ResourcePolicy, RunBudgets, RunKey,
    StartRequest,
    protocol::{WireState, decode_envelope, encode_reply},
};
use crate::{
    graph::{Endpoint, Graph},
    node::{CoreError, Registry, RuntimeInput},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

fn present_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
fn empty_budgets() -> Value {
    json!([])
}
#[derive(Deserialize)]
#[serde(
    tag = "method",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Request {
    Hello {},
    Definitions {},
    Validate {
        graph: Graph,
    },
    Prepare {
        graph: Graph,
    },
    Activate {
        token: String,
    },
    Rollback {
        expected_plan_version: DecimalId,
    },
    Status {},
    StartRun {
        start_key: DecimalId,
        expected_plan_version: DecimalId,
        targets: Vec<Endpoint>,
        #[serde(default, deserialize_with = "present_value")]
        input: Option<Value>,
        #[serde(default)]
        run_budget_ms: Option<Value>,
        #[serde(default = "empty_budgets")]
        job_budgets: Value,
    },
    GetStart {
        start_key: DecimalId,
    },
    GetRun {
        key: RunKey,
    },
    CancelRun {
        key: RunKey,
    },
    InvalidateInput {
        expected_plan_version: DecimalId,
        revision: u64,
    },
}
struct Prepared {
    token: String,
    graph: Graph,
    order: Vec<String>,
}
/// A private Controller prevents other handle clones from switching this
/// Session's plan/input between an operation and its response state. Independent
/// worker/time events can update runs, but not these six control-state fields.
pub struct SessionV3 {
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
    registry: Arc<Registry>,
    controller: ExecutionController,
    domain: String,
    policy: ResourcePolicy,
    active: Option<Graph>,
    previous: Option<Graph>,
    prepared: Option<Prepared>,
    next_token: u64,
}
impl SessionV3 {
    pub fn new(service: &ExecutionService, registry: Registry) -> Result<Self, CoreError> {
        Self::with_registry(service, Arc::new(registry))
    }
    /// Share immutable node declarations across Sessions without sharing plan state.
    pub fn with_registry(
        service: &ExecutionService,
        registry: Arc<Registry>,
    ) -> Result<Self, CoreError> {
        Self::with_registry_and_resource_root(service, registry, None)
    }
    pub fn with_registry_and_resource_root(
        service: &ExecutionService,
        registry: Arc<Registry>,
        resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            resource_root,
            registry,
            controller: service.open_empty()?,
            domain: service.resource_domain().into(),
            policy: service.resource_policy().clone(),
            active: None,
            previous: None,
            prepared: None,
            next_token: 0,
        })
    }
    pub fn session_epoch(&self) -> &super::SessionEpoch {
        self.controller.session_epoch()
    }
    fn state(&self) -> Result<WireState, CoreError> {
        WireState::from_control(
            self.controller.control_state()?,
            self.prepared.as_ref().map(|p| p.token.clone()),
        )
    }
    fn check_version(&self, expected: DecimalId) -> Result<(), CoreError> {
        if self.controller.plan_version()? != expected {
            return Err(CoreError::new(
                "plan-conflict",
                "active plan changed; refresh the session state",
            ));
        }
        Ok(())
    }
    /// A complete frame in, a complete frame out (neither includes a newline).
    /// Service/serialization faults return Err to the stream owner, which must
    /// close the connection rather than report a fabricated Node result.
    pub fn reply(&mut self, frame: &[u8]) -> Result<Vec<u8>, CoreError> {
        let (id, outcome) = match decode_envelope(frame) {
            Ok(envelope) => {
                let result = serde_json::from_value::<Request>(envelope.request)
                    .map_err(|error| CoreError::new("protocol-request", error.to_string()))
                    .and_then(|request| self.handle(request));
                (Some(envelope.id), result)
            }
            Err(error) => (error.id, Err(error.error)),
        };
        if let Err(error) = &outcome
            && matches!(
                error.code.as_str(),
                "service-fault" | "service-closed" | "service-unavailable" | "protocol-response"
            )
        {
            return Err(error.clone());
        }
        let state = self.state()?;
        encode_reply(id, outcome.as_ref(), &state)
    }
    fn handle(&mut self, request: Request) -> Result<Value, CoreError> {
        match request {
            Request::Hello {} => Ok(json!({
                "sessionEpoch":self.controller.session_epoch(),
                "capabilities":["execution-control-v0.2"],
                "resourceDomain":self.domain,"resourcePolicy":self.policy,
            })),
            Request::Definitions {} => {
                to_value(self.registry.execution_definitions().collect::<Vec<_>>())
            }
            Request::Status {} => Ok(json!({
                "activeGraph":self.active,"preparedGraph":self.prepared.as_ref().map(|p| &p.graph),
                "preparedOrder":self.prepared.as_ref().map(|p| p.order.as_slice()).unwrap_or(&[]),
            })),
            Request::Validate { graph } => {
                let plan = crate::resources::compile(
                    &graph,
                    &self.registry,
                    self.resource_root.as_deref(),
                )?;
                Ok(json!({"valid":true,"order":plan.node_order().collect::<Vec<_>>()}))
            }
            Request::Prepare { graph } => {
                let next = self
                    .next_token
                    .checked_add(1)
                    .ok_or_else(|| CoreError::new("id-exhausted", "preparation token exhausted"))?;
                let plan = crate::resources::compile(
                    &graph,
                    &self.registry,
                    self.resource_root.as_deref(),
                )?;
                let order = plan.node_order().map(str::to_owned).collect();
                self.controller.prepare(plan)?;
                let token = next.to_string();
                self.prepared = Some(Prepared {
                    token: token.clone(),
                    graph,
                    order,
                });
                self.next_token = next;
                Ok(json!({"token":token}))
            }
            Request::Activate { token } => {
                if self.prepared.as_ref().is_none_or(|p| p.token != token) {
                    return Err(CoreError::new(
                        "stale-preparation",
                        "prepare token no longer matches the pending plan",
                    ));
                }
                if !self.controller.activate()? {
                    return Err(CoreError::new("no-pending-plan", "no plan is prepared"));
                }
                self.previous = self.active.take();
                self.active = self.prepared.take().map(|p| p.graph);
                Ok(json!({}))
            }
            Request::Rollback {
                expected_plan_version,
            } => {
                self.check_version(expected_plan_version)?;
                if !self.controller.rollback()? {
                    return Err(CoreError::new(
                        "no-rollback",
                        "no previous plan is available",
                    ));
                }
                self.active = self.previous.take();
                self.prepared = None;
                Ok(json!({}))
            }
            Request::StartRun {
                start_key,
                expected_plan_version,
                targets,
                input,
                run_budget_ms,
                job_budgets,
            } => {
                // Do not precheck current plan/input here: Ledger must resolve
                // an already-accepted key before evaluating new-plan admission.
                let budgets = RunBudgets::from_value(
                    json!({"runBudgetMs":run_budget_ms,"jobBudgets":job_budgets}),
                )?;
                let input = input.map(RuntimeInput::from_value).transpose()?;
                to_value(self.controller.start_run(StartRequest {
                    start_key,
                    expected_plan_version,
                    targets,
                    input,
                    run_budget_ms: budgets.run_budget_ms,
                    job_budgets: budgets.job_budgets,
                })?)
            }
            Request::GetStart { start_key } => {
                Ok(json!({"key":self.controller.get_start(start_key)?}))
            }
            Request::GetRun { key } => to_value(self.controller.get_run(&key)?),
            Request::CancelRun { key } => to_value(self.controller.cancel_run(&key)?),
            Request::InvalidateInput {
                expected_plan_version,
                revision,
            } => {
                self.controller
                    .invalidate_input(expected_plan_version, revision)?;
                Ok(json!({}))
            }
        }
    }
}
fn to_value(value: impl Serialize) -> Result<Value, CoreError> {
    serde_json::to_value(value)
        .map_err(|error| CoreError::new("protocol-response", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhausted_preparation_tokens_leave_the_core_and_pending_graph_unchanged() {
        let policy = serde_json::from_str::<Value>(include_str!(
            "../../../../tests/contracts/execution_control.json"
        ))
        .unwrap()["resourceProfiles"]["bounded"]
            .clone();
        let host = super::super::ExecutionHost::new(
            "v3-token-exhaustion",
            ResourcePolicy::from_value(policy).unwrap(),
        )
        .unwrap();
        let service = ExecutionService::with_clock(
            host,
            super::super::ExecutionClock::controlled(std::time::Duration::ZERO),
        )
        .unwrap();
        let mut session = SessionV3::new(&service, Registry::new()).unwrap();
        let graph = Graph::from_json(r#"{"version":1,"nodes":[],"edges":[]}"#).unwrap();
        session
            .handle(Request::Prepare {
                graph: graph.clone(),
            })
            .unwrap();
        session.next_token = u64::MAX;
        let before = session.state().unwrap();
        assert_eq!(
            session.handle(Request::Prepare { graph }).unwrap_err().code,
            "id-exhausted"
        );
        assert_eq!(session.state().unwrap(), before);
        session
            .handle(Request::Activate { token: "1".into() })
            .unwrap();
        assert_eq!(session.controller.plan_version().unwrap().get(), 1);
    }
}
