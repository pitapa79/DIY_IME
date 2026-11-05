//! Single control owner with an explicit monotonic clock. No native timer thread.
use super::{
    CompletionWake, ControlObserver, DecimalId, DispatchEvent, DispatchQueue, ExecutionHost,
    GraphExecution, GraphObservation, RunKey, RunLedger, RunSnapshot, SessionEpoch, StartReceipt,
    StartRequest, TerminalReason, WakeReason,
};
use crate::{graph::ExecutionPlan, node::CoreError};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

struct Observer {
    handle: ControlObserver,
    frame: Option<Vec<bool>>,
    wake_at: Option<Duration>,
    timer_due: bool,
}
struct Run {
    graph: GraphExecution,
    observers: BTreeMap<String, Observer>,
}
struct PendingObservation {
    run: DecimalId,
    node: String,
    observer: Observer,
    prepared: super::PreparedGraphObservation,
    priority: bool,
}
/// Drives multiple sessions through one Host/FIFO. Call advance_to from a trusted
/// clock owner; read-only getters do not drain workers or run policy callbacks.
pub struct ExecutionPump {
    host: ExecutionHost,
    clock: Option<super::ExecutionClock>,
    queue: DispatchQueue,
    sessions: BTreeMap<String, RunLedger>,
    runs: BTreeMap<DecimalId, Run>,
    tracked: BTreeMap<DecimalId, RunKey>,
    now: Duration,
    driving: bool,
    policy_pending: bool,
    policy_revision: u64,
    pending_grace: Option<Duration>,
    pending_observations: VecDeque<PendingObservation>,
    #[cfg(test)]
    test_notices: Vec<(Duration, &'static str)>,
}
impl ExecutionPump {
    pub fn new(host: ExecutionHost) -> Result<Self, CoreError> {
        Self::new_inner(host, None)
    }
    pub fn with_clock(
        host: ExecutionHost,
        clock: super::ExecutionClock,
    ) -> Result<Self, CoreError> {
        Self::new_inner(host, Some(clock))
    }
    fn new_inner(
        host: ExecutionHost,
        clock: Option<super::ExecutionClock>,
    ) -> Result<Self, CoreError> {
        let now = clock
            .as_ref()
            .map(|clock| clock.now())
            .transpose()?
            .unwrap_or(Duration::ZERO);
        if clock.is_some()
            && now
                < host
                    .retention
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .now()
        {
            return Err(CoreError::new(
                "invalid-clock",
                "clock precedes retained Host state",
            ));
        }
        let queue = match &clock {
            Some(clock) => DispatchQueue::clocked(host.clone(), clock.clone())?,
            None => DispatchQueue::new(host.clone())?,
        };
        if let Some(clock) = &clock {
            clock.register(&queue.wake_signal());
        }
        Ok(Self {
            host,
            clock,
            queue,
            sessions: BTreeMap::new(),
            runs: BTreeMap::new(),
            tracked: BTreeMap::new(),
            now,
            driving: false,
            policy_pending: false,
            policy_revision: 0,
            pending_grace: None,
            pending_observations: VecDeque::new(),
            #[cfg(test)]
            test_notices: vec![],
        })
    }
    pub fn now(&self) -> Duration {
        self.now
    }
    pub fn wake_signal(&self) -> CompletionWake {
        self.queue.wake_signal()
    }
    pub(crate) fn has_pending_events(&self) -> bool {
        self.queue.has_pending_events()
    }
    pub(crate) fn get_run_at(&self, key: &RunKey, now: Duration) -> Result<RunSnapshot, CoreError> {
        let mut snapshot = self.get_run(key)?;
        if snapshot.state != super::RunPhase::Terminal {
            let at = self
                .session(&key.session_epoch)?
                .accepted_at(key)
                .expect("validated active run");
            let elapsed = now
                .checked_sub(at)
                .filter(|elapsed| elapsed.as_millis() <= u128::from(super::types::MAX_SAFE_INTEGER))
                .ok_or_else(|| {
                    CoreError::new(
                        "invalid-clock",
                        "snapshot elapsed time is not representable",
                    )
                })?;
            snapshot.elapsed_ms = elapsed.as_millis() as u64;
        }
        Ok(snapshot)
    }
    /// Close a logical session without pinning its active plan in the retention
    /// cache. Physical jobs keep their own plan/lease until actual termination.
    pub fn close_session(&mut self, epoch: &SessionEpoch) -> Result<bool, CoreError> {
        self.refresh()?;
        if !self.sessions.contains_key(epoch.as_str()) {
            return Ok(false);
        }
        let ids: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, run)| &run.graph.key().session_epoch == epoch)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let mut run = self.runs.remove(&id).expect("owned run");
            let key = run.graph.key().clone();
            run.graph.finish(
                self.sessions
                    .get_mut(epoch.as_str())
                    .expect("owned session"),
                TerminalReason::Cancelled,
            )?;
            self.queue.cancel_run(&key, self.now)?;
        }
        let keys: Vec<_> = self
            .tracked
            .values()
            .filter(|key| &key.session_epoch == epoch)
            .cloned()
            .collect();
        for key in keys {
            self.queue.cancel_run(&key, self.now)?;
        }
        self.sessions.remove(epoch.as_str());
        self.prune_prepared()?;
        self.sync_counts()?;
        Ok(true)
    }
    pub fn open_session(
        &mut self,
        host: ExecutionHost,
        epoch: SessionEpoch,
        plan: ExecutionPlan,
        version: DecimalId,
    ) -> Result<(), CoreError> {
        self.open_session_shared(host, epoch, Arc::new(plan), version, 0)
    }
    pub(crate) fn open_session_shared(
        &mut self,
        host: ExecutionHost,
        epoch: SessionEpoch,
        plan: Arc<ExecutionPlan>,
        version: DecimalId,
        revision: u64,
    ) -> Result<(), CoreError> {
        self.refresh()?;
        // Host identity is checked by DispatchQueue at admission, but reject a
        // foreign session here before adding state or accepting its clock.
        if !self.queue.belongs_to(&host) {
            return Err(CoreError::new(
                "resource-domain-conflict",
                "pump sessions must share its Host",
            ));
        }
        if self.sessions.contains_key(epoch.as_str()) {
            return Err(CoreError::new(
                "session-conflict",
                "session epoch already belongs to this pump",
            ));
        }
        let mut ledger = RunLedger::with_shared_plan(host, epoch.clone(), plan, version, revision);
        if ledger.now() > self.now {
            // A replacement pump must explicitly catch up to the shared domain
            // clock before sessions are opened; never rewind retained results.
            return Err(CoreError::new(
                "invalid-clock",
                "pump precedes shared Host clock",
            ));
        }
        ledger.advance_clock(self.now)?;
        self.sessions.insert(epoch.as_str().to_owned(), ledger);
        Ok(())
    }
    fn session(&self, epoch: &SessionEpoch) -> Result<&RunLedger, CoreError> {
        self.sessions
            .get(epoch.as_str())
            .ok_or_else(|| CoreError::new("session-unknown", "session is not owned by this pump"))
    }
    pub fn get_run(&self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.session(&key.session_epoch)?.get_run(key)
    }
    pub fn get_start(&self, epoch: &SessionEpoch, start: DecimalId) -> Result<RunKey, CoreError> {
        self.session(epoch)?.get_start(start)
    }
    /// Read the plan-state revision without driving clock or worker events.
    pub fn session_revision(&self, epoch: &SessionEpoch) -> Result<u64, CoreError> {
        Ok(self.session(epoch)?.revision())
    }
    pub fn session_has_pending(&self, epoch: &SessionEpoch) -> Result<bool, CoreError> {
        Ok(self.session(epoch)?.has_pending())
    }
    pub fn session_control_state(
        &self,
        epoch: &SessionEpoch,
    ) -> Result<super::SessionControlState, CoreError> {
        Ok(self.session(epoch)?.control_state())
    }
    pub fn session_latest_input_revision(
        &self,
        epoch: &SessionEpoch,
    ) -> Result<Option<u64>, CoreError> {
        Ok(self.session(epoch)?.latest_input_revision())
    }
    pub fn plan_version(&self, epoch: &SessionEpoch) -> Result<DecimalId, CoreError> {
        Ok(self.session(epoch)?.plan_version())
    }
    pub fn start(
        &mut self,
        epoch: &SessionEpoch,
        request: StartRequest,
    ) -> Result<StartReceipt, CoreError> {
        self.refresh()?;
        let admitted = loop {
            let guard = self
                .clock
                .as_ref()
                .map(|clock| super::clock::AdmissionClock {
                    clock: clock.clone(),
                    minimum: self.now,
                    before: self.next_deadline(),
                });
            let result = {
                let ledger = self.sessions.get_mut(epoch.as_str()).ok_or_else(|| {
                    CoreError::new("session-unknown", "session is not owned by this pump")
                })?;
                let mut results: Vec<_> = self
                    .runs
                    .values_mut()
                    .filter(|run| &run.graph.key().session_epoch == epoch)
                    .map(|run| &mut run.graph.results)
                    .collect();
                self.queue
                    .admit_clocked(ledger, request.clone(), &mut results, guard.as_ref())
            };
            match result {
                Err(error) if error.code == "clock-retry" => self.refresh()?,
                result => break result?,
            }
        };
        if let Some(graph) = admitted.graph {
            let ledger = self
                .sessions
                .get_mut(epoch.as_str())
                .expect("owned session");
            self.now = self.now.max(ledger.now());
            ledger.mark_running(graph.key())?;
            self.tracked.insert(graph.key().run_id, graph.key().clone());
            self.runs.insert(
                graph.key().run_id,
                Run {
                    graph,
                    observers: admitted
                        .observers
                        .into_iter()
                        .map(|(node, handle)| {
                            (
                                node,
                                Observer {
                                    handle,
                                    frame: None,
                                    wake_at: None,
                                    timer_due: false,
                                },
                            )
                        })
                        .collect(),
                },
            );
            self.close_inactive()?;
            // Reservation can advance the clock beyond the preflight snapshot.
            // Its guard proves no older deadline is due at this acceptance point.
            if self.clock.is_some() {
                self.clock_step(self.now, false)?;
            }
            self.refresh()?;
            for event in admitted.events {
                self.apply(event)?;
            }
            self.drive()?;
        }
        Ok(admitted.receipt)
    }
    pub fn cancel(&mut self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.refresh()?;
        // Validate the full key before touching physical work.
        self.get_run(key)?;
        if let Some(run) = self.runs.get_mut(&key.run_id) {
            let ledger = self
                .sessions
                .get_mut(key.session_epoch.as_str())
                .expect("owned session");
            run.graph.finish(ledger, TerminalReason::Cancelled)?;
            self.runs.remove(&key.run_id);
            self.queue.cancel_run(key, self.now)?;
        }
        self.sync_counts()?;
        self.get_run(key)
    }
    pub fn prepare(&mut self, epoch: &SessionEpoch, plan: ExecutionPlan) -> Result<(), CoreError> {
        self.refresh()?;
        self.sessions
            .get_mut(epoch.as_str())
            .ok_or_else(|| CoreError::new("session-unknown", "session is not owned by this pump"))?
            .prepare(plan)
    }
    fn change(
        &mut self,
        epoch: &SessionEpoch,
        operation: impl FnOnce(
            &mut RunLedger,
            &mut [&mut super::ExecutionResults],
        ) -> Result<bool, CoreError>,
    ) -> Result<bool, CoreError> {
        self.refresh()?;
        let ledger = self.sessions.get_mut(epoch.as_str()).ok_or_else(|| {
            CoreError::new("session-unknown", "session is not owned by this pump")
        })?;
        let mut results: Vec<_> = self
            .runs
            .values_mut()
            .filter(|run| &run.graph.key().session_epoch == epoch)
            .map(|run| &mut run.graph.results)
            .collect();
        let changed = operation(ledger, &mut results)?;
        drop(results);
        self.close_inactive()?;
        self.sync_counts()?;
        Ok(changed)
    }
    pub fn activate(&mut self, epoch: &SessionEpoch) -> Result<bool, CoreError> {
        self.change(epoch, RunLedger::activate_collected)
    }
    pub fn rollback(&mut self, epoch: &SessionEpoch) -> Result<bool, CoreError> {
        self.change(epoch, RunLedger::rollback_collected)
    }
    pub fn invalidate_input(
        &mut self,
        epoch: &SessionEpoch,
        expected: DecimalId,
        revision: u64,
    ) -> Result<(), CoreError> {
        self.change(epoch, |ledger, results| {
            ledger
                .invalidate_input_collected(expected, revision, results)
                .map(|_| true)
        })
        .map(|_| ())
    }
    fn close_inactive(&mut self) -> Result<(), CoreError> {
        let closed: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, run)| {
                !self.sessions[run.graph.key().session_epoch.as_str()].active_key(run.graph.key())
            })
            .map(|(id, _)| *id)
            .collect();
        for id in closed {
            let mut run = self.runs.remove(&id).expect("known run");
            self.queue.cancel_run(run.graph.key(), self.now)?;
            run.graph.close_work();
            // Dropping this run also removes every observer and its timer.
        }
        self.prune_prepared()?;
        Ok(())
    }
    fn prune_prepared(&mut self) -> Result<(), CoreError> {
        let mut keep = VecDeque::new();
        while let Some(pending) = self.pending_observations.pop_front() {
            let Some(run) = self.runs.get_mut(&pending.run) else {
                continue;
            };
            if run.graph.phase(&pending.node) != Some(super::GraphNodePhase::Active) {
                continue;
            }
            if run.graph.prepared_observation_current(&pending.prepared)? {
                keep.push_back(pending);
            } else {
                run.observers.insert(pending.node, pending.observer);
            }
        }
        self.pending_observations = keep;
        Ok(())
    }
    fn finish_ready(&mut self, id: DecimalId) -> Result<bool, CoreError> {
        if !self
            .runs
            .get(&id)
            .is_some_and(|run| run.graph.is_complete())
        {
            return Ok(false);
        }
        let mut run = self.runs.remove(&id).expect("complete run");
        let key = run.graph.key().clone();
        run.graph.finish(
            self.sessions
                .get_mut(key.session_epoch.as_str())
                .expect("owned session"),
            TerminalReason::Completed,
        )?;
        self.queue.cancel_run(&key, self.now)?;
        self.prune_prepared()?;
        Ok(true)
    }
    fn apply(&mut self, event: DispatchEvent) -> Result<(), CoreError> {
        let key = event.stamp().key.clone();
        if let Some(session) = self.sessions.get_mut(key.session_epoch.as_str()) {
            session.next_event_seq()?;
        }
        if let Some(run) = self.runs.get_mut(&key.run_id) {
            let released = event.apply(&mut run.graph)?;
            self.queue.cancel_released(&key, &released, self.now)?;
            for node in released {
                run.observers.remove(&node.node_id);
            }
        }
        self.prune_prepared()?;
        // Genuine late events still reclaim ownership inside DispatchQueue,
        // even after the graph/retained result has gone away.
        Ok(())
    }
    fn drive(&mut self) -> Result<(), CoreError> {
        if self.driving {
            self.policy_revision = self
                .policy_revision
                .checked_add(1)
                .ok_or_else(|| CoreError::new("id-exhausted", "policy event revision exhausted"))?;
            self.policy_pending = true;
            return Ok(());
        }
        self.driving = true;
        let result = self.drive_turns();
        self.driving = false;
        result
    }
    fn commit_observer(&mut self, pending: PendingObservation) -> Result<(), CoreError> {
        let PendingObservation {
            run: id,
            node,
            mut observer,
            prepared,
            ..
        } = pending;
        let Some(run) = self.runs.get_mut(&id) else {
            return Ok(());
        };
        if run.graph.phase(&node) != Some(super::GraphNodePhase::Active) {
            return Ok(());
        }
        let key = run.graph.key().clone();
        // A prepared Frame can wait behind newer deadline events. Its adoption
        // is a new linearization point, separate from that earlier observation.
        self.sessions
            .get_mut(key.session_epoch.as_str())
            .expect("owned session")
            .next_event_seq()?;
        let observation =
            match run
                .graph
                .commit_observation(&mut observer.handle, prepared, self.now)
            {
                Ok(observation) => observation,
                Err(error) if error.code == "observation-stale" => {
                    run.observers.insert(node, observer);
                    return Ok(());
                }
                Err(error) => {
                    run.observers.insert(node, observer);
                    return Err(error);
                }
            };
        match observation {
            GraphObservation::Wait { wake_at } => {
                observer.wake_at = wake_at;
                run.observers.insert(node, observer);
            }
            GraphObservation::Finished { released } => {
                self.queue.cancel_released(&key, &released, self.now)?;
                for node in released {
                    run.observers.remove(&node.node_id);
                }
            }
        }
        self.prune_prepared()?;
        self.finish_ready(id)?;
        Ok(())
    }
    fn drive_turns(&mut self) -> Result<(), CoreError> {
        loop {
            self.policy_pending = false;
            let mut changed = false;
            let mut deferred = false;
            let ids: Vec<_> = self.runs.keys().copied().collect();
            for id in ids {
                if self.finish_ready(id)? {
                    changed = true;
                    continue;
                }
                let Some(run) = self.runs.get_mut(&id) else {
                    continue;
                };
                let events = self.queue.schedule_ready(&mut run.graph, self.now)?;
                changed |= !events.is_empty();
                if !self
                    .runs
                    .values()
                    .any(|run| run.observers.values().any(|observer| observer.timer_due))
                {
                    self.refresh()?;
                }
                for event in events {
                    self.apply(event)?;
                }
                let Some(run) = self.runs.get(&id) else {
                    continue;
                };
                let nodes = run.graph.observer_nodes();
                for node in nodes {
                    let Some(run) = self.runs.get_mut(&id) else {
                        break;
                    };
                    // Rebuild after earlier controls in this turn resolved inputs.
                    let Some(frame) = run.graph.observer_frame(&node)? else {
                        continue;
                    };
                    // An earlier observer in this turn can discard this one.
                    let Some(observer) = run.observers.get(&node) else {
                        continue;
                    };
                    let reason = match &observer.frame {
                        None => Some(WakeReason::Started),
                        Some(old) if old != &frame => Some(WakeReason::Input),
                        _ if observer.timer_due => Some(WakeReason::Timer),
                        _ => None,
                    };
                    let Some(reason) = reason else {
                        continue;
                    };
                    // Keep the in-flight handle outside the map. A clock recheck
                    // may drive other observers, but cannot invoke this one twice.
                    let mut observer = run.observers.remove(&node).expect("known observer");
                    let key = run.graph.key().clone();
                    let seq = self
                        .sessions
                        .get_mut(key.session_epoch.as_str())
                        .expect("owned session")
                        .next_event_seq()?;
                    observer.frame = Some(frame);
                    observer.wake_at = None;
                    observer.timer_due = false;
                    let before_policy = self.policy_revision;
                    let prepared = match run.graph.prepare_observation(
                        &node,
                        &mut observer.handle,
                        reason,
                        seq,
                        self.now,
                    ) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            run.observers.insert(node, observer);
                            return Err(error);
                        }
                    };
                    self.refresh()?;
                    let pending = PendingObservation {
                        run: id,
                        node,
                        observer,
                        prepared,
                        priority: self.policy_revision == before_policy
                            && reason != WakeReason::Started,
                    };
                    if self.policy_pending || !self.pending_observations.is_empty() {
                        if pending.priority {
                            let index = self
                                .pending_observations
                                .iter()
                                .position(|pending| !pending.priority)
                                .unwrap_or(self.pending_observations.len());
                            self.pending_observations.insert(index, pending);
                        } else {
                            self.pending_observations.push_back(pending);
                        }
                        deferred = true;
                    } else {
                        self.commit_observer(pending)?;
                    }
                    changed = true;
                }
                changed |= self.finish_ready(id)?;
            }
            let events = self.queue.dispatch_queued(self.now)?;
            changed |= !events.is_empty();
            let retry = self.queue.take_clock_retry();
            self.refresh()?;
            changed |= retry;
            for event in events {
                self.apply(event)?;
            }
            if !deferred
                && !self.policy_pending
                && let Some(mut pending) = self.pending_observations.pop_front()
            {
                self.refresh()?;
                if self.policy_pending {
                    pending.priority = false;
                    let index = self
                        .pending_observations
                        .iter()
                        .position(|pending| !pending.priority)
                        .unwrap_or(self.pending_observations.len());
                    self.pending_observations.insert(index, pending);
                } else {
                    self.commit_observer(pending)?;
                }
                changed = true;
            }
            if !changed && !self.policy_pending && self.pending_observations.is_empty() {
                self.flush_grace()?;
                break;
            }
        }
        self.sync_counts()
    }
    fn sync_counts(&mut self) -> Result<(), CoreError> {
        let mut remove = vec![];
        for (id, key) in &self.tracked {
            let counts = self.queue.counts(key);
            if let Some(ledger) = self.sessions.get_mut(key.session_epoch.as_str()) {
                if !self.runs.contains_key(id) && counts == super::PhysicalCounts::default() {
                    ledger.trace_physical_ended(key);
                }
                if let Ok(snapshot) = ledger.get_run(key)
                    && snapshot.physical != counts
                {
                    ledger.update_physical(key, counts)?;
                }
            } else {
                self.host
                    .retention
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .update_closed_physical(key, counts);
            }
            if !self.runs.contains_key(id) && counts == super::PhysicalCounts::default() {
                remove.push(*id);
            }
        }
        for id in remove {
            self.tracked.remove(&id);
        }
        Ok(())
    }
    pub fn next_deadline(&self) -> Option<Duration> {
        self.queue
            .next_deadline()
            .into_iter()
            .chain(self.queue.next_grace_deadline().filter(|deadline| {
                !(self.driving
                    && self
                        .pending_grace
                        .is_some_and(|pending| *deadline <= pending))
            }))
            .chain(
                self.sessions
                    .values()
                    .filter_map(RunLedger::next_run_deadline),
            )
            .chain(self.runs.values().flat_map(|run| {
                run.observers
                    .values()
                    .filter_map(|observer| observer.wake_at)
            }))
            .chain(
                self.host
                    .retention
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .next_deadline(&self.host),
            )
            .min()
    }
    fn clock_step(&mut self, now: Duration, policy: bool) -> Result<(), CoreError> {
        self.now = now;
        // Queue failures are recorded before run expiry; policy is deferred.
        let events = self.queue.expire_queue(now)?;
        for event in events {
            self.apply(event)?;
        }
        for (epoch, ledger) in &mut self.sessions {
            #[cfg(test)]
            let expiring: Vec<_> = ledger
                .active_keys()
                .into_iter()
                .filter(|key| {
                    ledger
                        .run_deadline(key)
                        .is_some_and(|deadline| deadline <= now)
                })
                .collect();
            let mut results: Vec<_> = self
                .runs
                .values_mut()
                .filter(|run| run.graph.key().session_epoch.as_str() == epoch)
                .map(|run| &mut run.graph.results)
                .collect();
            ledger.advance_clock_with_results(now, &mut results)?;
            #[cfg(test)]
            for key in expiring {
                if ledger.get_run(&key).is_ok_and(|snapshot| {
                    snapshot
                        .result
                        .is_some_and(|result| result.reason == TerminalReason::Expired)
                }) {
                    self.test_notices.push((now, "run-expired"));
                }
            }
        }
        self.host
            .retention
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .advance(&self.host, now);
        self.close_inactive()?;
        let events = self.queue.expire_jobs(now)?;
        for event in events {
            self.apply(event)?;
        }
        for observer in self
            .runs
            .values_mut()
            .flat_map(|run| run.observers.values_mut())
        {
            if observer.wake_at.is_some_and(|at| at <= now) {
                observer.wake_at = None;
                observer.timer_due = true;
            }
        }
        let nested = self.driving;
        if policy {
            self.drive()?;
        }
        if nested {
            if let Some(deadline) = self
                .queue
                .next_grace_deadline()
                .filter(|deadline| *deadline <= self.now)
            {
                self.pending_grace = Some(deadline);
            }
        } else {
            self.flush_grace()?;
        }
        self.sync_counts()
    }
    fn flush_grace(&mut self) -> Result<(), CoreError> {
        self.pending_grace = None;
        #[cfg(test)]
        let before: BTreeMap<_, _> = self
            .tracked
            .iter()
            .map(|(id, key)| (*id, self.queue.counts(key).stalled))
            .collect();
        self.queue.mark_stalled(self.now)?;
        #[cfg(test)]
        for (id, key) in &self.tracked {
            let after = self.queue.counts(key).stalled;
            for _ in 0..after.saturating_sub(before[id]) {
                self.test_notices.push((self.now, "stalled"));
            }
        }
        self.sync_counts()
    }
    /// Advance all due internal deadlines chronologically before adopting any
    /// pending completion. Callbacks see exact deadline times during clock jumps.
    pub fn advance_to(&mut self, now: Duration) -> Result<(), CoreError> {
        if self.clock.is_some() {
            return Err(CoreError::new(
                "invalid-clock",
                "clocked pumps use tick; advance the controlled clock itself",
            ));
        }
        self.advance_deadlines(now, true)?;
        self.drain_completions()
    }
    fn advance_deadlines(&mut self, now: Duration, final_policy: bool) -> Result<(), CoreError> {
        if now < self.now {
            return Err(CoreError::new(
                "invalid-clock",
                "pump clock moved backwards",
            ));
        }
        for ledger in self.sessions.values() {
            ledger.validate_clock(now)?;
        }
        loop {
            // Equal-time observer work precedes grace. Before a later time is
            // processed, any deferred grace at the earlier time must be observed.
            if self.pending_grace.is_some_and(|deadline| deadline < now) {
                self.flush_grace()?;
            }
            let Some(next) = self.next_deadline().filter(|next| *next <= now) else {
                break;
            };
            if next < self.now {
                return Err(CoreError::new(
                    "invalid-clock",
                    "pending deadline precedes processed time",
                ));
            }
            self.clock_step(next, true)?;
        }
        if self.now < now || (self.now == now && final_policy) {
            self.clock_step(now, final_policy)?;
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<(), CoreError> {
        let Some(clock) = self.clock.clone() else {
            return Ok(());
        };
        let target = clock.now()?;
        self.advance_deadlines(target, false)
    }
    /// Clock-owner entry. State getters never call this or drain worker events.
    pub fn tick(&mut self) -> Result<(), CoreError> {
        if self.clock.is_none() {
            return Err(CoreError::new(
                "invalid-clock",
                "tick requires an attached clock",
            ));
        }
        self.refresh()?;
        self.drive()?;
        self.drain_completions()
    }
    fn drain_completions(&mut self) -> Result<(), CoreError> {
        while let Some(prepared) = self.queue.prepare_event()? {
            let key = prepared.completion().stamp().key.clone();
            let validated = match self.runs.get(&key.run_id) {
                Some(run) => run
                    .graph
                    .prepare_completion(prepared.completion().clone())?,
                None => None,
            };
            self.refresh()?;
            let event = self.queue.commit_event(prepared)?;
            if let Some(session) = self.sessions.get_mut(key.session_epoch.as_str()) {
                session.next_event_seq()?;
            }
            if event
                .completion()
                .is_some_and(|completion| completion.execution().is_some())
                && let Some(validated) = validated
                && let Some(run) = self.runs.get_mut(&key.run_id)
            {
                let released = run.graph.commit_completion(validated)?;
                self.queue.cancel_released(&key, &released, self.now)?;
                for node in released {
                    run.observers.remove(&node.node_id);
                }
            }
            self.finish_ready(key.run_id)?;
            self.prune_prepared()?;
            self.drive()?;
        }
        self.sync_counts()
    }
}

#[cfg(test)]
mod clock_fixture_tests {
    use super::*;
    use crate::execution_control::{
        CancellationToken, CompletionPermit, DeferredNode, OwnedInvocation, ResourcePolicy,
    };
    use crate::{
        graph::Graph,
        node::{NodeDefinition, Port, Registry},
    };
    use serde_json::{Value, json};
    use std::{sync::mpsc, time::Instant};
    struct Held(mpsc::Sender<CompletionPermit>);
    impl DeferredNode for Held {
        fn definition(&self) -> NodeDefinition {
            NodeDefinition::new("fixture.clock-held", 1, vec![], vec![Port::packet("out")])
        }
        fn start(
            &self,
            _: OwnedInvocation,
            permit: CompletionPermit,
            _: CancellationToken,
        ) -> Result<(), CoreError> {
            self.0.send(permit).map_err(|_| {
                CoreError::new("test-producer-dropped", "clock fixture producer dropped")
            })
        }
    }
    #[test]
    fn formal_run_deadline_and_grace_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let ids: Vec<String> =
            serde_json::from_str(include_str!("../../../../tests/execution_clock_cases.json"))
                .unwrap();
        assert_eq!(ids, ["T07-clock-jump-processes-deadline-and-grace"]);
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == ids[0])
            .unwrap();
        let initial = &case["initial"];
        assert_eq!(initial["workload"], "single");
        let spec = &fixture["workloads"]["single"];
        let node = spec["jobs"][0].as_str().unwrap();
        let host = ExecutionHost::new(
            "formal-clock-t07",
            ResourcePolicy::from_value(
                fixture["resourceProfiles"][initial["resourceProfile"].as_str().unwrap()].clone(),
            )
            .unwrap(),
        )
        .unwrap();
        let (sender, held) = mpsc::channel();
        let mut registry = Registry::new();
        registry.register_deferred(Held(sender)).unwrap();
        let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":node,"kind":"fixture.clock-held","version":1}],"edges":[]}).to_string()).unwrap();
        let epoch = SessionEpoch::new(initial["sessionEpoch"].as_str().unwrap()).unwrap();
        let version: DecimalId = initial["planVersion"].as_str().unwrap().parse().unwrap();
        let mut pump = ExecutionPump::new(host.clone()).unwrap();
        pump.open_session(
            host.clone(),
            epoch.clone(),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            version,
        )
        .unwrap();
        pump.invalidate_input(
            &epoch,
            version,
            initial["latestInputRevision"].as_u64().unwrap(),
        )
        .unwrap();
        let events = case["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        let start = &events[0];
        assert_eq!(start["op"], "start");
        assert_eq!(start["atMs"], 0);
        let targets: Vec<_> = spec["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| {
                let (node, port) = target.as_str().unwrap().split_once('.').unwrap();
                json!({"node":node,"port":port})
            })
            .collect();
        let request:StartRequest=serde_json::from_value(json!({"startKey":start["startKey"],"expectedPlanVersion":initial["planVersion"],"targets":targets,"input":start["input"],"runBudgetMs":start["runBudgetMs"],"jobBudgets":start["jobBudgets"]})).unwrap();
        let key = pump.start(&epoch, request).unwrap().key;
        // Establish a genuinely started producer before the single clock jump.
        let permit = held.recv_timeout(Duration::from_secs(5)).unwrap();
        pump.advance_to(Duration::ZERO).unwrap();
        assert!(pump.test_notices.is_empty());
        let advance = &events[1];
        assert_eq!(advance["op"], "advance-clock");
        pump.advance_to(Duration::from_millis(advance["atMs"].as_u64().unwrap()))
            .unwrap();
        let snapshot = pump.get_run(&key).unwrap();
        let wire = serde_json::to_value(&snapshot).unwrap();
        let notices: Vec<_> = pump
            .test_notices
            .iter()
            .map(|(at, code)| json!({"atMs":at.as_millis() as u64,"code":code}))
            .collect();
        for (field, want) in case["expected"].as_object().unwrap() {
            match field.as_str() {
                "r1" => {
                    for (field, want) in want.as_object().unwrap() {
                        match field.as_str() {
                            "state" => assert_eq!(&wire["state"], want),
                            "reason" | "delivery" => assert_eq!(&wire["result"][field], want),
                            other => panic!("unsupported clock run expectation {other}"),
                        }
                    }
                }
                "physicalJobs" => assert_eq!(json!(host.usage().physical_jobs), *want),
                "stalledJobs" => assert_eq!(
                    json!(
                        (0..snapshot.physical.stalled)
                            .map(|_| format!("r1/{node}"))
                            .collect::<Vec<_>>()
                    ),
                    *want
                ),
                "notices" => assert_eq!(json!(notices), *want),
                other => panic!("unsupported clock expectation {other}"),
            }
        }
        assert!(permit.finish_cancelled());
        let end = Instant::now() + Duration::from_secs(5);
        while host.usage().physical_jobs != 0 {
            pump.advance_to(pump.now()).unwrap();
            assert!(Instant::now() < end, "clock fixture physical cleanup");
            std::thread::sleep(Duration::from_millis(1));
        }
        let reclaimed = pump.get_run(&key).unwrap();
        assert_eq!(
            reclaimed.physical,
            crate::execution_control::PhysicalCounts::default()
        );
        assert_eq!(reclaimed.elapsed_ms, snapshot.elapsed_ms);
        assert_eq!(
            serde_json::to_value(reclaimed.result).unwrap(),
            serde_json::to_value(snapshot.result).unwrap()
        );
    }
}
