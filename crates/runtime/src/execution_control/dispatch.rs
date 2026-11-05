//! Admission + FIFO ownership. Clock/observation/graph event driving is host-owned.
use super::queue_bytes::{Ownership, UnitKey};
use super::{
    ControlObserver, DecimalId, ExecutionHost, GraphExecution, GraphNodePhase, JobStamp,
    OwnedInvocation, PhysicalCounts, ReleasedNode, ResourceLease, ResourceUsage, RunKey, RunLedger,
    StartReceipt, StartRequest, WorkerCompletion, WorkerDispatcher, WorkerJobState,
};
use crate::node::{CoreError, RegisteredImplementation};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Weak},
    time::Duration,
};

struct ByteUnit {
    _lease: ResourceLease,
}
enum PlannedUnit {
    Existing(Arc<ByteUnit>),
    New(u64),
}
#[derive(Default)]
struct BytePlan {
    units: BTreeMap<UnitKey, PlannedUnit>,
    bytes: u64,
}
struct EntryBytes {
    metadata: u64,
    units: Vec<UnitKey>,
}
impl BytePlan {
    fn add(
        &mut self,
        ownership: Ownership,
        pool: &BTreeMap<UnitKey, Weak<ByteUnit>>,
    ) -> Result<EntryBytes, CoreError> {
        self.bytes = checked_add(self.bytes, ownership.metadata)?;
        let mut keys = vec![];
        for (key, bytes) in ownership.units {
            keys.push(key);
            if let std::collections::btree_map::Entry::Vacant(entry) = self.units.entry(key) {
                if let Some(unit) = pool.get(&key).and_then(Weak::upgrade) {
                    entry.insert(PlannedUnit::Existing(unit));
                } else {
                    self.bytes = checked_add(self.bytes, bytes)?;
                    entry.insert(PlannedUnit::New(bytes));
                }
            }
        }
        Ok(EntryBytes {
            metadata: ownership.metadata,
            units: keys,
        })
    }
    fn materialize(
        self,
        lease: &mut ResourceLease,
        pool: &mut BTreeMap<UnitKey, Weak<ByteUnit>>,
    ) -> BTreeMap<UnitKey, Arc<ByteUnit>> {
        self.units
            .into_iter()
            .map(|(key, unit)| {
                let unit = match unit {
                    PlannedUnit::Existing(unit) => unit,
                    PlannedUnit::New(bytes) => Arc::new(ByteUnit {
                        _lease: lease
                            .split(ResourceUsage {
                                queued_input_bytes: bytes,
                                ..Default::default()
                            })
                            .expect("validated byte reservation"),
                    }),
                };
                pool.insert(key, Arc::downgrade(&unit));
                (key, unit)
            })
            .collect()
    }
}
enum Placement {
    Physical,
    Queued(EntryBytes),
}
struct Initial {
    index: usize,
    job: Option<DecimalId>,
    placement: Option<Placement>,
}
struct Queued {
    stamp: JobStamp,
    invocation: OwnedInvocation,
    lease: ResourceLease,
    _bytes: Vec<Arc<ByteUnit>>,
    deadline: Option<Duration>,
    budget: Option<super::Milliseconds>,
    run_deadline: Option<Duration>,
}
struct Running {
    stamp: JobStamp,
    deadline: Option<Duration>,
    accepts_result: bool,
    physically_finished: bool,
}
struct HandoffBudget {
    job: Option<super::Milliseconds>,
    run_deadline: Option<Duration>,
    transferred_at: Option<Duration>,
}
/// One uncommitted authenticated notification. Recheck time/Graph validation,
/// then commit it even when its logical result has become obsolete.
pub struct PreparedDispatchEvent {
    owner: Arc<()>,
    receipt: Arc<()>,
    event: WorkerCompletion,
}
impl PreparedDispatchEvent {
    pub fn completion(&self) -> &WorkerCompletion {
        &self.event
    }
}
#[derive(Clone)]
enum EventKind {
    Completion(WorkerCompletion),
    Failure { stamp: JobStamp, error: CoreError },
}
/// A Core-issued event. IDs alone do not authorize injecting a dispatch failure.
#[derive(Clone)]
pub struct DispatchEvent {
    inner: EventKind,
}
impl DispatchEvent {
    fn failure(stamp: JobStamp, error: CoreError) -> Self {
        Self {
            inner: EventKind::Failure { stamp, error },
        }
    }
    pub fn stamp(&self) -> &JobStamp {
        match &self.inner {
            EventKind::Completion(event) => event.stamp(),
            EventKind::Failure { stamp, .. } => stamp,
        }
    }
    pub fn error(&self) -> Option<&CoreError> {
        match &self.inner {
            EventKind::Failure { error, .. } => Some(error),
            _ => None,
        }
    }
    pub fn completion(&self) -> Option<&WorkerCompletion> {
        match &self.inner {
            EventKind::Completion(event) => Some(event),
            _ => None,
        }
    }
    pub fn apply(self, graph: &mut GraphExecution) -> Result<Vec<ReleasedNode>, CoreError> {
        match self.inner {
            EventKind::Completion(event) => graph.complete_work(event),
            EventKind::Failure { stamp, error } => graph.fail_bound(&stamp, error),
        }
    }
}
pub struct AdmittedGraph {
    pub receipt: StartReceipt,
    /// Replays return no new graph, observers or work.
    pub graph: Option<GraphExecution>,
    pub observers: BTreeMap<String, ControlObserver>,
    pub events: Vec<DispatchEvent>,
    pub cancelled_runs: Vec<RunKey>,
}
pub struct DispatchQueue {
    host: ExecutionHost,
    workers: WorkerDispatcher,
    pending: VecDeque<Queued>,
    running: BTreeMap<DecimalId, Running>,
    pool: BTreeMap<UnitKey, Weak<ByteUnit>>,
    now: Duration,
    staged: Option<Weak<()>>,
    clock: Option<super::ExecutionClock>,
    retry_clock: bool,
    _owner: Arc<()>,
}
impl DispatchQueue {
    pub(crate) fn belongs_to(&self, host: &ExecutionHost) -> bool {
        self.host.same_host(host)
    }
    pub fn new(host: ExecutionHost) -> Result<Self, CoreError> {
        Self::new_inner(host, None)
    }
    pub(crate) fn clocked(
        host: ExecutionHost,
        clock: super::ExecutionClock,
    ) -> Result<Self, CoreError> {
        Self::new_inner(host, Some(clock))
    }
    fn new_inner(
        host: ExecutionHost,
        clock: Option<super::ExecutionClock>,
    ) -> Result<Self, CoreError> {
        let owner = host.claim_dispatcher()?;
        Ok(Self {
            workers: WorkerDispatcher::new(host.clone()),
            host,
            pending: VecDeque::new(),
            running: BTreeMap::new(),
            pool: BTreeMap::new(),
            now: Duration::ZERO,
            staged: None,
            clock,
            retry_clock: false,
            _owner: owner,
        })
    }
    fn check_time(&self, now: Duration) -> Result<(), CoreError> {
        if now < self.now {
            return Err(CoreError::new(
                "invalid-clock",
                "dispatch clock moved backwards",
            ));
        }
        Ok(())
    }
    fn queued_deadline(&self, now: Duration) -> Result<Option<Duration>, CoreError> {
        self.host
            .policy()
            .queue_wait_ms
            .map(|delay| delay.checked_deadline(now))
            .transpose()
    }
    fn clean_pool(&mut self) {
        self.pool.retain(|_, unit| unit.strong_count() > 0);
    }
    /// Reserve known work + observers in the same atomic exchange as the run/result.
    /// No node callback or worker is invoked during forecast or failed admission.
    pub fn admit(
        &mut self,
        ledger: &mut RunLedger,
        request: StartRequest,
    ) -> Result<AdmittedGraph, CoreError> {
        self.admit_collected(ledger, request, &mut [])
    }
    pub(crate) fn admit_collected(
        &mut self,
        ledger: &mut RunLedger,
        request: StartRequest,
        results: &mut [&mut super::ExecutionResults],
    ) -> Result<AdmittedGraph, CoreError> {
        self.admit_clocked(ledger, request, results, None)
    }
    pub(crate) fn admit_clocked(
        &mut self,
        ledger: &mut RunLedger,
        request: StartRequest,
        results: &mut [&mut super::ExecutionResults],
        clock: Option<&super::clock::AdmissionClock>,
    ) -> Result<AdmittedGraph, CoreError> {
        if !self.host.same_host(ledger.host()) {
            return Err(CoreError::new(
                "resource-domain-conflict",
                "ledger and queue must share an ExecutionHost",
            ));
        }
        match ledger.get_start(request.start_key) {
            Err(error) if error.code == "start-not-seen" => {}
            _ => {
                let receipt = ledger.start_run(request)?;
                return Ok(AdmittedGraph {
                    receipt,
                    graph: None,
                    observers: BTreeMap::new(),
                    events: vec![],
                    cancelled_runs: vec![],
                });
            }
        }
        let now = ledger.now();
        self.check_time(now)?;
        let plan = ledger.admission_plan(&request)?;
        let budgets = request.job_budgets.clone();
        let input = request.input.clone().map(Arc::new);
        let mut available = if self.pending.is_empty() {
            self.host
                .policy()
                .max_physical_jobs
                .saturating_sub(self.host.usage().physical_jobs)
        } else {
            0
        };
        let mut bytes = BytePlan::default();
        let mut demand = ResourceUsage::default();
        let mut initial = vec![];
        for index in super::results::target_scope(&plan, &request.targets) {
            let node = &plan.nodes[index];
            if matches!(
                node.node.implementation,
                RegisteredImplementation::Control { .. }
            ) {
                demand.observers = checked_add(demand.observers, 1)?;
                initial.push(Initial {
                    index,
                    job: None,
                    placement: None,
                });
            } else if node.sources.is_empty() {
                let placement = if available > 0 {
                    available -= 1;
                    demand.physical_jobs = checked_add(demand.physical_jobs, 1)?;
                    Placement::Physical
                } else {
                    demand.queued_jobs = checked_add(demand.queued_jobs, 1)?;
                    Placement::Queued(bytes.add(
                        Ownership::initial(&plan, index, input.as_ref())?,
                        &self.pool,
                    )?)
                };
                initial.push(Initial {
                    index,
                    job: Some(super::worker::issue_job_id()?),
                    placement: Some(placement),
                });
            }
        }
        demand.queued_input_bytes = bytes.bytes;
        let _deadline = if demand.queued_jobs > 0 {
            self.queued_deadline(now)?
        } else {
            None
        };
        let old_runs = ledger.active_keys();
        let queue_wait = if demand.queued_jobs > 0 {
            self.host.policy().queue_wait_ms
        } else {
            None
        };
        let (receipt, lease) = ledger.admit_clocked(request, demand, results, clock, queue_wait)?;
        let now = ledger.now();
        let deadline = if demand.queued_jobs > 0 {
            self.queued_deadline(now)
                .expect("validated queue deadline at reservation")
        } else {
            None
        };
        let cancelled_runs: Vec<_> = old_runs
            .into_iter()
            .filter(|key| !ledger.active_key(key))
            .collect();
        for key in &cancelled_runs {
            self.cancel_run(key, now)
                .expect("validated monotonic admission time");
        }
        let mut lease = lease.expect("fresh admission owns reserved work");
        let units = bytes.materialize(&mut lease, &mut self.pool);
        let mut graph = GraphExecution::new(
            self.host.clone(),
            ledger
                .result_collector(&receipt.key)
                .expect("accepted run owns plan"),
            input,
            now,
            ledger.trace_ticket(&receipt.key),
        )
        .expect("validated initial graph");
        graph.set_job_budgets(&budgets);
        graph.set_run_deadline(ledger.run_deadline(&receipt.key));
        let mut observers = BTreeMap::new();
        let mut handoffs = vec![];
        for entry in initial {
            let node = &plan.nodes[entry.index].id;
            if let Some(job) = entry.job {
                let invocation = graph.start_work(node).expect("forecast work is ready");
                graph.bind_job(node, job).expect("issued job is unique");
                let stamp = JobStamp {
                    key: receipt.key.clone(),
                    job_id: job,
                };
                match entry.placement.expect("work placement") {
                    Placement::Physical => handoffs.push((
                        stamp,
                        invocation,
                        lease
                            .split(ResourceUsage {
                                physical_jobs: 1,
                                ..Default::default()
                            })
                            .expect("reserved physical slot"),
                    )),
                    Placement::Queued(bytes) => {
                        invocation
                            .trace_event(super::TraceKind::ResourceWait, Some(stamp.job_id.get()));
                        let reservation = lease
                            .split(ResourceUsage {
                                queued_jobs: 1,
                                queued_input_bytes: bytes.metadata,
                                ..Default::default()
                            })
                            .expect("reserved queue entry");
                        self.pending.push_back(Queued {
                            stamp,
                            invocation,
                            lease: reservation,
                            _bytes: bytes
                                .units
                                .iter()
                                .map(|key| Arc::clone(&units[key]))
                                .collect(),
                            deadline,
                            budget: graph.job_budget(node),
                            run_deadline: graph.run_deadline(),
                        });
                    }
                }
            } else {
                let observer = graph
                    .start_observer_reserved(
                        node,
                        lease
                            .split(ResourceUsage {
                                observers: 1,
                                ..Default::default()
                            })
                            .expect("reserved observer"),
                    )
                    .expect("forecast control is pending");
                observers.insert(node.clone(), observer);
            }
        }
        debug_assert_eq!(lease.usage(), ResourceUsage::default());
        self.now = now;
        let mut events = vec![];
        for (stamp, invocation, lease) in handoffs {
            let budget = graph.job_budget(invocation.context().node_id);
            if let Some(event) = self.handoff(
                stamp,
                invocation,
                lease,
                HandoffBudget {
                    job: budget,
                    run_deadline: graph.run_deadline(),
                    transferred_at: None,
                },
                now,
            ) {
                events.push(event);
            }
        }
        self.clean_pool();
        Ok(AdmittedGraph {
            receipt,
            graph: Some(graph),
            observers,
            events,
            cancelled_runs,
        })
    }
    fn handoff(
        &mut self,
        stamp: JobStamp,
        invocation: OwnedInvocation,
        lease: ResourceLease,
        budget: HandoffBudget,
        now: Duration,
    ) -> Option<DispatchEvent> {
        let now = match (&self.clock, budget.transferred_at) {
            (_, Some(at)) => at,
            (Some(clock), None) => match clock.now() {
                Ok(at) if at >= now => at,
                Ok(_) => {
                    return Some(DispatchEvent::failure(
                        stamp,
                        CoreError::new("invalid-clock", "handoff clock moved backwards"),
                    ));
                }
                Err(error) => return Some(DispatchEvent::failure(stamp, error)),
            },
            (None, None) => now,
        };
        if budget.run_deadline.is_some_and(|deadline| deadline <= now) {
            return Some(DispatchEvent::failure(
                stamp,
                CoreError::new("run-expired", "run expired before physical handoff"),
            ));
        }
        let deadline = match budget
            .job
            .map(|budget| budget.checked_deadline(now))
            .transpose()
        {
            Ok(deadline) => deadline,
            Err(error) => return Some(DispatchEvent::failure(stamp, error)),
        };
        match self
            .workers
            .submit_reserved(invocation, stamp.job_id, lease)
        {
            Ok(()) => {
                self.running.insert(
                    stamp.job_id,
                    Running {
                        stamp,
                        deadline,
                        accepts_result: true,
                        physically_finished: false,
                    },
                );
                None
            }
            Err(error) => Some(DispatchEvent::failure(stamp, error)),
        }
    }
    /// Later ready work is admitted individually. A capacity failure is a NodeError,
    /// never a reversal of the run's accepted receipt. Initial observers already exist.
    pub fn schedule_ready(
        &mut self,
        graph: &mut GraphExecution,
        now: Duration,
    ) -> Result<Vec<DispatchEvent>, CoreError> {
        if !graph.belongs_to(&self.host) {
            return Err(CoreError::new(
                "resource-domain-conflict",
                "graph and queue must share an ExecutionHost",
            ));
        }
        self.check_time(now)?;
        let ready = graph.ready_nodes()?;
        if ready
            .iter()
            .any(|node| node.execution_kind == super::ExecutionKind::Control)
        {
            return Err(CoreError::new(
                "invalid-dispatch",
                "initial observers must be admitted together with the run",
            ));
        }
        let mut events = vec![];
        let ids = super::worker::issue_job_ids(ready.len())?;
        for (node, job) in ready.into_iter().zip(ids) {
            if graph.phase(&node.node_id) != Some(GraphNodePhase::Pending) {
                continue;
            }
            let invocation = graph.preview_work(&node.node_id)?;
            let planned = self.reserve_later(&invocation, now);
            let invocation = graph.start_work(&node.node_id)?;
            graph.bind_job(&node.node_id, job)?;
            let stamp = JobStamp {
                key: graph.key().clone(),
                job_id: job,
            };
            match planned {
                Ok((lease, placement, units)) => match placement {
                    None => {
                        let budget = graph.job_budget(&node.node_id);
                        if let Some(event) = self.handoff(
                            stamp,
                            invocation,
                            lease,
                            HandoffBudget {
                                job: budget,
                                run_deadline: graph.run_deadline(),
                                transferred_at: None,
                            },
                            now,
                        ) {
                            events.push(event);
                        }
                    }
                    Some((_, deadline)) => {
                        invocation
                            .trace_event(super::TraceKind::ResourceWait, Some(stamp.job_id.get()));
                        self.pending.push_back(Queued {
                            stamp,
                            invocation,
                            lease,
                            _bytes: units,
                            deadline,
                            budget: graph.job_budget(&node.node_id),
                            run_deadline: graph.run_deadline(),
                        });
                    }
                },
                Err(error) => {
                    let error = if error.code == "busy" {
                        CoreError::new("resource-busy", "work/queue reservation is full")
                    } else {
                        error
                    };
                    events.push(DispatchEvent::failure(stamp, error));
                }
            }
        }
        self.now = now;
        self.clean_pool();
        Ok(events)
    }
    fn reserve_later(
        &mut self,
        invocation: &OwnedInvocation,
        now: Duration,
    ) -> Result<LaterReservation, CoreError> {
        if self.pending.is_empty()
            && self.host.usage().physical_jobs < self.host.policy().max_physical_jobs
        {
            return Ok((
                self.host.try_reserve(ResourceUsage {
                    physical_jobs: 1,
                    ..Default::default()
                })?,
                None,
                vec![],
            ));
        }
        let deadline = self.queued_deadline(now)?;
        let mut bytes = BytePlan::default();
        let entry = bytes.add(Ownership::invocation(invocation)?, &self.pool)?;
        let mut lease = self.host.try_reserve(ResourceUsage {
            queued_jobs: 1,
            queued_input_bytes: bytes.bytes,
            ..Default::default()
        })?;
        let units = bytes.materialize(&mut lease, &mut self.pool);
        let owners = entry
            .units
            .iter()
            .map(|key| Arc::clone(&units[key]))
            .collect();
        debug_assert_eq!(
            lease.usage(),
            ResourceUsage {
                queued_jobs: 1,
                queued_input_bytes: entry.metadata,
                ..Default::default()
            }
        );
        Ok((lease, Some((entry, deadline)), owners))
    }
    pub fn wake_signal(&self) -> super::CompletionWake {
        self.workers.wake_signal()
    }
    pub(crate) fn has_pending_events(&self) -> bool {
        self.workers.has_pending()
    }
    pub fn poll_one(&mut self) -> Option<DispatchEvent> {
        let prepared = self.prepare_event().ok().flatten()?;
        self.commit_event(prepared).ok()
    }
    pub fn prepare_event(&mut self) -> Result<Option<PreparedDispatchEvent>, CoreError> {
        if self.staged.as_ref().and_then(Weak::upgrade).is_some() {
            return Err(CoreError::new(
                "dispatch-event-pending",
                "commit the prepared event before reading another",
            ));
        }
        self.staged = None;
        let Some(event) = self.workers.poll_one() else {
            return Ok(None);
        };
        if event.physically_finished()
            && let Some(running) = self.running.get_mut(&event.stamp().job_id)
        {
            running.physically_finished = true;
        }
        // Retain the logical deadline through all deferred output/Graph validation.
        let receipt = Arc::new(());
        self.staged = Some(Arc::downgrade(&receipt));
        Ok(Some(PreparedDispatchEvent {
            owner: Arc::clone(&self._owner),
            receipt,
            event,
        }))
    }
    pub fn commit_event(
        &mut self,
        prepared: PreparedDispatchEvent,
    ) -> Result<DispatchEvent, CoreError> {
        if !Arc::ptr_eq(&prepared.owner, &self._owner)
            || self
                .staged
                .as_ref()
                .and_then(Weak::upgrade)
                .is_none_or(|receipt| !Arc::ptr_eq(&receipt, &prepared.receipt))
        {
            return Err(CoreError::new(
                "dispatch-event-conflict",
                "prepared event belongs to another queue or turn",
            ));
        }
        self.staged = None;
        let mut event = prepared.event;
        if let Some(running) = self.running.get_mut(&event.stamp().job_id) {
            if !running.accepts_result {
                event.execution = None;
            }
            if event.execution().is_some() {
                running.deadline = None;
                running.accepts_result = false;
            }
            if running.physically_finished && !running.accepts_result {
                self.running.remove(&event.stamp().job_id);
            }
        } else {
            // Expiry/cancel can retire an already physically-ended row while the
            // owner is validating this event. Its old result cannot resurrect it.
            event.execution = None;
        }
        Ok(DispatchEvent {
            inner: EventKind::Completion(event),
        })
    }
    pub fn poll(&mut self) -> Vec<DispatchEvent> {
        std::iter::from_fn(|| self.poll_one()).collect()
    }
    pub fn expire_queue(&mut self, now: Duration) -> Result<Vec<DispatchEvent>, CoreError> {
        self.check_time(now)?;
        let mut keep = VecDeque::new();
        let mut events = vec![];
        while let Some(entry) = self.pending.pop_front() {
            if entry.deadline.is_some_and(|deadline| deadline <= now) {
                events.push(DispatchEvent::failure(
                    entry.stamp.clone(),
                    CoreError::new("queue-expired", "queue wait budget elapsed"),
                ));
            } else {
                keep.push_back(entry);
            }
        }
        self.pending = keep;
        self.now = now;
        self.clean_pool();
        Ok(events)
    }
    /// Call after due run/job/observer events and cancellation have been handled.
    pub fn dispatch_queued(&mut self, now: Duration) -> Result<Vec<DispatchEvent>, CoreError> {
        let mut events = self.expire_queue(now)?;
        let clock = self.clock.clone();
        while let Some(entry) = self.pending.front_mut() {
            let guard = clock.as_ref().map(|clock| super::clock::AdmissionClock {
                clock: clock.clone(),
                minimum: now,
                before: entry.deadline.into_iter().chain(entry.run_deadline).min(),
            });
            let transferred_at = match entry.lease.replace_checked(
                ResourceUsage {
                    physical_jobs: 1,
                    ..Default::default()
                },
                || guard.as_ref().map(|guard| guard.sample()).transpose(),
            ) {
                Ok(at) => at,
                Err(error) if error.code == "busy" => break,
                Err(error) if error.code == "clock-retry" => {
                    self.retry_clock = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            let entry = self.pending.pop_front().expect("front exists");
            if let Some(event) = self.handoff(
                entry.stamp,
                entry.invocation,
                entry.lease,
                HandoffBudget {
                    job: entry.budget,
                    run_deadline: entry.run_deadline,
                    transferred_at,
                },
                now,
            ) {
                events.push(event);
            }
        }
        self.clean_pool();
        Ok(events)
    }
    pub(crate) fn take_clock_retry(&mut self) -> bool {
        std::mem::take(&mut self.retry_clock)
    }
    pub fn cancel(&mut self, stamp: &JobStamp, now: Duration) -> Result<bool, CoreError> {
        self.check_time(now)?;
        if let Some(index) = self.pending.iter().position(|entry| &entry.stamp == stamp) {
            self.pending.remove(index);
            self.now = now;
            self.clean_pool();
            return Ok(true);
        }
        if self
            .running
            .get(&stamp.job_id)
            .is_none_or(|running| &running.stamp != stamp)
        {
            return Ok(false);
        }
        let running = self
            .running
            .get_mut(&stamp.job_id)
            .expect("checked running job");
        running.deadline = None;
        running.accepts_result = false;
        let finished = running.physically_finished;
        self.now = now;
        self.workers.cancel(stamp.job_id, now);
        if finished {
            self.running.remove(&stamp.job_id);
        }
        Ok(true)
    }
    pub fn cancel_run(&mut self, key: &RunKey, now: Duration) -> Result<usize, CoreError> {
        self.check_time(now)?;
        let stamps: Vec<_> = self
            .pending
            .iter()
            .map(|entry| &entry.stamp)
            .chain(self.running.values().map(|running| &running.stamp))
            .filter(|stamp| &stamp.key == key)
            .cloned()
            .collect();
        let n = stamps.len();
        for stamp in stamps {
            self.cancel(&stamp, now)?;
        }
        Ok(n)
    }
    pub fn cancel_released(
        &mut self,
        key: &RunKey,
        nodes: &[ReleasedNode],
        now: Duration,
    ) -> Result<(), CoreError> {
        self.check_time(now)?;
        for node in nodes {
            if let Some(job_id) = node.job_id {
                self.cancel(
                    &JobStamp {
                        key: key.clone(),
                        job_id,
                    },
                    now,
                )?;
            }
        }
        Ok(())
    }
    pub fn mark_stalled(&mut self, now: Duration) -> Result<(), CoreError> {
        self.check_time(now)?;
        self.workers
            .mark_stalled(now, self.host.policy().cancel_grace_ms);
        self.now = now;
        Ok(())
    }
    pub fn counts(&self, key: &RunKey) -> PhysicalCounts {
        let mut counts = PhysicalCounts {
            queued: self
                .pending
                .iter()
                .filter(|entry| &entry.stamp.key == key)
                .count() as u64,
            ..Default::default()
        };
        for (run, state) in self.workers.states() {
            if run == key {
                match state {
                    WorkerJobState::Running => counts.running += 1,
                    WorkerJobState::CancelRequested => counts.cancel_requested += 1,
                    WorkerJobState::Stalled => counts.stalled += 1,
                }
            }
        }
        counts
    }
    pub fn queued(&self) -> impl Iterator<Item = &JobStamp> {
        self.pending.iter().map(|entry| &entry.stamp)
    }
    pub fn next_deadline(&self) -> Option<Duration> {
        self.next_queue_deadline()
            .into_iter()
            .chain(self.next_job_deadline())
            .min()
    }
    pub fn next_queue_deadline(&self) -> Option<Duration> {
        self.pending.iter().filter_map(|entry| entry.deadline).min()
    }
    pub fn next_job_deadline(&self) -> Option<Duration> {
        self.running
            .values()
            .filter_map(|entry| entry.deadline)
            .min()
    }
    pub(crate) fn next_grace_deadline(&self) -> Option<Duration> {
        self.workers
            .next_grace_deadline(self.host.policy().cancel_grace_ms)
    }
    /// The host processes run deadlines first, then these job deadlines, before poll.
    pub fn expire_jobs(&mut self, now: Duration) -> Result<Vec<DispatchEvent>, CoreError> {
        self.check_time(now)?;
        let mut due: Vec<_> = self
            .running
            .iter()
            .filter_map(|(id, running)| {
                running
                    .deadline
                    .filter(|deadline| *deadline <= now)
                    .map(|deadline| (deadline, *id))
            })
            .collect();
        due.sort_unstable();
        let mut events = vec![];
        for (deadline, id) in due {
            let running = self.running.get_mut(&id).expect("known due job");
            running.deadline = None;
            running.accepts_result = false;
            self.workers.cancel(id, deadline);
            events.push(DispatchEvent::failure(
                running.stamp.clone(),
                CoreError::new("deadline-exceeded", "job execution budget elapsed"),
            ));
        }
        self.running
            .retain(|_, running| !running.physically_finished || running.accepts_result);
        self.now = now;
        Ok(events)
    }
}
type LaterReservation = (
    ResourceLease,
    Option<(EntryBytes, Option<Duration>)>,
    Vec<Arc<ByteUnit>>,
);
fn checked_add(a: u64, b: u64) -> Result<u64, CoreError> {
    a.checked_add(b)
        .ok_or_else(|| CoreError::new("busy", "resource forecast overflow"))
}

#[cfg(test)]
mod clock_handoff_tests {
    use super::*;
    use crate::{
        graph::{ExecutionPlan, Graph},
        node::{Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry, RunContext},
    };
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Count(Arc<AtomicU64>);
    impl Node for Count {
        fn definition(&self) -> NodeDefinition {
            NodeDefinition::new("test.count", 1, vec![], vec![Port::packet("out")])
        }
        fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok([("out".into(), OutputValue::NoOutput)].into())
        }
    }
    fn setup(
        name: &str,
    ) -> (
        ExecutionHost,
        RunLedger,
        DispatchQueue,
        Arc<AtomicU64>,
        super::super::ExecutionClock,
    ) {
        let profile = super::super::ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!(
                "../../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        let host = ExecutionHost::new(name, profile).unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let mut registry = Registry::new();
        registry.register(Count(Arc::clone(&calls))).unwrap();
        let graph = Graph::from_json(
            &json!({"version":1,"nodes":[{"id":"a","kind":"test.count","version":1}],"edges":[]})
                .to_string(),
        )
        .unwrap();
        let ledger = RunLedger::new(
            host.clone(),
            super::super::SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            DecimalId::new(1).unwrap(),
        );
        let clock = super::super::ExecutionClock::controlled(Duration::from_millis(10));
        let queue = DispatchQueue::clocked(host.clone(), clock.clone()).unwrap();
        (host, ledger, queue, calls, clock)
    }
    #[test]
    fn job_budget_is_anchored_to_actual_handoff_instead_of_the_old_ledger_time() {
        let (host, mut ledger, mut queue, _, clock) = setup("clock-handoff-anchor");
        let request=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"a","port":"out"}],"jobBudgets":[{"nodeId":"a","budgetMs":5}]})).unwrap();
        // Deliberately use the low-level frozen admission with a newer handoff
        // clock, isolating the physical-start anchor from the accepted anchor.
        let admitted = queue.admit(&mut ledger, request).unwrap();
        assert!(admitted.events.is_empty());
        assert_eq!(ledger.now(), Duration::ZERO);
        assert_eq!(queue.next_job_deadline(), Some(Duration::from_millis(15)));
        clock.advance_to(Duration::from_millis(14)).unwrap();
        assert!(
            queue
                .expire_jobs(Duration::from_millis(14))
                .unwrap()
                .is_empty()
        );
        clock.advance_to(Duration::from_millis(15)).unwrap();
        assert_eq!(
            queue.expire_jobs(Duration::from_millis(15)).unwrap()[0]
                .error()
                .unwrap()
                .code,
            "deadline-exceeded"
        );
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while host.usage().physical_jobs != 0 {
            queue.poll();
            assert!(std::time::Instant::now() < end);
            std::thread::yield_now();
        }
    }
    #[test]
    fn an_expired_run_is_not_handed_to_an_adapter_after_clock_recheck() {
        let (host, mut ledger, mut queue, calls, _) = setup("clock-handoff-expired");
        let request=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"a","port":"out"}],"runBudgetMs":5})).unwrap();
        let admitted = queue.admit(&mut ledger, request).unwrap();
        assert_eq!(admitted.events[0].error().unwrap().code, "run-expired");
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(host.usage().physical_jobs, 0);
    }
}
