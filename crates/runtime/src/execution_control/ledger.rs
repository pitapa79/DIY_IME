//! Host-side admission/state component. Scheduling and worker execution are separate.
use super::protocol::MAX_REQUEST_BYTES;
use super::report::{
    PhysicalCounts, RunPhase, RunSnapshot, StartReceipt, TerminalReason, TerminalResult,
    retained_bytes,
};
use super::types::MAX_SAFE_INTEGER;
use super::{
    DecimalId, ExecutionHost, JobBudget, Milliseconds, ResourceLease, ResourceUsage, RunBudgets,
    RunKey, SessionEpoch,
};
use crate::{
    graph::{Endpoint, ExecutionPlan},
    node::{CoreError, RuntimeInput},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    pub start_key: DecimalId,
    pub expected_plan_version: DecimalId,
    pub targets: Vec<Endpoint>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::json::optional_non_null"
    )]
    pub input: Option<RuntimeInput>,
    #[serde(default)]
    pub run_budget_ms: Option<Milliseconds>,
    #[serde(default)]
    pub job_budgets: Vec<JobBudget>,
}
struct ActiveRun {
    trace: Option<super::trace::TraceTicket>,
    key: RunKey,
    start_key: DecimalId,
    payload: Value,
    plan: Arc<ExecutionPlan>,
    targets: Vec<Endpoint>,
    accepted_at: Duration,
    deadline: Option<Duration>,
    phase: RunPhase,
    physical: PhysicalCounts,
    lease: ResourceLease,
}
#[derive(Debug)]
struct CachedRun {
    trace: Option<super::trace::TraceTicket>,
    key: RunKey,
    start_key: DecimalId,
    payload: Value,
    ended_at: Duration,
    elapsed_ms: u64,
    result: TerminalResult,
    physical: PhysicalCounts,
    lease: ResourceLease,
}
#[derive(Debug, Default)]
pub(crate) struct RetentionStore {
    now: Duration,
    records: BTreeMap<DecimalId, CachedRun>,
    tombstones: VecDeque<(RunKey, ResourceLease)>,
}
impl RetentionStore {
    pub(crate) fn now(&self) -> Duration {
        self.now
    }
    pub(crate) fn update_closed_physical(&mut self, key: &RunKey, physical: PhysicalCounts) {
        if let Some(record) = self
            .records
            .get_mut(&key.run_id)
            .filter(|record| &record.key == key)
        {
            let was_ending = record.physical.running
                + record.physical.cancel_requested
                + record.physical.stalled
                + record.physical.queued
                != 0;
            record.physical = physical;
            if was_ending
                && physical.running == 0
                && physical.cancel_requested == 0
                && physical.stalled == 0
                && physical.queued == 0
                && let Some(t) = &record.trace
            {
                t.emit(super::TraceKind::PhysicalEnded, None, None);
            }
        }
    }
    pub(crate) fn next_deadline(&self, host: &ExecutionHost) -> Option<Duration> {
        self.records
            .values()
            .filter_map(|record| {
                record
                    .ended_at
                    .checked_add(host.policy().retention_ms.duration())
            })
            .min()
    }
    pub(crate) fn advance(&mut self, host: &ExecutionHost, now: Duration) {
        self.now = self.now.max(now);
        self.expire(host);
    }
    fn tombstone(&mut self, host: &ExecutionHost, key: RunKey) {
        if host.policy().max_tombstones == 0 {
            return;
        }
        let usage = ResourceUsage {
            tombstones: 1,
            ..Default::default()
        };
        let lease = match host.try_reserve(usage) {
            Ok(lease) => Some(lease),
            Err(_) => {
                if let Some((_, mut oldest)) = self.tombstones.pop_front() {
                    host.try_reserve_replacing(&mut [&mut oldest], usage).ok()
                } else {
                    None
                }
            }
        };
        if let Some(lease) = lease {
            self.tombstones.push_back((key, lease));
        }
    }
    fn remove(&mut self, host: &ExecutionHost, id: DecimalId) {
        if let Some(record) = self.records.remove(&id) {
            let key = record.key.clone();
            drop(record);
            self.tombstone(host, key);
        }
    }
    fn expire(&mut self, host: &ExecutionHost) {
        let ids: Vec<_> = self
            .records
            .iter()
            .filter(|(_, r)| {
                self.now.saturating_sub(r.ended_at) >= host.policy().retention_ms.duration()
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.remove(host, id);
        }
    }
    fn reserve(
        &mut self,
        host: &ExecutionHost,
        demand: ResourceUsage,
        active: Option<&mut ResourceLease>,
    ) -> Result<ResourceLease, CoreError> {
        self.reserve_checked(host, demand, active, || Ok(()))
            .map(|(lease, ())| lease)
    }
    fn reserve_checked<T>(
        &mut self,
        host: &ExecutionHost,
        demand: ResourceUsage,
        mut active: Option<&mut ResourceLease>,
        check: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<(ResourceLease, T), CoreError> {
        let mut check = Some(check);
        let mut order: Vec<_> = self
            .records
            .iter()
            .map(|(id, r)| (r.ended_at, *id))
            .collect();
        order.sort();
        let mut selected = std::collections::BTreeSet::new();
        loop {
            let mut releases: Vec<_> = self
                .records
                .iter_mut()
                .filter(|(id, _)| selected.contains(*id))
                .map(|(_, r)| &mut r.lease)
                .collect();
            if let Some(lease) = active.as_deref_mut() {
                releases.push(lease);
            }
            let result = host.try_reserve_replacing_checked(&mut releases, demand, || {
                check.take().expect("checked only on a fitting exchange")()
            });
            drop(releases);
            if let Ok(lease) = result {
                for id in selected {
                    self.remove(host, id);
                }
                return Ok(lease);
            }
            if check.is_none() || result.as_ref().is_err_and(|error| error.code != "busy") {
                return result;
            }
            if selected.len() == order.len() {
                return result;
            }
            selected.insert(order[selected.len()].1);
        }
    }
}

pub struct RunLedger {
    trace: Arc<super::trace::TraceSession>,
    host: ExecutionHost,
    epoch: SessionEpoch,
    plan: Arc<ExecutionPlan>,
    previous: Option<Arc<ExecutionPlan>>,
    pending: Option<Arc<ExecutionPlan>>,
    version: DecimalId,
    revision: u64,
    now: Duration,
    seq: u64,
    generation: DecimalId,
    max_start: Option<DecimalId>,
    latest_revision: Option<u64>,
    bound_input: Option<RuntimeInput>,
    active: BTreeMap<DecimalId, ActiveRun>,
}
impl RunLedger {
    pub fn new(
        host: ExecutionHost,
        epoch: SessionEpoch,
        plan: ExecutionPlan,
        version: DecimalId,
    ) -> Self {
        Self::with_shared_plan(host, epoch, Arc::new(plan), version, 0)
    }
    pub(crate) fn with_shared_plan(
        host: ExecutionHost,
        epoch: SessionEpoch,
        plan: Arc<ExecutionPlan>,
        version: DecimalId,
        revision: u64,
    ) -> Self {
        let now = host.retention.lock().unwrap_or_else(|p| p.into_inner()).now;
        let trace = host.diagnostics.session(&epoch);
        Self {
            trace,
            host,
            epoch,
            plan,
            previous: None,
            pending: None,
            version,
            revision,
            now,
            seq: 0,
            generation: DecimalId::new(1).expect("positive"),
            max_start: None,
            latest_revision: None,
            bound_input: None,
            active: BTreeMap::new(),
        }
    }
    /// Metadata only. Existing runs are not retroactively instrumented.
    pub fn start_trace(&self) -> Result<(), CoreError> {
        self.trace.start()
    }
    pub fn stop_trace(&self) {
        self.trace.stop();
    }
    pub fn clear_trace(&self) {
        self.trace.clear();
    }
    pub fn read_trace(&self, after: u64, limit: usize) -> Result<super::TraceSnapshot, CoreError> {
        self.trace.read(after, limit)
    }
    pub(crate) fn trace_physical_ended(&self, key: &RunKey) {
        self.trace.physical_ended(key);
    }
    pub(crate) fn trace_ticket(&self, key: &RunKey) -> Option<super::trace::TraceTicket> {
        self.active
            .get(&key.run_id)
            .filter(|r| &r.key == key)
            .and_then(|r| r.trace.clone())
    }
    pub fn host(&self) -> &ExecutionHost {
        &self.host
    }
    pub fn plan_version(&self) -> DecimalId {
        self.version
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn control_state(&self) -> super::SessionControlState {
        super::SessionControlState {
            revision: self.revision,
            active_plan_version: Some(self.version),
            has_prepared: self.pending.is_some(),
            can_rollback: self.previous.is_some(),
            latest_input_revision: self.latest_revision,
            delivery_generation: self.generation,
        }
    }
    pub fn now(&self) -> Duration {
        self.now
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn latest_input_revision(&self) -> Option<u64> {
        self.latest_revision
    }
    pub fn delivery_generation(&self) -> DecimalId {
        self.generation
    }
    /// An invocation owns this Arc even if its logical run or plan is later replaced.
    pub fn plan_snapshot(&self, key: &RunKey) -> Result<Arc<ExecutionPlan>, CoreError> {
        self.check_key(key)?;
        let record = self
            .active
            .get(&key.run_id)
            .ok_or_else(|| CoreError::new("run-unknown", "run is not active"))?;
        if &record.key != key {
            return Err(CoreError::new("run-key-conflict", "run key does not match"));
        }
        Ok(Arc::clone(&record.plan))
    }
    /// Low-level host boundary for a fully resolved invocation. The graph scheduler
    /// remains responsible for demand, dependency order and executing a node once.
    /// This method never starts a job and is not a session request.
    pub fn owned_invocation(
        &self,
        key: &RunKey,
        node_id: &str,
        inputs: crate::error_input::ErrorAwareInputs,
    ) -> Result<super::OwnedInvocation, CoreError> {
        use crate::error_input::{ErrorAwareValue, merge_errors};
        let plan = self.plan_snapshot(key)?;
        let index = plan
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .ok_or_else(|| CoreError::new("unknown-node", node_id))?;
        let node = &plan.nodes[index];
        if inputs.len() != node.node.definition.inputs.len() {
            return Err(CoreError::new(
                "invalid-invocation",
                "all declared inputs must be resolved",
            ));
        }
        let mut input_errors = None;
        for port in &node.node.definition.inputs {
            match inputs.get(&port.port.id) {
                Some(ErrorAwareValue::Data(packet)) => {
                    packet
                        .validate()
                        .map_err(|error| CoreError::new("invalid-packet", error.to_string()))?;
                    if !port.port.root.accepts(&packet.root) {
                        return Err(CoreError::new(
                            "invalid-root",
                            "input root violates port constraint",
                        ));
                    }
                }
                Some(ErrorAwareValue::NoOutput) if !port.required => {}
                Some(ErrorAwareValue::Error(errors))
                    if node.node.implementation.receives_error(&port.port.id) =>
                {
                    if errors.causes().iter().any(|cause| {
                        cause.id.run_id != key.run_id.get()
                            || cause.id.plan_version != key.plan_version.get()
                    }) {
                        return Err(CoreError::new(
                            "invalid-invocation",
                            "input errors belong to another run",
                        ));
                    }
                    merge_errors(&mut input_errors, errors);
                }
                _ => {
                    return Err(CoreError::new(
                        "invalid-invocation",
                        "missing, required NoOutput or blocked error input",
                    ));
                }
            }
        }
        let record = &self.active[&key.run_id];
        let input = record
            .payload
            .get("input")
            .cloned()
            .map(serde_json::from_value::<RuntimeInput>)
            .transpose()
            .map_err(|error| CoreError::new("invalid-runtime-input", error.to_string()))?
            .map(Arc::new);
        Ok(super::OwnedInvocation {
            trace: self.trace_ticket(key),
            key: key.clone(),
            plan,
            index,
            input,
            inputs,
            input_recoveries: BTreeMap::new(),
            input_errors,
        })
    }
    /// Reserve one control observer without converting Pending into normal input.
    pub fn control_observer(
        &self,
        key: &RunKey,
        node_id: &str,
    ) -> Result<super::ControlObserver, CoreError> {
        let plan = self.plan_snapshot(key)?;
        let index = plan
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .ok_or_else(|| CoreError::new("unknown-node", node_id))?;
        if !matches!(
            plan.nodes[index].node.implementation,
            crate::node::RegisteredImplementation::Control { .. }
        ) {
            return Err(CoreError::new(
                "unsupported-capability",
                "node is not a control observer",
            ));
        }
        let record = &self.active[&key.run_id];
        let input = record
            .payload
            .get("input")
            .cloned()
            .map(serde_json::from_value::<RuntimeInput>)
            .transpose()
            .map_err(|error| CoreError::new("invalid-runtime-input", error.to_string()))?
            .map(Arc::new);
        let lease = self.host.try_reserve(ResourceUsage {
            observers: 1,
            ..Default::default()
        })?;
        let invocation = super::OwnedInvocation {
            trace: self.trace_ticket(key),
            key: key.clone(),
            plan,
            index,
            input,
            inputs: BTreeMap::new(),
            input_recoveries: BTreeMap::new(),
            input_errors: None,
        };
        Ok(super::ControlObserver::new(
            invocation,
            record.accepted_at,
            lease,
        ))
    }
    fn bump(&mut self) -> Result<(), CoreError> {
        self.seq = self
            .seq
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        Ok(())
    }
    fn check_key(&self, key: &RunKey) -> Result<(), CoreError> {
        key.validate()?;
        if key.session_epoch != self.epoch {
            Err(CoreError::new(
                "session-conflict",
                "run belongs to another session",
            ))
        } else {
            Ok(())
        }
    }
    fn check_version(&self, version: DecimalId) -> Result<(), CoreError> {
        if version != self.version {
            Err(CoreError::new("plan-conflict", "active plan changed"))
        } else {
            Ok(())
        }
    }
    fn current(&self, key: &RunKey) -> bool {
        key.plan_version == self.version
            && key
                .input_revision
                .is_none_or(|revision| Some(revision) == self.latest_revision)
    }
    fn elapsed(&self, start: Duration) -> u64 {
        self.now
            .saturating_sub(start)
            .as_millis()
            .min(u128::from(MAX_SAFE_INTEGER)) as u64
    }
    pub fn get_start(&self, start_key: DecimalId) -> Result<RunKey, CoreError> {
        if let Some(record) = self.active.values().find(|r| r.start_key == start_key) {
            return Ok(record.key.clone());
        }
        let cache = self
            .host
            .retention
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(record) = cache
            .records
            .values()
            .find(|r| r.key.session_epoch == self.epoch && r.start_key == start_key)
        {
            return Ok(record.key.clone());
        }
        Err(CoreError::new(
            if self.max_start.is_some_and(|max| start_key <= max) {
                "start-key-retired"
            } else {
                "start-not-seen"
            },
            "start receipt is unavailable; no work was replayed",
        ))
    }
    pub fn get_run(&self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.check_key(key)?;
        if let Some(r) = self.active.get(&key.run_id) {
            if &r.key != key {
                return Err(CoreError::new("run-key-conflict", "run key does not match"));
            }
            return Ok(RunSnapshot {
                key: key.clone(),
                state: r.phase,
                event_seq: DecimalId::new(self.seq)?,
                elapsed_ms: self.elapsed(r.accepted_at),
                current: self.current(key),
                physical: r.physical,
                poll_after_ms: 1,
                result: None,
            });
        }
        let cache = self
            .host
            .retention
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(r) = cache
            .records
            .get(&key.run_id)
            .filter(|r| r.key.session_epoch == self.epoch)
        {
            if &r.key != key {
                return Err(CoreError::new("run-key-conflict", "run key does not match"));
            }
            return Ok(RunSnapshot {
                key: key.clone(),
                state: RunPhase::Terminal,
                event_seq: DecimalId::new(self.seq.max(1))?,
                elapsed_ms: r.elapsed_ms,
                current: self.current(key),
                physical: r.physical,
                poll_after_ms: 0,
                result: Some(r.result.clone()),
            });
        }
        let gone = cache.tombstones.iter().any(|(old, _)| old == key);
        Err(CoreError::new(
            if gone { "result-gone" } else { "run-unknown" },
            "run result is unavailable; this does not mean it never executed",
        ))
    }
    pub fn start_run(&mut self, request: StartRequest) -> Result<StartReceipt, CoreError> {
        self.admit(request, ResourceUsage::default())
            .map(|(receipt, _)| receipt)
    }
    pub(crate) fn admit(
        &mut self,
        request: StartRequest,
        work: ResourceUsage,
    ) -> Result<(StartReceipt, Option<ResourceLease>), CoreError> {
        self.admit_with_results(request, work, &mut [])
    }
    pub(crate) fn admit_with_results(
        &mut self,
        request: StartRequest,
        work: ResourceUsage,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<(StartReceipt, Option<ResourceLease>), CoreError> {
        self.admit_clocked(request, work, results, None, None)
    }
    pub(crate) fn admit_clocked(
        &mut self,
        request: StartRequest,
        work: ResourceUsage,
        results: &mut [&mut super::ExecutionResults],
        clock: Option<&super::clock::AdmissionClock>,
        queue_wait: Option<Milliseconds>,
    ) -> Result<(StartReceipt, Option<ResourceLease>), CoreError> {
        let payload = serde_json::to_value(&request)
            .map_err(|e| CoreError::new("invalid-request", e.to_string()))?;
        if payload.to_string().len() > MAX_REQUEST_BYTES {
            return Err(CoreError::new(
                "request-limit",
                "start payload exceeds 1 MiB",
            ));
        }
        let previous = self
            .active
            .values()
            .find(|r| r.start_key == request.start_key)
            .map(|r| (r.key.clone(), &r.payload));
        if let Some((key, old)) = previous {
            return if old == &payload {
                Ok((
                    StartReceipt {
                        state: RunPhase::Accepted,
                        replayed: true,
                        key,
                    },
                    None,
                ))
            } else {
                Err(CoreError::new(
                    "start-conflict",
                    "start key was accepted with another payload",
                ))
            };
        }
        {
            let cache = self
                .host
                .retention
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(record) = cache
                .records
                .values()
                .find(|r| r.key.session_epoch == self.epoch && r.start_key == request.start_key)
            {
                return if record.payload == payload {
                    Ok((
                        StartReceipt {
                            state: RunPhase::Accepted,
                            replayed: true,
                            key: record.key.clone(),
                        },
                        None,
                    ))
                } else {
                    Err(CoreError::new(
                        "start-conflict",
                        "start key was accepted with another payload",
                    ))
                };
            }
        }
        if self.max_start.is_some_and(|max| request.start_key <= max) {
            return Err(CoreError::new(
                "start-key-retired",
                "retired start keys cannot execute again",
            ));
        }
        self.check_version(request.expected_plan_version)?;
        self.plan.validate_targets(&request.targets)?;
        RunBudgets {
            run_budget_ms: request.run_budget_ms,
            job_budgets: request.job_budgets.clone(),
        }
        .validate()?;
        for budget in &request.job_budgets {
            if !self.plan.nodes.iter().any(|node| {
                node.id == budget.node_id
                    && !matches!(
                        node.node.implementation,
                        crate::node::RegisteredImplementation::Control { .. }
                    )
            }) {
                return Err(CoreError::new(
                    "invalid-deadline",
                    "job budget node must exist and be sync/deferred",
                ));
            }
        }
        if let Some(input) = &request.input {
            input.validate()?;
            if self
                .latest_revision
                .is_some_and(|revision| input.revision < revision)
            {
                return Err(CoreError::new("stale-input", "input revision is stale"));
            }
            if self.latest_revision == Some(input.revision)
                && self.bound_input.as_ref().is_some_and(|old| old != input)
            {
                return Err(CoreError::new(
                    "input-conflict",
                    "revision is bound to another snapshot",
                ));
            }
        }
        let deadline = request
            .run_budget_ms
            .map(|budget| budget.checked_deadline(self.now))
            .transpose()?;
        let generation = self.generation.checked_next()?;
        self.seq
            .checked_add(1 + self.active.len() as u64)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        let worst = RunKey {
            session_epoch: self.epoch.clone(),
            plan_version: self.version,
            run_id: DecimalId::new(u64::MAX)?,
            input_revision: request.input.as_ref().map(|input| input.revision),
            delivery_generation: DecimalId::new(u64::MAX)?,
        };
        for result in [
            TerminalResult::limited(&request.targets),
            TerminalResult::aborted(&request.targets, TerminalReason::Cancelled),
            TerminalResult::aborted(&request.targets, TerminalReason::Expired),
        ] {
            if retained_bytes(&worst, MAX_SAFE_INTEGER, &result)?
                > self.host.policy().max_result_bytes_per_run
            {
                return Err(CoreError::new(
                    "invalid-result-budget",
                    "targets and identifiers do not fit the reserved terminal budget",
                ));
            }
        }
        let base = ResourceUsage {
            active_runs: 1,
            reserved_result_bytes: self.host.policy().max_result_bytes_per_run,
            dedupe_entries: 1,
            ..Default::default()
        };
        if work.active_runs != 0
            || work.retained_runs != 0
            || work.retained_bytes != 0
            || work.reserved_result_bytes != 0
            || work.dedupe_entries != 0
            || work.tombstones != 0
        {
            return Err(CoreError::new(
                "invalid-reservation",
                "initial work may only reserve work resources",
            ));
        }
        let demand = base
            .add(work)
            .ok_or_else(|| CoreError::new("busy", "resource demand overflow"))?;
        // An unused internal ID may leave a gap; no receipt/state is published on failure.
        // Check exhaustion before cache eviction so a failed ID allocation cannot evict results.
        let run_id = DecimalId::new(crate::runtime::allocate_run_id()?)?;
        let (mut lease, (accepted_at, deadline)) = {
            let host = self.host.clone();
            let mut cache = host.retention.lock().unwrap_or_else(|p| p.into_inner());
            cache.reserve_checked(&host, demand, None, || {
                if let Some(clock) = clock {
                    let at = clock.sample()?;
                    let deadline = request
                        .run_budget_ms
                        .map(|budget| budget.checked_deadline(at))
                        .transpose()?;
                    queue_wait
                        .map(|budget| budget.checked_deadline(at))
                        .transpose()?;
                    Ok((at, deadline))
                } else {
                    Ok((self.now, deadline))
                }
            })?
        };
        self.now = accepted_at;
        let work_lease = lease.split(work)?;
        let key = RunKey {
            session_epoch: self.epoch.clone(),
            plan_version: self.version,
            run_id,
            input_revision: request.input.as_ref().map(|input| input.revision),
            delivery_generation: generation,
        };
        let targets = request.targets;
        let new_input = request.input;
        self.max_start = Some(request.start_key);
        self.generation = generation;
        self.bump()?;
        self.active.insert(
            run_id,
            ActiveRun {
                trace: self.trace.accept(&key),
                key: key.clone(),
                start_key: request.start_key,
                payload,
                plan: Arc::clone(&self.plan),
                targets,
                accepted_at: self.now,
                deadline,
                phase: RunPhase::Accepted,
                physical: PhysicalCounts::default(),
                lease,
            },
        );
        if let Some(input) = new_input {
            let changed = self.latest_revision != Some(input.revision);
            self.latest_revision = Some(input.revision);
            self.bound_input = Some(input);
            if changed {
                let ids: Vec<_> = self
                    .active
                    .values()
                    .filter(|r| {
                        r.key
                            .input_revision
                            .is_some_and(|revision| Some(revision) != self.latest_revision)
                    })
                    .map(|r| r.key.clone())
                    .collect();
                for old in ids {
                    self.cancel_collected(&old, results)?;
                }
            }
        }
        Ok((
            StartReceipt {
                state: RunPhase::Accepted,
                replayed: false,
                key,
            },
            Some(work_lease),
        ))
    }
    pub(crate) fn admission_plan(
        &self,
        request: &StartRequest,
    ) -> Result<Arc<ExecutionPlan>, CoreError> {
        self.check_version(request.expected_plan_version)?;
        self.plan.validate_targets(&request.targets)?;
        if let Some(input) = &request.input {
            input.validate()?;
        }
        Ok(Arc::clone(&self.plan))
    }
    pub(crate) fn active_keys(&self) -> Vec<RunKey> {
        self.active
            .values()
            .map(|record| record.key.clone())
            .collect()
    }
    pub(crate) fn active_key(&self, key: &RunKey) -> bool {
        self.active
            .get(&key.run_id)
            .is_some_and(|record| &record.key == key)
    }
    pub(crate) fn next_run_deadline(&self) -> Option<Duration> {
        self.active.values().filter_map(|run| run.deadline).min()
    }
    pub(crate) fn run_deadline(&self, key: &RunKey) -> Option<Duration> {
        self.active
            .get(&key.run_id)
            .filter(|run| &run.key == key)
            .and_then(|run| run.deadline)
    }
    pub(crate) fn accepted_at(&self, key: &RunKey) -> Option<Duration> {
        self.active
            .get(&key.run_id)
            .filter(|run| &run.key == key)
            .map(|run| run.accepted_at)
    }
    pub(crate) fn next_event_seq(&mut self) -> Result<DecimalId, CoreError> {
        self.bump()?;
        DecimalId::new(self.seq)
    }
    pub fn mark_running(&mut self, key: &RunKey) -> Result<(), CoreError> {
        self.seq
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        self.check_key(key)?;
        let r = self
            .active
            .get_mut(&key.run_id)
            .ok_or_else(|| CoreError::new("run-unknown", "run is not active"))?;
        if &r.key != key {
            return Err(CoreError::new("run-key-conflict", "run key does not match"));
        }
        r.phase = RunPhase::Running;
        self.bump()
    }
    pub fn update_physical(
        &mut self,
        key: &RunKey,
        counts: PhysicalCounts,
    ) -> Result<bool, CoreError> {
        self.seq
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        self.check_key(key)?;
        if counts
            .running
            .checked_add(counts.cancel_requested)
            .and_then(|n| n.checked_add(counts.stalled))
            .is_none_or(|n| n > self.host.policy().max_physical_jobs)
            || counts.queued > self.host.policy().max_queued_jobs
        {
            return Err(CoreError::new(
                "invalid-result",
                "physical counts exceed host policy",
            ));
        }
        if let Some(r) = self.active.get_mut(&key.run_id) {
            if &r.key != key {
                return Err(CoreError::new("run-key-conflict", "run key does not match"));
            }
            r.physical = counts;
            self.bump()?;
            return Ok(true);
        }
        let changed = {
            let mut cache = self
                .host
                .retention
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(r) = cache
                .records
                .get_mut(&key.run_id)
                .filter(|r| r.key.session_epoch == self.epoch)
            {
                if &r.key != key {
                    return Err(CoreError::new("run-key-conflict", "run key does not match"));
                }
                let pending = r.physical.running
                    + r.physical.cancel_requested
                    + r.physical.stalled
                    + r.physical.queued
                    != 0;
                r.physical = counts;
                if pending
                    && counts.running == 0
                    && counts.cancel_requested == 0
                    && counts.stalled == 0
                    && counts.queued == 0
                    && let Some(t) = &r.trace
                {
                    t.emit(super::TraceKind::PhysicalEnded, None, None);
                }
                true
            } else {
                false
            }
        };
        if changed {
            self.bump()?;
        }
        Ok(changed)
    }
    pub fn finish_run(&mut self, key: &RunKey, result: TerminalResult) -> Result<bool, CoreError> {
        self.check_key(key)?;
        let Some(record) = self.active.get(&key.run_id) else {
            return Ok(false);
        };
        if &record.key != key {
            return Err(CoreError::new("run-key-conflict", "run key does not match"));
        }
        result.validate(&record.plan, &record.targets)?;
        self.end_run(key, result)?;
        Ok(true)
    }
    pub fn result_collector(&self, key: &RunKey) -> Result<super::ExecutionResults, CoreError> {
        let plan = self.plan_snapshot(key)?;
        Ok(super::ExecutionResults::new(
            key.clone(),
            plan,
            self.active[&key.run_id].targets.clone(),
        ))
    }
    pub fn graph_execution(&self, key: &RunKey) -> Result<super::GraphExecution, CoreError> {
        let results = self.result_collector(key)?;
        let input = self.active[&key.run_id]
            .payload
            .get("input")
            .cloned()
            .map(serde_json::from_value::<RuntimeInput>)
            .transpose()
            .map_err(|error| CoreError::new("invalid-runtime-input", error.to_string()))?
            .map(Arc::new);
        let mut graph = super::GraphExecution::new(
            self.host.clone(),
            results,
            input,
            self.active[&key.run_id].accepted_at,
            self.trace_ticket(key),
        )?;
        let budgets: Vec<JobBudget> =
            serde_json::from_value(self.active[&key.run_id].payload["jobBudgets"].clone())
                .expect("validated stored job budgets");
        graph.set_job_budgets(&budgets);
        graph.set_run_deadline(self.active[&key.run_id].deadline);
        Ok(graph)
    }
    fn check_collector(&self, results: &super::ExecutionResults) -> Result<(), CoreError> {
        self.check_key(&results.key)?;
        if let Some(record) = self.active.get(&results.key.run_id) {
            if record.key != results.key
                || record.targets != results.targets
                || !Arc::ptr_eq(&record.plan, &results.plan)
            {
                return Err(CoreError::new(
                    "run-key-conflict",
                    "collector belongs to another plan or run",
                ));
            }
        } else {
            self.get_run(&results.key)?;
        }
        Ok(())
    }
    /// Commit projected results once; a read-only preview is never completion.
    pub fn finish_collected(
        &mut self,
        results: &mut super::ExecutionResults,
        reason: TerminalReason,
    ) -> Result<RunSnapshot, CoreError> {
        self.check_collector(results)?;
        if !self.active.contains_key(&results.key.run_id) {
            let snapshot = self.get_run(&results.key)?;
            results.seal(snapshot.result.clone().expect("terminal"));
            return Ok(snapshot);
        }
        let result = results.project(reason)?;
        if result.reason == TerminalReason::Expired
            && self.active[&results.key.run_id]
                .deadline
                .is_none_or(|deadline| deadline > self.now)
        {
            return Err(CoreError::new(
                "invalid-deadline",
                "run deadline has not elapsed",
            ));
        }
        if result.reason == TerminalReason::Completed {
            result.validate(&results.plan, &results.targets)?;
        }
        let snapshot = self.end_run(&results.key, result)?;
        results.seal(snapshot.result.clone().expect("terminal"));
        Ok(snapshot)
    }
    fn end_run(
        &mut self,
        key: &RunKey,
        mut result: TerminalResult,
    ) -> Result<RunSnapshot, CoreError> {
        let sequence = self
            .seq
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        let mut record = self
            .active
            .remove(&key.run_id)
            .ok_or_else(|| CoreError::new("run-unknown", "run is not active"))?;
        let elapsed = self.elapsed(record.accepted_at);
        let mut bytes = retained_bytes(key, elapsed, &result)?;
        if bytes > self.host.policy().max_result_bytes_per_run {
            result = TerminalResult::limited(&record.targets);
            bytes = retained_bytes(key, elapsed, &result)?;
        }
        let snapshot = RunSnapshot {
            key: key.clone(),
            state: RunPhase::Terminal,
            event_seq: DecimalId::new(sequence)?,
            elapsed_ms: elapsed,
            current: self.current(key),
            physical: record.physical,
            poll_after_ms: 0,
            result: Some(result.clone()),
        };
        let trace = record.trace.clone();
        let host = self.host.clone();
        let mut cache = host.retention.lock().unwrap_or_else(|p| p.into_inner());
        let reservation = ResourceUsage {
            retained_runs: 1,
            retained_bytes: bytes,
            dedupe_entries: 1,
            ..Default::default()
        };
        match cache.reserve(&host, reservation, Some(&mut record.lease)) {
            Ok(lease) => {
                cache.records.insert(
                    key.run_id,
                    CachedRun {
                        trace: record.trace.clone(),
                        key: key.clone(),
                        start_key: record.start_key,
                        payload: record.payload,
                        ended_at: self.now,
                        elapsed_ms: elapsed,
                        result,
                        physical: record.physical,
                        lease,
                    },
                );
            }
            Err(_) => {
                drop(record);
                cache.tombstone(&host, key.clone());
            }
        }
        let available = cache.records.contains_key(&key.run_id);
        drop(cache);
        self.seq = sequence;
        if let Some(trace) = &trace {
            use super::TraceKind;
            let event = match snapshot.result.as_ref().expect("terminal result").reason {
                TerminalReason::Cancelled => TraceKind::RunCancelled,
                TerminalReason::Expired => TraceKind::RunExpired,
                TerminalReason::Failed => TraceKind::RunFailed,
                TerminalReason::Completed => {
                    match snapshot.result.as_ref().expect("terminal result").delivery {
                        super::RunDelivery::Complete | super::RunDelivery::Recovered
                            if snapshot.current =>
                        {
                            TraceKind::CoreResultReady
                        }
                        super::RunDelivery::Complete | super::RunDelivery::Recovered => {
                            TraceKind::ResultSuperseded
                        }
                        super::RunDelivery::NoOutput => TraceKind::NoOutput,
                        _ => TraceKind::RunCompleted,
                    }
                }
            };
            trace.emit(
                if available {
                    event
                } else {
                    TraceKind::ResultUnavailable
                },
                None,
                None,
            );
        }
        Ok(snapshot)
    }
    pub fn cancel_run(&mut self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.check_key(key)?;
        if let Some(r) = self.active.get(&key.run_id) {
            if &r.key != key {
                return Err(CoreError::new("run-key-conflict", "run key does not match"));
            }
            let result = TerminalResult::aborted(&r.targets, TerminalReason::Cancelled);
            self.end_run(key, result)
        } else {
            self.get_run(key)
        }
    }
    fn cancel_collected(
        &mut self,
        key: &RunKey,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<RunSnapshot, CoreError> {
        if let Some(results) = results.iter_mut().find(|results| &results.key == key) {
            self.finish_collected(results, TerminalReason::Cancelled)
        } else {
            self.cancel_run(key)
        }
    }
    pub fn advance_clock(&mut self, now: Duration) -> Result<(), CoreError> {
        self.advance_clock_with_results(now, &mut [])
    }
    pub(crate) fn validate_clock(&self, now: Duration) -> Result<(), CoreError> {
        if now < self.now
            || self.active.values().any(|r| {
                r.deadline.is_none_or(|deadline| deadline > now)
                    && now.saturating_sub(r.accepted_at).as_millis() > u128::from(MAX_SAFE_INTEGER)
            })
        {
            return Err(CoreError::new(
                "invalid-clock",
                "clock moved backwards or elapsed time is not representable",
            ));
        }
        Ok(())
    }
    /// Preserve observed diagnostics on expiry without polling workers or policy.
    pub fn advance_clock_with_results(
        &mut self,
        now: Duration,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<(), CoreError> {
        let mut seen = std::collections::BTreeSet::new();
        for results in results.iter() {
            self.check_collector(results)?;
            if !seen.insert(results.key.run_id) {
                return Err(CoreError::new(
                    "duplicate-result",
                    "collector supplied twice",
                ));
            }
        }
        self.validate_clock(now)?;
        self.seq
            .checked_add(self.active.len() as u64 + 1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        let mut due: Vec<_> = self
            .active
            .values()
            .filter_map(|r| {
                r.deadline
                    .filter(|deadline| *deadline <= now)
                    .map(|deadline| (deadline, r.key.clone()))
            })
            .collect();
        due.sort_by_key(|(deadline, key)| (*deadline, key.run_id));
        for (deadline, key) in due {
            self.now = deadline;
            let targets = self.active[&key.run_id].targets.clone();
            if let Some(results) = results.iter_mut().find(|results| results.key == key) {
                let result = results.project(TerminalReason::Expired)?;
                let snapshot = self.end_run(&key, result)?;
                results.seal(snapshot.result.expect("terminal"));
            } else {
                self.end_run(
                    &key,
                    TerminalResult::aborted(&targets, TerminalReason::Expired),
                )?;
            }
        }
        if self.now != now {
            self.now = now;
            self.bump()?;
        }
        let mut cache = self
            .host
            .retention
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        cache.advance(&self.host, now);
        Ok(())
    }
    pub fn invalidate_input(
        &mut self,
        expected: DecimalId,
        revision: u64,
    ) -> Result<(), CoreError> {
        self.invalidate_input_collected(expected, revision, &mut [])
    }
    pub(crate) fn invalidate_input_collected(
        &mut self,
        expected: DecimalId,
        revision: u64,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<(), CoreError> {
        self.check_version(expected)?;
        if !(1..=crate::node::MAX_INPUT_REVISION).contains(&revision) {
            return Err(CoreError::new(
                "invalid-runtime-input",
                "invalid input revision",
            ));
        }
        if self.latest_revision.is_some_and(|old| revision < old) {
            return Err(CoreError::new("stale-input", "input revision is stale"));
        }
        if self.latest_revision == Some(revision) {
            return Ok(());
        }
        let generation = self.generation.checked_next()?;
        self.seq
            .checked_add(self.active.len() as u64 + 1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        self.latest_revision = Some(revision);
        self.bound_input = None;
        self.generation = generation;
        self.bump()?;
        let keys: Vec<_> = self
            .active
            .values()
            .filter(|r| r.key.input_revision.is_some())
            .map(|r| r.key.clone())
            .collect();
        for key in keys {
            self.cancel_collected(&key, results)?;
        }
        Ok(())
    }
    pub fn prepare(&mut self, plan: ExecutionPlan) -> Result<(), CoreError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "revision exhausted"))?;
        self.pending = Some(Arc::new(plan));
        self.revision = revision;
        Ok(())
    }
    fn switch(
        &mut self,
        next: Arc<ExecutionPlan>,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<(), CoreError> {
        let version = self.version.checked_next()?;
        let generation = self.generation.checked_next()?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "revision exhausted"))?;
        self.seq
            .checked_add(self.active.len() as u64 + 1)
            .ok_or_else(|| CoreError::new("id-exhausted", "event sequence exhausted"))?;
        let keys: Vec<_> = self.active.values().map(|r| r.key.clone()).collect();
        for key in keys {
            self.cancel_collected(&key, results)?;
        }
        self.plan = next;
        self.version = version;
        self.generation = generation;
        self.revision = revision;
        self.bump()?;
        Ok(())
    }
    pub fn activate_pending(&mut self) -> Result<bool, CoreError> {
        self.activate_collected(&mut [])
    }
    pub(crate) fn activate_collected(
        &mut self,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<bool, CoreError> {
        let Some(next) = self.pending.clone() else {
            return Ok(false);
        };
        let previous = Arc::clone(&self.plan);
        self.switch(next, results)?;
        self.pending = None;
        self.previous = Some(previous);
        Ok(true)
    }
    pub fn rollback(&mut self) -> Result<bool, CoreError> {
        self.rollback_collected(&mut [])
    }
    pub(crate) fn rollback_collected(
        &mut self,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<bool, CoreError> {
        let Some(previous) = self.previous.clone() else {
            return Ok(false);
        };
        self.switch(previous, results)?;
        self.pending = None;
        self.previous = None;
        Ok(true)
    }
}

impl Drop for RunLedger {
    fn drop(&mut self) {
        self.trace.clear();
    }
}

#[cfg(test)]
mod admission_clock_tests {
    use super::super::{ExecutionClock, ResolvedPort, RunDelivery};
    use super::*;
    use serde_json::json;
    fn setup(name: &str) -> RunLedger {
        let mut policy = super::super::ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!(
                "../../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        policy.max_retained_bytes = policy.max_result_bytes_per_run;
        let host = ExecutionHost::new(name, policy).unwrap();
        let mut registry = crate::node::Registry::new();
        crate::builtins::register_demo_nodes(&mut registry).unwrap();
        let graph=crate::graph::Graph::from_json(&json!({"version":1,"nodes":[{"id":"a","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"data"}}}}],"edges":[]}).to_string()).unwrap();
        RunLedger::new(
            host,
            SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            DecimalId::new(1).unwrap(),
        )
    }
    fn request(start: u64, budget: Option<u64>) -> StartRequest {
        serde_json::from_value(json!({"startKey":start.to_string(),"expectedPlanVersion":"1","targets":[{"node":"a","port":"out"}],"runBudgetMs":budget})).unwrap()
    }
    fn finish(ledger: &mut RunLedger, key: &RunKey) {
        ledger
            .finish_run(
                key,
                TerminalResult::completed(
                    vec![Endpoint::new("a", "out")],
                    vec![ResolvedPort {
                        endpoint: Endpoint::new("a", "out"),
                        value: json!({"status":"no-output"}),
                    }],
                    RunDelivery::NoOutput,
                ),
            )
            .unwrap();
    }
    #[test]
    fn a_due_clock_guard_rejects_before_result_eviction_or_start_key_mutation() {
        let mut ledger = setup("native-admit-retry");
        let first = ledger.start_run(request(1, None)).unwrap().key;
        finish(&mut ledger, &first);
        let before = ledger.host.usage();
        let generation = ledger.generation;
        let clock = ExecutionClock::controlled(std::time::Duration::from_millis(5));
        let guard = super::super::clock::AdmissionClock {
            clock,
            minimum: Duration::ZERO,
            before: Some(Duration::from_millis(5)),
        };
        assert_eq!(
            ledger
                .admit_clocked(
                    request(2, Some(5)),
                    ResourceUsage::default(),
                    &mut [],
                    Some(&guard),
                    None
                )
                .unwrap_err()
                .code,
            "clock-retry"
        );
        assert_eq!(ledger.host.usage(), before);
        assert!(ledger.get_run(&first).is_ok());
        assert_eq!(ledger.generation, generation);
        assert_eq!(
            ledger
                .get_start(DecimalId::new(2).unwrap())
                .unwrap_err()
                .code,
            "start-not-seen"
        );
    }
    #[test]
    fn accepted_time_is_the_atomic_reservation_time_after_preflight() {
        let mut ledger = setup("native-admit-anchor");
        let guard = super::super::clock::AdmissionClock {
            clock: ExecutionClock::controlled(Duration::from_millis(20)),
            minimum: Duration::ZERO,
            before: None,
        };
        let (receipt, _) = ledger
            .admit_clocked(
                request(1, Some(5)),
                ResourceUsage::default(),
                &mut [],
                Some(&guard),
                None,
            )
            .unwrap();
        assert_eq!(ledger.now(), Duration::from_millis(20));
        assert_eq!(
            ledger.active[&receipt.key.run_id].accepted_at,
            Duration::from_millis(20)
        );
        assert_eq!(
            ledger.run_deadline(&receipt.key),
            Some(Duration::from_millis(25))
        );
        assert_eq!(ledger.get_run(&receipt.key).unwrap().elapsed_ms, 0);
    }
    #[test]
    fn deadline_overflow_at_actual_reservation_time_does_not_evict_old_results() {
        let mut ledger = setup("native-admit-overflow");
        let first = ledger.start_run(request(1, None)).unwrap().key;
        finish(&mut ledger, &first);
        let before = ledger.host.usage();
        let guard = super::super::clock::AdmissionClock {
            clock: ExecutionClock::controlled(Duration::MAX),
            minimum: Duration::ZERO,
            before: None,
        };
        assert_eq!(
            ledger
                .admit_clocked(
                    request(2, Some(1)),
                    ResourceUsage::default(),
                    &mut [],
                    Some(&guard),
                    None
                )
                .unwrap_err()
                .code,
            "invalid-deadline"
        );
        assert_eq!(ledger.host.usage(), before);
        assert!(ledger.get_run(&first).is_ok());
        assert_eq!(ledger.now(), Duration::ZERO);
    }
}
