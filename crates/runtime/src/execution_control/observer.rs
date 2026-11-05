//! A host-driven observer boundary; scheduling wakeups belongs to the controller.
use super::{
    ControlDecision, ControlFrame, ControlInput, DecimalId, OwnedInvocation, ResourceLease,
    WakeReason,
};
use crate::{
    error_input::{ErrorAwareInputs, ErrorAwareValue, merge_errors},
    node::{CoreError, RegisteredImplementation},
    runtime::{self, NodeExecution, NodeStatus},
};
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::Duration,
};

pub enum ControlObservation {
    Wait { wake_at: Option<Duration> },
    Finished(Box<NodeExecution>),
}
enum PreparedDecision {
    Wait(Option<super::Milliseconds>),
    Finished(Box<NodeExecution>),
}
/// A single-use decision bound to one observer and its latest Frame. Preparation
/// validates policy output but does not end observer ownership or anchor Wait.
pub struct PreparedObservation {
    authority: Arc<()>,
    sequence: DecimalId,
    observed_at: Duration,
    decision: PreparedDecision,
}
impl PreparedObservation {
    pub(crate) fn execution(&self) -> Option<&NodeExecution> {
        match &self.decision {
            PreparedDecision::Finished(execution) => Some(execution),
            _ => None,
        }
    }
}
pub struct ControlObserver {
    authority: Arc<()>,
    invocation: OwnedInvocation,
    accepted_at: Duration,
    last_time: Duration,
    last_sequence: Option<DecimalId>,
    lease: Option<ResourceLease>,
}
impl ControlObserver {
    pub(crate) fn new(
        invocation: OwnedInvocation,
        accepted_at: Duration,
        lease: ResourceLease,
    ) -> Self {
        Self {
            authority: Arc::new(()),
            invocation,
            accepted_at,
            last_time: accepted_at,
            last_sequence: None,
            lease: Some(lease),
        }
    }
    /// Core/host-only input. Does not run timers or inspect worker completion queues.
    pub fn observe(
        &mut self,
        inputs: BTreeMap<String, ControlInput>,
        reason: WakeReason,
        event_seq: DecimalId,
        now: Duration,
    ) -> Result<ControlObservation, CoreError> {
        let prepared = self.prepare(inputs, reason, event_seq, now)?;
        self.commit(prepared, now)
    }
    pub fn prepare(
        &mut self,
        inputs: BTreeMap<String, ControlInput>,
        reason: WakeReason,
        event_seq: DecimalId,
        now: Duration,
    ) -> Result<PreparedObservation, CoreError> {
        if self.lease.is_none() {
            return Err(CoreError::new("observer-finished", "observer is terminal"));
        }
        if now < self.last_time
            || self.last_sequence.is_some_and(|last| event_seq <= last)
            || now.saturating_sub(self.accepted_at).as_millis()
                > u128::from(super::types::MAX_SAFE_INTEGER)
        {
            return Err(CoreError::new(
                "invalid-observation",
                "time and event sequence must advance monotonically",
            ));
        }
        let planned = &self.invocation.plan.nodes[self.invocation.index];
        if inputs.len() != planned.node.definition.inputs.len()
            || planned
                .node
                .definition
                .inputs
                .iter()
                .any(|port| !inputs.contains_key(&port.port.id))
        {
            return Err(CoreError::new(
                "invalid-observation",
                "every declared input needs a pending/resolved state",
            ));
        }
        let mut resolved = ErrorAwareInputs::new();
        let mut errors = None;
        let mut blocked = None;
        let mut missing = false;
        let mut pending = false;
        for port in &planned.node.definition.inputs {
            match &inputs[&port.port.id] {
                ControlInput::Pending => pending = true,
                ControlInput::Resolved(value) => {
                    match value {
                        ErrorAwareValue::Data(packet) => {
                            packet.validate().map_err(|error| {
                                CoreError::new("invalid-packet", error.to_string())
                            })?;
                            if !port.port.root.accepts(&packet.root) {
                                return Err(CoreError::new(
                                    "invalid-root",
                                    "observer input violates port constraint",
                                ));
                            }
                        }
                        ErrorAwareValue::NoOutput => missing |= port.required,
                        ErrorAwareValue::Error(set) => {
                            if set.causes().iter().any(|cause| {
                                cause.id.run_id != self.invocation.key.run_id.get()
                                    || cause.id.plan_version
                                        != self.invocation.key.plan_version.get()
                            }) {
                                return Err(CoreError::new(
                                    "invalid-observation",
                                    "error belongs to another run",
                                ));
                            }
                            merge_errors(&mut errors, set);
                            if !planned.node.implementation.receives_error(&port.port.id) {
                                blocked.get_or_insert_with(|| set.causes()[0].diagnostic.clone());
                            }
                        }
                    }
                    resolved.insert(port.port.id.clone(), value.clone());
                }
            }
        }
        self.last_sequence = Some(event_seq);
        self.last_time = now;
        self.invocation.inputs = resolved;
        self.invocation.input_errors = errors;
        // Preserve legacy E-before-R priority. Known blocked/required-NoOutput input
        // cannot invoke policy while another input is still pending.
        if pending && (blocked.is_some() || missing) {
            return Ok(self.prepared(PreparedDecision::Wait(None), event_seq, now));
        }
        let planned = &self.invocation.plan.nodes[self.invocation.index];
        let execution = if let Some(diagnostic) = blocked {
            Some(runtime::failed(
                planned,
                NodeStatus::Blocked,
                diagnostic,
                self.invocation.input_errors.clone(),
            ))
        } else if missing {
            Some(runtime::empty_execution(planned))
        } else {
            let RegisteredImplementation::Control { node, .. } = &planned.node.implementation
            else {
                unreachable!("validated control declaration")
            };
            let frame = ControlFrame {
                inputs,
                reason,
                event_seq,
                elapsed_ms: now.saturating_sub(self.accepted_at).as_millis() as u64,
            };
            let decision = catch_unwind(AssertUnwindSafe(|| {
                node.observe(self.invocation.context(), &frame, &planned.config)
            }))
            .unwrap_or_else(|_| Err(CoreError::new("node-panic", "control observer panicked")));
            let outcome = match decision {
                Ok(ControlDecision::Wait { wake_after_ms }) => {
                    return Ok(self.prepared(
                        PreparedDecision::Wait(wake_after_ms),
                        event_seq,
                        now,
                    ));
                }
                Ok(ControlDecision::Finish(outcome)) => Ok(outcome),
                Err(error) => Err(error),
            };
            Some(runtime::resolve_outcome(
                planned,
                self.invocation.context(),
                &self.invocation.inputs,
                &self.invocation.input_recoveries,
                self.invocation.input_errors.as_ref(),
                outcome,
            ))
        };
        let mut execution = execution.expect("finish path");
        execution.input_errors = self.invocation.input_errors.clone();
        Ok(self.prepared(
            PreparedDecision::Finished(Box::new(execution)),
            event_seq,
            now,
        ))
    }
    fn prepared(
        &self,
        decision: PreparedDecision,
        sequence: DecimalId,
        observed_at: Duration,
    ) -> PreparedObservation {
        PreparedObservation {
            authority: Arc::clone(&self.authority),
            sequence,
            observed_at,
            decision,
        }
    }
    /// Commit after the host rechecks time and input changes. A Wait is anchored
    /// here, not before callback/validation; a stale token never releases a lease.
    pub fn commit(
        &mut self,
        prepared: PreparedObservation,
        now: Duration,
    ) -> Result<ControlObservation, CoreError> {
        if self.lease.is_none()
            || !Arc::ptr_eq(&prepared.authority, &self.authority)
            || self.last_sequence != Some(prepared.sequence)
        {
            return Err(CoreError::new(
                "observation-stale",
                "decision does not belong to the current observer Frame",
            ));
        }
        if now < prepared.observed_at
            || now < self.last_time
            || now.saturating_sub(self.accepted_at).as_millis()
                > u128::from(super::types::MAX_SAFE_INTEGER)
        {
            return Err(CoreError::new(
                "invalid-observation",
                "commit time is not representable or moved backwards",
            ));
        }
        self.last_time = now;
        let execution = match prepared.decision {
            PreparedDecision::Wait(delay) => {
                match delay.map(|delay| delay.checked_deadline(now)).transpose() {
                    Ok(wake_at) => return Ok(ControlObservation::Wait { wake_at }),
                    Err(error) => {
                        let planned = &self.invocation.plan.nodes[self.invocation.index];
                        let mut execution = runtime::resolve_outcome(
                            planned,
                            self.invocation.context(),
                            &self.invocation.inputs,
                            &self.invocation.input_recoveries,
                            self.invocation.input_errors.as_ref(),
                            Err(error),
                        );
                        execution.input_errors = self.invocation.input_errors.clone();
                        Box::new(execution)
                    }
                }
            }
            PreparedDecision::Finished(execution) => execution,
        };
        self.lease.take();
        Ok(ControlObservation::Finished(execution))
    }
    pub fn key(&self) -> &super::RunKey {
        self.invocation.key()
    }
    pub fn node_id(&self) -> &str {
        self.invocation.context().node_id
    }
    pub(crate) fn set_input_recoveries(
        &mut self,
        records: BTreeMap<String, Vec<crate::error_input::RecoveryRecord>>,
    ) {
        self.invocation.input_recoveries = records;
    }
}
