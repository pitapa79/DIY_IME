use super::worker::CompletionPermit;
use super::{CancellationToken, DecimalId, Milliseconds, RunKey};
use crate::{
    error_input::{
        ErrorAwareExecution, ErrorAwareInputs, ErrorAwareValue, ErrorSet, RecoveryRecord,
    },
    graph::ExecutionPlan,
    node::{CoreError, NodeDefinition, RunContext, RuntimeInput},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

/// Host-clock intervals only. These never establish execution or OS authority.
/// Not part of the guest ABI or the pinned Node SDK.
#[derive(Clone, Copy)]
pub enum HostTracePhase {
    Request,
    WorkerStaging,
    WorkerProcess,
    WorkerCleanup,
    BrokerExchange,
}
impl HostTracePhase {
    fn events(self) -> (super::TraceKind, super::TraceKind) {
        use super::TraceKind::*;
        match self {
            Self::Request => (HostRequestStarted, HostRequestFinished),
            Self::WorkerStaging => (WorkerStagingStarted, WorkerStagingFinished),
            Self::WorkerProcess => (WorkerProcessStarted, WorkerProcessFinished),
            Self::WorkerCleanup => (WorkerCleanupStarted, WorkerCleanupFinished),
            Self::BrokerExchange => (BrokerExchangeStarted, BrokerExchangeFinished),
        }
    }
}
/// Scope exit includes errors/unwinding; it does not mean success or process exit.
pub struct HostTraceSpan<'a> {
    invocation: Option<&'a OwnedInvocation>,
    finished: super::TraceKind,
}
impl Drop for HostTraceSpan<'_> {
    fn drop(&mut self) {
        if let Some(i) = self.invocation {
            i.trace_event(self.finished, None);
        }
    }
}

#[derive(Clone)]
pub struct OwnedInvocation {
    pub(crate) trace: Option<super::trace::TraceTicket>,
    pub(crate) key: RunKey,
    pub(crate) plan: Arc<ExecutionPlan>,
    pub(crate) index: usize,
    pub(crate) input: Option<Arc<RuntimeInput>>,
    pub(crate) inputs: ErrorAwareInputs,
    pub(crate) input_recoveries: BTreeMap<String, Vec<RecoveryRecord>>,
    pub(crate) input_errors: Option<ErrorSet>,
}
impl OwnedInvocation {
    pub fn host_trace_is_detailed(&self) -> bool {
        self.trace.as_ref().is_some_and(|t| t.is_detailed())
    }
    /// Worker-local elapsed times, received after OS exit; never Host timestamps.
    pub fn trace_worker_interval(
        &self,
        instance: u32,
        phase: super::WorkerTracePhase,
        started: std::time::Duration,
        finished: Option<std::time::Duration>,
    ) {
        if !self.host_trace_is_detailed() {
            return;
        }
        if instance == 0
            || instance > 20_000
            || finished.is_some_and(|end| end < started)
            || started.as_micros() > u128::from(u64::MAX)
            || finished.is_some_and(|end| end.as_micros() > u128::from(u64::MAX))
        {
            self.trace_worker_diagnostics_lost();
            return;
        }
        let p = &self.plan.nodes[self.index];
        self.trace.as_ref().expect("enabled ticket").emit_worker(
            (
                self.index,
                &p.node.definition.kind,
                p.node.definition.version,
            ),
            super::WorkerTraceInterval {
                instance,
                phase,
                started_us: started.as_micros().to_string(),
                finished_us: finished.map(|d| d.as_micros().to_string()),
            },
        );
    }
    pub fn trace_worker_diagnostics_lost(&self) {
        if let Some(t) = &self.trace {
            t.lost_diagnostics();
        }
    }
    /// Internal native-Host bridge. Queue one bounded received metadata bundle
    /// without competing with execution events for the recording mutex.
    #[doc(hidden)]
    pub fn trace_worker_intervals(
        &self,
        instance: u32,
        intervals: impl IntoIterator<
            Item = (
                super::WorkerTracePhase,
                std::time::Duration,
                Option<std::time::Duration>,
            ),
        >,
    ) {
        if !self.host_trace_is_detailed() {
            return;
        }
        let mut received = Vec::new();
        for (phase, start, end) in intervals
            .into_iter()
            .take(super::trace::MAX_WORKER_INTERVALS + 1)
        {
            if instance == 0
                || instance > 20_000
                || end.is_some_and(|end| end < start)
                || start.as_micros() > u128::from(u64::MAX)
                || end.is_some_and(|end| end.as_micros() > u128::from(u64::MAX))
            {
                self.trace_worker_diagnostics_lost();
                return;
            }
            received.push((
                phase,
                start.as_micros() as u64,
                end.map(|end| end.as_micros() as u64),
            ));
        }
        if received.is_empty() || received.len() > super::trace::MAX_WORKER_INTERVALS {
            self.trace_worker_diagnostics_lost();
            return;
        }
        let p = &self.plan.nodes[self.index];
        self.trace.as_ref().expect("enabled ticket").queue_workers(
            (
                self.index,
                &p.node.definition.kind,
                p.node.definition.version,
            ),
            instance,
            received.into_boxed_slice(),
        );
    }
    /// Trusted Host metadata. Disabled/basic recording does no clock read or formatting.
    pub fn trace_host_phase(&self, phase: HostTracePhase) -> HostTraceSpan<'_> {
        let (started, finished) = phase.events();
        let enabled = self.trace.as_ref().is_some_and(|t| t.is_detailed());
        if enabled {
            self.trace_event(started, None);
        }
        HostTraceSpan {
            invocation: enabled.then_some(self),
            finished,
        }
    }
    pub(crate) fn trace_event(&self, event: super::TraceKind, job: Option<u64>) {
        if let Some(t) = &self.trace {
            let p = &self.plan.nodes[self.index];
            t.emit(
                event,
                Some((
                    self.index,
                    &p.node.definition.kind,
                    p.node.definition.version,
                )),
                job,
            );
        }
    }
    pub fn key(&self) -> &RunKey {
        &self.key
    }
    pub fn context(&self) -> RunContext<'_> {
        RunContext {
            run_id: self.key.run_id.get(),
            plan_version: self.key.plan_version.get(),
            node_id: &self.plan.nodes[self.index].id,
            input: self.input.as_deref(),
        }
    }
    pub fn inputs(&self) -> &ErrorAwareInputs {
        &self.inputs
    }
    pub fn config(&self) -> &Value {
        &self.plan.nodes[self.index].config
    }
}
#[derive(Debug, Clone)]
pub enum ControlInput {
    Pending,
    Resolved(ErrorAwareValue),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    Started,
    Input,
    Timer,
}
#[derive(Debug, Clone)]
pub struct ControlFrame {
    pub inputs: BTreeMap<String, ControlInput>,
    pub reason: WakeReason,
    pub event_seq: DecimalId,
    pub elapsed_ms: u64,
}
#[derive(Debug, Clone)]
pub enum ControlDecision {
    Wait { wake_after_ms: Option<Milliseconds> },
    Finish(ErrorAwareExecution),
}
pub trait DeferredNode: Send + Sync {
    fn definition(&self) -> NodeDefinition;
    fn error_inputs(&self) -> Vec<String> {
        vec![]
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        empty_config(config)
    }
    /// Return quickly. Complete/finish_cancelled asserts physical work has ended.
    fn start(
        &self,
        invocation: OwnedInvocation,
        completion: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError>;
}
pub trait ControlNode: Send + Sync {
    fn definition(&self) -> NodeDefinition;
    fn error_inputs(&self) -> Vec<String> {
        vec![]
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        empty_config(config)
    }
    /// Trusted, short, nonblocking policy. A slow callback is not forcibly interruptible.
    fn observe(
        &self,
        context: RunContext<'_>,
        frame: &ControlFrame,
        config: &Value,
    ) -> Result<ControlDecision, CoreError>;
}
fn empty_config(config: &Value) -> Result<(), CoreError> {
    if config == &json!({}) {
        Ok(())
    } else {
        Err(CoreError::new("invalid-config", "expected empty config"))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionKind {
    Sync,
    Deferred,
    Control,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionDefinition {
    pub version: u32,
    pub execution_kind: ExecutionKind,
    pub definition: NodeDefinition,
    pub error_inputs: Vec<String>,
}
