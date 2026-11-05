//! Host-driven dependency and demand state. No thread/queue/clock service here.
use super::{
    ControlInput, ControlObservation, ControlObserver, DecimalId, ExecutionHost, ExecutionKind,
    ExecutionResults, OwnedInvocation, ResourceUsage, RunKey, RunLedger, RunSnapshot,
    TerminalReason, WakeReason, WorkerCompletion,
};
use crate::{
    error_input::{ErrorAwareInputs, ErrorAwareValue, ErrorSet, RecoveryRecord, merge_errors},
    graph::Endpoint,
    node::{CoreError, RegisteredImplementation, RuntimeInput},
    runtime::{self, Diagnostic, NodeExecution, NodeStatus, ResolvedValue},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphNodePhase {
    Pending,
    Active,
    Resolved,
    Discarded,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyNode {
    pub node_id: String,
    pub execution_kind: ExecutionKind,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasedNode {
    pub node_id: String,
    pub execution_kind: ExecutionKind,
    pub job_id: Option<DecimalId>,
}
pub enum GraphObservation {
    Wait { wake_at: Option<Duration> },
    Finished { released: Vec<ReleasedNode> },
}
pub struct GraphCompletion {
    pub snapshot: RunSnapshot,
    pub released: Vec<ReleasedNode>,
}
/// Graph-issued validated completion. Its fields and graph identity are private.
pub struct PreparedGraphCompletion {
    authority: Arc<()>,
    stamp: super::JobStamp,
    node: String,
    execution: NodeExecution,
}
pub struct PreparedGraphObservation {
    authority: Arc<()>,
    node: String,
    frame: Vec<bool>,
    decision: super::PreparedObservation,
    validated_finish: bool,
}
struct ResolvedInputs {
    inputs: ErrorAwareInputs,
    recoveries: BTreeMap<String, Vec<RecoveryRecord>>,
    errors: Option<ErrorSet>,
    blocked: Option<Diagnostic>,
    missing: bool,
}
pub struct GraphExecution {
    pub(crate) trace: Option<super::trace::TraceTicket>,
    authority: Arc<()>,
    pub(crate) results: ExecutionResults,
    host: ExecutionHost,
    input: Option<Arc<RuntimeInput>>,
    accepted_at: Duration,
    phases: BTreeMap<String, GraphNodePhase>,
    indices: BTreeMap<String, usize>,
    jobs: BTreeMap<DecimalId, String>,
    terminal: bool,
    job_budgets: BTreeMap<String, Option<super::Milliseconds>>,
    run_deadline: Option<Duration>,
}
impl GraphExecution {
    pub(crate) fn new(
        host: ExecutionHost,
        results: ExecutionResults,
        input: Option<Arc<RuntimeInput>>,
        accepted_at: Duration,
        trace: Option<super::trace::TraceTicket>,
    ) -> Result<Self, CoreError> {
        let phases: BTreeMap<_, _> = results
            .required_nodes()
            .map(|node| (node.to_owned(), GraphNodePhase::Pending))
            .collect();
        let indices = results
            .plan
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| phases.contains_key(&node.id))
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        let mut graph = Self {
            trace,
            authority: Arc::new(()),
            results,
            host,
            input,
            accepted_at,
            phases,
            indices,
            jobs: BTreeMap::new(),
            terminal: false,
            job_budgets: BTreeMap::new(),
            run_deadline: None,
        };
        if let Some(t) = &graph.trace {
            for (&i, p) in graph
                .indices
                .values()
                .map(|i| (i, &graph.results.plan.nodes[*i]))
            {
                t.emit(
                    super::TraceKind::DependencyWait,
                    Some((i, &p.node.definition.kind, p.node.definition.version)),
                    None,
                );
            }
        }
        graph.settle()?;
        Ok(graph)
    }
    pub fn key(&self) -> &RunKey {
        self.results.key()
    }
    pub(crate) fn set_job_budgets(&mut self, budgets: &[super::JobBudget]) {
        self.job_budgets = budgets
            .iter()
            .map(|budget| (budget.node_id.clone(), budget.budget_ms))
            .collect();
    }
    pub(crate) fn job_budget(&self, node: &str) -> Option<super::Milliseconds> {
        self.job_budgets.get(node).copied().flatten()
    }
    pub(crate) fn set_run_deadline(&mut self, deadline: Option<Duration>) {
        self.run_deadline = deadline;
    }
    pub(crate) fn run_deadline(&self) -> Option<Duration> {
        self.run_deadline
    }
    pub(crate) fn belongs_to(&self, host: &ExecutionHost) -> bool {
        self.host.same_host(host)
    }
    pub fn phase(&self, node: &str) -> Option<GraphNodePhase> {
        self.phases.get(node).copied()
    }
    pub fn is_complete(&self) -> bool {
        self.results
            .targets
            .iter()
            .all(|target| self.phase(&target.node) == Some(GraphNodePhase::Resolved))
    }
    pub fn ready_nodes(&self) -> Result<Vec<ReadyNode>, CoreError> {
        if self.terminal {
            return Ok(vec![]);
        }
        let mut ready = vec![];
        for (index, node) in self.results.plan.nodes.iter().enumerate() {
            if self.phase(&node.id) != Some(GraphNodePhase::Pending) {
                continue;
            }
            let kind = kind(&node.node.implementation);
            if kind == ExecutionKind::Control || self.inputs(index)?.is_some() {
                ready.push(ReadyNode {
                    node_id: node.id.clone(),
                    execution_kind: kind,
                });
            }
        }
        Ok(ready)
    }
    fn index(&self, node: &str) -> Result<usize, CoreError> {
        self.indices
            .get(node)
            .copied()
            .ok_or_else(|| CoreError::new("unrequested-node", "node is outside target ancestors"))
    }
    fn pending(&self, node: &str) -> Result<usize, CoreError> {
        if self.terminal || self.phase(node) != Some(GraphNodePhase::Pending) {
            return Err(CoreError::new("node-not-ready", "node is not pending"));
        }
        self.index(node)
    }
    fn context_invocation(&self, index: usize) -> OwnedInvocation {
        OwnedInvocation {
            trace: self.trace.clone(),
            key: self.results.key.clone(),
            plan: Arc::clone(&self.results.plan),
            index,
            input: self.input.clone(),
            inputs: BTreeMap::new(),
            input_recoveries: BTreeMap::new(),
            input_errors: None,
        }
    }
    pub fn start_work(&mut self, node: &str) -> Result<OwnedInvocation, CoreError> {
        let invocation = self.preview_work(node)?;
        self.phases.insert(node.into(), GraphNodePhase::Active);
        if let Some(t) = &self.trace {
            let p = &invocation.plan.nodes[invocation.index];
            t.emit(
                super::TraceKind::NodeReady,
                Some((
                    invocation.index,
                    &p.node.definition.kind,
                    p.node.definition.version,
                )),
                None,
            );
        }
        Ok(invocation)
    }
    pub(crate) fn preview_work(&self, node: &str) -> Result<OwnedInvocation, CoreError> {
        let index = self.pending(node)?;
        let planned = &self.results.plan.nodes[index];
        if kind(&planned.node.implementation) == ExecutionKind::Control {
            return Err(CoreError::new(
                "unsupported-capability",
                "control nodes use observer start",
            ));
        }
        let inputs = self.inputs(index)?.ok_or_else(|| {
            CoreError::new(
                "node-not-ready",
                "normal work requires every input to resolve",
            )
        })?;
        if inputs.blocked.is_some() || inputs.missing {
            return Err(CoreError::new(
                "node-not-ready",
                "gated work must settle before start",
            ));
        }
        let mut invocation = self.context_invocation(index);
        invocation.inputs = inputs.inputs;
        invocation.input_recoveries = inputs.recoveries;
        invocation.input_errors = inputs.errors;
        Ok(invocation)
    }
    pub fn bind_job(&mut self, node: &str, job: DecimalId) -> Result<(), CoreError> {
        let index = self.index(node)?;
        if self.terminal
            || self.phase(node) != Some(GraphNodePhase::Active)
            || kind(&self.results.plan.nodes[index].node.implementation) == ExecutionKind::Control
            || self.jobs.contains_key(&job)
            || self.jobs.values().any(|bound| bound == node)
        {
            return Err(CoreError::new(
                "job-conflict",
                "job/node binding must be active and unique",
            ));
        }
        self.jobs.insert(job, node.into());
        Ok(())
    }
    pub fn complete_work(
        &mut self,
        completion: WorkerCompletion,
    ) -> Result<Vec<ReleasedNode>, CoreError> {
        match self.prepare_completion(completion)? {
            Some(prepared) => self.commit_completion(prepared),
            None => Ok(vec![]),
        }
    }
    pub fn prepare_completion(
        &self,
        completion: WorkerCompletion,
    ) -> Result<Option<PreparedGraphCompletion>, CoreError> {
        if completion.stamp.key != self.results.key {
            return Err(CoreError::new(
                "run-key-conflict",
                "completion belongs to another run",
            ));
        }
        let Some(execution) = completion.execution else {
            return Ok(None);
        };
        let node = self
            .jobs
            .get(&completion.stamp.job_id)
            .cloned()
            .ok_or_else(|| {
                CoreError::new("stale-completion", "job has no active graph consumer")
            })?;
        self.results.validate_record(&node, &execution)?;
        Ok(Some(PreparedGraphCompletion {
            authority: Arc::clone(&self.authority),
            stamp: completion.stamp,
            node,
            execution,
        }))
    }
    pub fn commit_completion(
        &mut self,
        prepared: PreparedGraphCompletion,
    ) -> Result<Vec<ReleasedNode>, CoreError> {
        if !Arc::ptr_eq(&prepared.authority, &self.authority) {
            return Err(CoreError::new(
                "run-key-conflict",
                "prepared result belongs to another graph owner",
            ));
        }
        if self.terminal
            || self.jobs.get(&prepared.stamp.job_id) != Some(&prepared.node)
            || self.phase(&prepared.node) != Some(GraphNodePhase::Active)
        {
            return Err(CoreError::new(
                "stale-completion",
                "prepared result no longer has a graph consumer",
            ));
        }
        self.results
            .record_validated(&prepared.node, prepared.execution)?;
        self.resolved(&prepared.node)
    }
    /// A failed handoff / queue or job expiry resolves logical work separately from
    /// physical cancellation. The caller still owns any surviving physical job.
    pub fn fail_work(
        &mut self,
        node: &str,
        error: CoreError,
    ) -> Result<Vec<ReleasedNode>, CoreError> {
        let index = self.index(node)?;
        if self.terminal
            || self.phase(node) != Some(GraphNodePhase::Active)
            || kind(&self.results.plan.nodes[index].node.implementation) == ExecutionKind::Control
        {
            return Err(CoreError::new(
                "node-not-active",
                "normal work is not active",
            ));
        }
        let inputs = self
            .inputs(index)?
            .ok_or_else(|| CoreError::new("node-not-ready", "work inputs are not resolved"))?;
        let invocation = self.context_invocation(index);
        let mut execution = runtime::resolve_outcome(
            &self.results.plan.nodes[index],
            invocation.context(),
            &inputs.inputs,
            &inputs.recoveries,
            inputs.errors.as_ref(),
            Err(error),
        );
        execution.input_errors = inputs.errors;
        self.resolve(node, execution)
    }
    pub fn start_observer(&mut self, node: &str) -> Result<ControlObserver, CoreError> {
        let index = self.pending(node)?;
        if kind(&self.results.plan.nodes[index].node.implementation) != ExecutionKind::Control {
            return Err(CoreError::new(
                "unsupported-capability",
                "node is not a control observer",
            ));
        }
        let lease = self.host.try_reserve(ResourceUsage {
            observers: 1,
            ..Default::default()
        })?;
        self.start_observer_reserved(node, lease)
    }
    pub(crate) fn start_observer_reserved(
        &mut self,
        node: &str,
        lease: super::ResourceLease,
    ) -> Result<ControlObserver, CoreError> {
        self.host.check_lease(
            &lease,
            ResourceUsage {
                observers: 1,
                ..Default::default()
            },
        )?;
        let index = self.pending(node)?;
        if kind(&self.results.plan.nodes[index].node.implementation) != ExecutionKind::Control {
            return Err(CoreError::new(
                "unsupported-capability",
                "node is not a control observer",
            ));
        }
        let observer =
            ControlObserver::new(self.context_invocation(index), self.accepted_at, lease);
        self.phases.insert(node.into(), GraphNodePhase::Active);
        Ok(observer)
    }
    pub(crate) fn fail_bound(
        &mut self,
        stamp: &super::JobStamp,
        error: CoreError,
    ) -> Result<Vec<ReleasedNode>, CoreError> {
        if stamp.key != self.results.key {
            return Err(CoreError::new(
                "run-key-conflict",
                "dispatch failure belongs to another run",
            ));
        }
        let node = self.jobs.get(&stamp.job_id).cloned().ok_or_else(|| {
            CoreError::new("stale-completion", "job has no active graph consumer")
        })?;
        self.fail_work(&node, error)
    }
    pub fn observe(
        &mut self,
        node: &str,
        observer: &mut ControlObserver,
        reason: WakeReason,
        sequence: DecimalId,
        now: Duration,
    ) -> Result<GraphObservation, CoreError> {
        let prepared = self.prepare_observation(node, observer, reason, sequence, now)?;
        self.commit_observation(observer, prepared, now)
    }
    pub fn prepare_observation(
        &self,
        node: &str,
        observer: &mut ControlObserver,
        reason: WakeReason,
        sequence: DecimalId,
        now: Duration,
    ) -> Result<PreparedGraphObservation, CoreError> {
        let index = self.index(node)?;
        if self.terminal
            || self.phase(node) != Some(GraphNodePhase::Active)
            || kind(&self.results.plan.nodes[index].node.implementation) != ExecutionKind::Control
            || observer.key() != self.key()
            || observer.node_id() != node
        {
            return Err(CoreError::new(
                "observer-conflict",
                "observer must match the active node and run",
            ));
        }
        let (inputs, recoveries) = self.control_inputs(index)?;
        let frame = inputs
            .values()
            .map(|input| matches!(input, ControlInput::Resolved(_)))
            .collect();
        observer.set_input_recoveries(recoveries);
        let decision = observer.prepare(inputs, reason, sequence, now)?;
        if let Some(execution) = decision.execution() {
            self.results.validate_record(node, execution)?;
        }
        let validated_finish = decision.execution().is_some();
        Ok(PreparedGraphObservation {
            authority: Arc::clone(&self.authority),
            node: node.into(),
            frame,
            decision,
            validated_finish,
        })
    }
    pub fn commit_observation(
        &mut self,
        observer: &mut ControlObserver,
        prepared: PreparedGraphObservation,
        now: Duration,
    ) -> Result<GraphObservation, CoreError> {
        if !Arc::ptr_eq(&prepared.authority, &self.authority) {
            return Err(CoreError::new(
                "observer-conflict",
                "prepared decision belongs to another graph owner",
            ));
        }
        let node = prepared.node;
        if self.terminal
            || self.phase(&node) != Some(GraphNodePhase::Active)
            || observer.key() != self.key()
            || observer.node_id() != node
        {
            return Err(CoreError::new(
                "observation-stale",
                "prepared decision no longer has a graph consumer",
            ));
        }
        let frame = self.observer_frame(&node)?.expect("active observer");
        if frame != prepared.frame {
            return Err(CoreError::new(
                "observation-stale",
                "inputs changed after preparation",
            ));
        }
        match observer.commit(prepared.decision, now)? {
            ControlObservation::Wait { wake_at } => Ok(GraphObservation::Wait { wake_at }),
            ControlObservation::Finished(execution) => {
                if prepared.validated_finish {
                    self.results.record_validated(&node, *execution)?;
                } else {
                    // Only Wait deadline overflow creates a new Error at commit.
                    self.results.record(&node, *execution)?;
                }
                Ok(GraphObservation::Finished {
                    released: self.resolved(&node)?,
                })
            }
        }
    }
    pub(crate) fn prepared_observation_current(
        &self,
        prepared: &PreparedGraphObservation,
    ) -> Result<bool, CoreError> {
        if self.terminal
            || !Arc::ptr_eq(&prepared.authority, &self.authority)
            || self.phase(&prepared.node) != Some(GraphNodePhase::Active)
        {
            return Ok(false);
        }
        Ok(self
            .observer_frame(&prepared.node)?
            .is_some_and(|frame| frame == prepared.frame))
    }
    fn resolve(
        &mut self,
        node: &str,
        execution: NodeExecution,
    ) -> Result<Vec<ReleasedNode>, CoreError> {
        if self.terminal || self.phase(node) != Some(GraphNodePhase::Active) {
            return Err(CoreError::new(
                "node-not-active",
                "node no longer accepts results",
            ));
        }
        self.results.record(node, execution)?;
        self.resolved(node)
    }
    fn trace_node(&self, node: &str, event: super::TraceKind) {
        if let Some(t) = &self.trace
            && let Some(&i) = self.indices.get(node)
        {
            let p = &self.results.plan.nodes[i];
            t.emit(
                event,
                Some((i, &p.node.definition.kind, p.node.definition.version)),
                None,
            );
        }
    }
    fn trace_resolution(&self, node: &str) {
        if self.trace.is_none() {
            return;
        }
        use super::TraceKind;
        use crate::runtime::{NodeStatus, ResolvedValue};
        if let Some(e) = self.results.execution(node) {
            let event = match e.status {
                NodeStatus::Skipped => TraceKind::NodeSkipped,
                NodeStatus::Failed | NodeStatus::Blocked => TraceKind::NodeFailed,
                _ if e
                    .outputs
                    .values()
                    .any(|v| matches!(v, ResolvedValue::Error(_))) =>
                {
                    TraceKind::NodeFailed
                }
                _ if e
                    .outputs
                    .values()
                    .all(|v| matches!(v, ResolvedValue::NoOutput)) =>
                {
                    TraceKind::NodeNoOutput
                }
                _ => TraceKind::NodeResolved,
            };
            self.trace_node(node, event);
        }
    }
    fn resolved(&mut self, node: &str) -> Result<Vec<ReleasedNode>, CoreError> {
        self.phases.insert(node.into(), GraphNodePhase::Resolved);
        self.trace_resolution(node);
        self.jobs.retain(|_, bound| bound != node);
        self.settle()
    }
    fn input_value(
        &self,
        index: usize,
        port: &str,
    ) -> Result<Option<(ErrorAwareValue, Vec<RecoveryRecord>)>, CoreError> {
        let node = &self.results.plan.nodes[index];
        let Some(source) = node.sources.get(port) else {
            return Ok(Some((ErrorAwareValue::NoOutput, vec![])));
        };
        match self.phase(&source.node) {
            Some(GraphNodePhase::Resolved) => {
                let execution = self.results.execution(&source.node).ok_or_else(|| {
                    CoreError::new("graph-state", "resolved node has no execution")
                })?;
                Ok(Some(match &execution.outputs[&source.port] {
                    ResolvedValue::Data(packet) => (
                        ErrorAwareValue::Data(Arc::clone(packet)),
                        execution
                            .output_recoveries
                            .get(&source.port)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                    ResolvedValue::NoOutput => (ErrorAwareValue::NoOutput, vec![]),
                    ResolvedValue::Error(_) => (
                        ErrorAwareValue::Error(
                            execution.error_outputs[&source.port]
                                .routed(source.clone(), Endpoint::new(&node.id, port)),
                        ),
                        vec![],
                    ),
                }))
            }
            Some(GraphNodePhase::Pending | GraphNodePhase::Active) => Ok(None),
            _ => Err(CoreError::new(
                "graph-state",
                "a live consumer references discarded work",
            )),
        }
    }
    fn inputs(&self, index: usize) -> Result<Option<ResolvedInputs>, CoreError> {
        let node = &self.results.plan.nodes[index];
        let mut resolved = ResolvedInputs {
            inputs: BTreeMap::new(),
            recoveries: BTreeMap::new(),
            errors: None,
            blocked: None,
            missing: false,
        };
        for port in &node.node.definition.inputs {
            let Some((value, recoveries)) = self.input_value(index, &port.port.id)? else {
                return Ok(None);
            };
            if let ErrorAwareValue::Error(errors) = &value {
                merge_errors(&mut resolved.errors, errors);
                if !node.node.implementation.receives_error(&port.port.id) {
                    resolved
                        .blocked
                        .get_or_insert_with(|| errors.causes()[0].diagnostic.clone());
                }
            }
            resolved.missing |= port.required && matches!(value, ErrorAwareValue::NoOutput);
            resolved.recoveries.insert(port.port.id.clone(), recoveries);
            resolved.inputs.insert(port.port.id.clone(), value);
        }
        Ok(Some(resolved))
    }
    fn control_inputs(&self, index: usize) -> Result<ControlInputs, CoreError> {
        let mut inputs = BTreeMap::new();
        let mut recoveries = BTreeMap::new();
        for port in &self.results.plan.nodes[index].node.definition.inputs {
            let value = match self.input_value(index, &port.port.id)? {
                Some((value, records)) => {
                    recoveries.insert(port.port.id.clone(), records);
                    ControlInput::Resolved(value)
                }
                None => ControlInput::Pending,
            };
            inputs.insert(port.port.id.clone(), value);
        }
        Ok((inputs, recoveries))
    }
    pub(crate) fn observer_nodes(&self) -> Vec<String> {
        self.results
            .plan
            .nodes
            .iter()
            .filter(|node| {
                self.phase(&node.id) == Some(GraphNodePhase::Active)
                    && kind(&node.node.implementation) == ExecutionKind::Control
            })
            .map(|node| node.id.clone())
            .collect()
    }
    pub(crate) fn observer_frame(&self, node: &str) -> Result<Option<Vec<bool>>, CoreError> {
        if self.phase(node) != Some(GraphNodePhase::Active) {
            return Ok(None);
        }
        let planned = &self.results.plan.nodes[self.index(node)?];
        if kind(&planned.node.implementation) != ExecutionKind::Control {
            return Ok(None);
        }
        let mut frame = BTreeMap::new();
        for port in &planned.node.definition.inputs {
            let resolved = match planned.sources.get(&port.port.id) {
                None => true,
                Some(source) => match self.phase(&source.node) {
                    Some(GraphNodePhase::Resolved) => true,
                    Some(GraphNodePhase::Pending | GraphNodePhase::Active) => false,
                    _ => {
                        return Err(CoreError::new(
                            "graph-state",
                            "live observer references discarded work",
                        ));
                    }
                },
            };
            frame.insert(port.port.id.as_str(), resolved);
        }
        Ok(Some(frame.into_values().collect()))
    }
    fn demanded(&self) -> BTreeSet<String> {
        let mut stack: Vec<_> = self
            .results
            .targets
            .iter()
            .map(|target| target.node.clone())
            .collect();
        let mut needed = BTreeSet::new();
        while let Some(node) = stack.pop() {
            if needed.insert(node.clone()) && self.phase(&node) != Some(GraphNodePhase::Resolved) {
                let index = self.index(&node).expect("validated target ancestry");
                stack.extend(
                    self.results.plan.nodes[index]
                        .sources
                        .values()
                        .map(|source| source.node.clone()),
                );
            }
        }
        needed
    }
    fn release(&mut self, node: &str) -> ReleasedNode {
        let job_id = self
            .jobs
            .iter()
            .find_map(|(job, bound)| (bound == node).then_some(*job));
        self.jobs.retain(|_, bound| bound != node);
        ReleasedNode {
            node_id: node.into(),
            execution_kind: kind(
                &self.results.plan.nodes[self.index(node).expect("known node")]
                    .node
                    .implementation,
            ),
            job_id,
        }
    }
    fn settle(&mut self) -> Result<Vec<ReleasedNode>, CoreError> {
        let mut released = vec![];
        loop {
            let demanded = self.demanded();
            let order: Vec<_> = self.results.required_nodes().map(str::to_owned).collect();
            let mut changed = false;
            for node in &order {
                if !demanded.contains(node)
                    && matches!(
                        self.phase(node),
                        Some(GraphNodePhase::Pending | GraphNodePhase::Active)
                    )
                {
                    if self.phase(node) == Some(GraphNodePhase::Active) {
                        released.push(self.release(node));
                    }
                    self.results.discard_unneeded(node)?;
                    self.phases.insert(node.clone(), GraphNodePhase::Discarded);
                    self.trace_node(node, super::TraceKind::NodeDiscarded);
                    changed = true;
                }
            }
            for node in &order {
                if self.phase(node) != Some(GraphNodePhase::Pending) {
                    continue;
                }
                let index = self.index(node)?;
                let planned = &self.results.plan.nodes[index];
                if kind(&planned.node.implementation) == ExecutionKind::Control {
                    continue;
                }
                let Some(inputs) = self.inputs(index)? else {
                    continue;
                };
                let mut execution = if let Some(diagnostic) = inputs.blocked {
                    runtime::failed(
                        planned,
                        NodeStatus::Blocked,
                        diagnostic,
                        inputs.errors.clone(),
                    )
                } else if inputs.missing {
                    runtime::empty_execution(planned)
                } else {
                    continue;
                };
                execution.input_errors = inputs.errors;
                self.results.record(node, execution)?;
                self.phases.insert(node.clone(), GraphNodePhase::Resolved);
                self.trace_resolution(node);
                changed = true;
            }
            if !changed {
                return Ok(released);
            }
        }
    }
    fn close(&mut self, snapshot: RunSnapshot) -> GraphCompletion {
        GraphCompletion {
            snapshot,
            released: self.close_work(),
        }
    }
    pub(crate) fn close_work(&mut self) -> Vec<ReleasedNode> {
        self.terminal = true;
        let active: Vec<_> = self
            .phases
            .iter()
            .filter_map(|(node, phase)| (*phase == GraphNodePhase::Active).then_some(node.clone()))
            .collect();
        let released = active.iter().map(|node| self.release(node)).collect();
        for phase in self.phases.values_mut() {
            if matches!(*phase, GraphNodePhase::Pending | GraphNodePhase::Active) {
                *phase = GraphNodePhase::Discarded;
            }
        }
        released
    }
    pub fn finish(
        &mut self,
        ledger: &mut RunLedger,
        reason: TerminalReason,
    ) -> Result<GraphCompletion, CoreError> {
        let snapshot = ledger.finish_collected(&mut self.results, reason)?;
        Ok(self.close(snapshot))
    }
}
type ControlInputs = (
    BTreeMap<String, ControlInput>,
    BTreeMap<String, Vec<RecoveryRecord>>,
);
fn kind(implementation: &RegisteredImplementation) -> ExecutionKind {
    match implementation {
        RegisteredImplementation::Legacy(_) | RegisteredImplementation::ErrorAware { .. } => {
            ExecutionKind::Sync
        }
        RegisteredImplementation::Deferred { .. } => ExecutionKind::Deferred,
        RegisteredImplementation::Control { .. } => ExecutionKind::Control,
    }
}
