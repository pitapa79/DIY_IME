//! One background control owner per Host. Request callbacks are private fixed Core
//! operations; Node work remains on WorkerDispatcher's bounded workers.
use super::{
    CompletionWake, CompletionWakeStatus, DecimalId, ExecutionClock, ExecutionHost, ExecutionPump,
    ResourcePolicy, RunKey, RunSnapshot, SessionControlState, SessionEpoch, StartReceipt,
    StartRequest,
};
use crate::{graph::ExecutionPlan, node::CoreError};
use std::{
    collections::BTreeMap,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    time::Duration,
};

pub const MAX_SERVICE_SESSIONS: usize = 4;
pub const MAX_PENDING_SERVICE_REQUESTS: usize = 16;
const MAX_WAIT: Duration = Duration::from_secs(60);
struct Lifecycle {
    owners: usize,
    closing: bool,
}
struct RequestPermit(Arc<AtomicUsize>);
impl Drop for RequestPermit {
    fn drop(&mut self) {
        let before = self.0.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(before > 0);
    }
}
struct SessionToken {
    trace: Arc<super::trace::TraceSession>,
    epoch: SessionEpoch,
    wake: CompletionWake,
    issued_start: AtomicU64,
}
impl Drop for SessionToken {
    fn drop(&mut self) {
        self.trace.clear();
        self.wake.notify();
    }
}
struct Command {
    operation: Box<dyn FnOnce(&mut Actor) + Send>,
    _permit: RequestPermit,
    // Keeps a session alive through an abandoned caller's already-sent request.
    _scope: Option<Arc<SessionToken>>,
}
pub(crate) struct Shared {
    diagnostics: Arc<super::trace::TraceRegistry>,
    sender: SyncSender<Command>,
    wake: CompletionWake,
    clock: ExecutionClock,
    lifecycle: Mutex<Lifecycle>,
    state_changed: Condvar,
    fault: Mutex<Option<CoreError>>,
    requests: Arc<AtomicUsize>,
    stopped: AtomicBool,
    domain: String,
    policy: ResourcePolicy,
}
impl Shared {
    fn unavailable(&self) -> CoreError {
        self.fault
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| CoreError::new("service-closed", "execution service is closing"))
    }
    fn call<T: Send + 'static>(
        &self,
        scope: Option<Arc<SessionToken>>,
        operation: impl FnOnce(&mut Actor) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        if self
            .lifecycle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closing
        {
            return Err(self.unavailable());
        }
        self.requests
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_PENDING_SERVICE_REQUESTS).then_some(count + 1)
            })
            .map_err(|_| CoreError::new("busy", "execution service has 16 pending requests"))?;
        let permit = RequestPermit(Arc::clone(&self.requests));
        let (reply, response) = mpsc::channel();
        let command = Command {
            operation: Box::new(move |actor| {
                let result = operation(actor);
                let _ = reply.send(result);
            }),
            _permit: permit,
            _scope: scope,
        };
        match self.sender.try_send(command) {
            Ok(()) => self.wake.notify(),
            Err(TrySendError::Full(_)) => {
                return Err(CoreError::new(
                    "busy",
                    "execution service command queue is full",
                ));
            }
            Err(TrySendError::Disconnected(_)) => return Err(self.unavailable()),
        }
        match response.recv() {
            Ok(result) => result,
            Err(_) => {
                // The panicking operation can drop its reply before the outer
                // control-owner guard records the fault. Wait for that record.
                let mut life = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
                while !life.closing {
                    life = self
                        .state_changed
                        .wait(life)
                        .unwrap_or_else(|p| p.into_inner());
                }
                drop(life);
                Err(self.unavailable())
            }
        }
    }
}
pub struct ExecutionService {
    shared: Arc<Shared>,
}
impl fmt::Debug for ExecutionService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionService")
            .field("resource_domain", &self.shared.domain)
            .finish_non_exhaustive()
    }
}
impl Clone for ExecutionService {
    fn clone(&self) -> Self {
        self.shared
            .lifecycle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .owners += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}
impl Drop for ExecutionService {
    fn drop(&mut self) {
        let mut life = self
            .shared
            .lifecycle
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        life.owners = life.owners.checked_sub(1).expect("service owner balance");
        drop(life);
        self.shared.wake.notify();
    }
}
impl ExecutionService {
    pub fn new(host: ExecutionHost) -> Result<Self, CoreError> {
        Self::create(host, None)
    }
    pub fn with_clock(host: ExecutionHost, clock: ExecutionClock) -> Result<Self, CoreError> {
        Self::create(host, Some(clock))
    }
    fn create(host: ExecutionHost, clock: Option<ExecutionClock>) -> Result<Self, CoreError> {
        let slot = Arc::clone(&host.service);
        let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(shared) = slot.upgrade() {
            let mut life = shared.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
            if life.closing {
                return Err(shared.unavailable());
            }
            if clock
                .as_ref()
                .is_some_and(|clock| !clock.same_clock(&shared.clock))
            {
                return Err(CoreError::new(
                    "clock-conflict",
                    "this Host already owns another service clock",
                ));
            }
            life.owners += 1;
            drop(life);
            return Ok(Self { shared });
        }
        if host.usage().physical_jobs != 0 {
            return Err(CoreError::new(
                "service-unavailable",
                "old physical work has no live control owner",
            ));
        }
        let clock_slot = Arc::clone(&host.service_clock);
        let mut clock_slot = clock_slot.lock().unwrap_or_else(|p| p.into_inner());
        let clock = match (&*clock_slot, clock) {
            (Some(existing), Some(requested)) if !existing.same_clock(&requested) => {
                return Err(CoreError::new(
                    "clock-conflict",
                    "this Host keeps its original service clock",
                ));
            }
            (Some(existing), _) => existing.clone(),
            (None, Some(clock)) => clock,
            (None, None) => ExecutionClock::native(
                host.retention
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .now(),
            )?,
        };
        let pump = ExecutionPump::with_clock(host.clone(), clock.clone())?;
        let wake = pump.wake_signal();
        let (sender, receiver) = mpsc::sync_channel(MAX_PENDING_SERVICE_REQUESTS);
        let shared = Arc::new(Shared {
            diagnostics: host.diagnostics.clone(),
            sender,
            wake,
            clock: clock.clone(),
            lifecycle: Mutex::new(Lifecycle {
                owners: 1,
                closing: false,
            }),
            state_changed: Condvar::new(),
            fault: Mutex::new(None),
            requests: Arc::new(AtomicUsize::new(0)),
            stopped: AtomicBool::new(false),
            domain: host.resource_domain().into(),
            policy: host.policy().clone(),
        });
        let actor = Actor {
            host,
            pump,
            receiver,
            shared: Arc::clone(&shared),
            sessions: BTreeMap::new(),
            unready: BTreeMap::new(),
            clock_revision: clock.revision(),
            #[cfg(test)]
            before_receive: None,
        };
        std::thread::Builder::new()
            .name("diy-ime-control".into())
            .spawn(move || run_actor(actor))
            .map_err(|error| CoreError::new("service-unavailable", error.to_string()))?;
        *clock_slot = Some(clock);
        *slot = Arc::downgrade(&shared);
        Ok(Self { shared })
    }
    pub fn same_service(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
    pub fn clock(&self) -> ExecutionClock {
        self.shared.clock.clone()
    }
    pub fn resource_domain(&self) -> &str {
        &self.shared.domain
    }
    pub fn resource_policy(&self) -> &ResourcePolicy {
        &self.shared.policy
    }
    /// Open a session without an active or prepared plan. No run is admitted.
    pub fn open_empty(&self) -> Result<ExecutionController, CoreError> {
        self.open(None)
    }
    /// Ready-plan Core entry, preserving the existing revision-zero API.
    pub fn open_ready(
        &self,
        plan: ExecutionPlan,
        version: DecimalId,
    ) -> Result<ExecutionController, CoreError> {
        self.open(Some((plan, version)))
    }
    fn open(
        &self,
        ready: Option<(ExecutionPlan, DecimalId)>,
    ) -> Result<ExecutionController, CoreError> {
        let epoch = super::epoch::generate()?;
        let token = Arc::new(SessionToken {
            trace: self.shared.diagnostics.session(&epoch),
            epoch,
            wake: self.shared.wake.clone(),
            issued_start: AtomicU64::new(0),
        });
        let epoch = token.epoch.clone();
        let weak = Arc::downgrade(&token);
        self.shared.call(Some(Arc::clone(&token)), move |actor| {
            if actor.sessions.len() >= MAX_SERVICE_SESSIONS {
                return Err(CoreError::new("busy", "execution service has 4 sessions"));
            }
            if actor.sessions.contains_key(&epoch) {
                return Err(CoreError::new(
                    "session-conflict",
                    "session epoch already exists",
                ));
            }
            if let Some((plan, version)) = ready {
                actor
                    .pump
                    .open_session(actor.host.clone(), epoch.clone(), plan, version)?;
            } else {
                actor.unready.insert(
                    epoch.clone(),
                    Unready {
                        pending: None,
                        revision: 0,
                    },
                );
            }
            actor.sessions.insert(epoch, weak);
            Ok(())
        })?;
        Ok(ExecutionController {
            shared: Arc::clone(&self.shared),
            token,
        })
    }
}
#[derive(Clone)]
pub struct ExecutionController {
    shared: Arc<Shared>,
    token: Arc<SessionToken>,
}
impl fmt::Debug for ExecutionController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionController")
            .field("session_epoch", &self.token.epoch)
            .finish_non_exhaustive()
    }
}
impl ExecutionController {
    pub fn trace_is_recording(&self) -> bool {
        self.token.trace.is_recording()
    }
    /// Trusted Host only: call after successfully flushing a current usable response.
    /// This is Host publication, never browser rendering or TSF delivery.
    pub fn trace_response_published(&self, key: &RunKey) {
        if key.session_epoch == self.token.epoch {
            self.token.trace.response_published(key);
        }
    }

    pub fn start_trace(&self) -> Result<(), CoreError> {
        self.token.trace.start()
    }
    /// Explicit opt-in; adds trusted Host intervals to the basic diagnostics.
    pub fn start_detailed_trace(&self) -> Result<(), CoreError> {
        self.token.trace.start_detailed()
    }
    pub fn stop_trace(&self) {
        self.token.trace.stop();
    }
    pub fn clear_trace(&self) {
        self.token.trace.clear();
    }
    pub fn read_trace(&self, after: u64, limit: usize) -> Result<super::TraceSnapshot, CoreError> {
        self.token.trace.read(after, limit)
    }

    pub fn session_epoch(&self) -> &SessionEpoch {
        &self.token.epoch
    }
    /// An issuance counter, distinct from the Ledger's accepted maximum. Store
    /// the issued key with its payload for explicit deduplicated retries.
    pub fn next_start_key(&self) -> Result<DecimalId, CoreError> {
        let before = self
            .token
            .issued_start
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| CoreError::new("id-exhausted", "start key exhausted"))?;
        DecimalId::new(before + 1)
    }
    fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Actor) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        self.shared.call(Some(Arc::clone(&self.token)), operation)
    }
    fn check_key(&self, key: &RunKey) -> Result<(), CoreError> {
        key.validate()?;
        if key.session_epoch != self.token.epoch {
            return Err(CoreError::new(
                "session-conflict",
                "key belongs to another controller epoch",
            ));
        }
        Ok(())
    }
    pub fn start_run(&self, request: StartRequest) -> Result<StartReceipt, CoreError> {
        self.token
            .issued_start
            .fetch_max(request.start_key.get(), Ordering::AcqRel);
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            actor.require_ready(&epoch)?;
            actor.pump.start(&epoch, request)
        })
    }
    pub fn get_run(&self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.check_key(key)?;
        let key = key.clone();
        self.call(move |actor| {
            if actor.unready.contains_key(&key.session_epoch) {
                return Err(CoreError::new("run-unknown", "session has no accepted run"));
            }
            actor.pump.get_run_at(&key, actor.shared.clock.now()?)
        })
    }
    pub fn get_start(&self, start: DecimalId) -> Result<RunKey, CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            if actor.unready.contains_key(&epoch) {
                return Err(CoreError::new(
                    "start-not-seen",
                    "session has no accepted start",
                ));
            }
            actor.pump.get_start(&epoch, start)
        })
    }
    pub fn cancel_run(&self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.check_key(key)?;
        let key = key.clone();
        self.call(move |actor| {
            if actor.unready.contains_key(&key.session_epoch) {
                return Err(CoreError::new("run-unknown", "session has no accepted run"));
            }
            actor.pump.cancel(&key)
        })
    }
    pub fn invalidate_input(&self, expected: DecimalId, revision: u64) -> Result<(), CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            actor.require_ready(&epoch)?;
            actor.pump.invalidate_input(&epoch, expected, revision)
        })
    }
    pub fn prepare(&self, plan: ExecutionPlan) -> Result<(), CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            if let Some(unready) = actor.unready.get_mut(&epoch) {
                let revision = unready.next_revision()?;
                unready.pending = Some(Arc::new(plan));
                unready.revision = revision;
                Ok(())
            } else {
                actor.pump.prepare(&epoch, plan)
            }
        })
    }
    pub fn activate(&self) -> Result<bool, CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            if let Some(unready) = actor.unready.get(&epoch) {
                let Some(plan) = unready.pending.clone() else {
                    return Ok(false);
                };
                let revision = unready.next_revision()?;
                actor.pump.open_session_shared(
                    actor.host.clone(),
                    epoch.clone(),
                    plan,
                    DecimalId::new(1)?,
                    revision,
                )?;
                actor.unready.remove(&epoch);
                Ok(true)
            } else {
                actor.pump.activate(&epoch)
            }
        })
    }
    pub fn rollback(&self) -> Result<bool, CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            if actor.unready.contains_key(&epoch) {
                Ok(false)
            } else {
                actor.pump.rollback(&epoch)
            }
        })
    }
    pub fn plan_version(&self) -> Result<DecimalId, CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| {
            actor.require_ready(&epoch)?;
            actor.pump.plan_version(&epoch)
        })
    }
    /// Read one consistent view; never drives the pump or invokes a Node.
    pub fn control_state(&self) -> Result<SessionControlState, CoreError> {
        let epoch = self.token.epoch.clone();
        self.call(move |actor| match actor.unready.get(&epoch) {
            Some(unready) => Ok(SessionControlState {
                revision: unready.revision,
                active_plan_version: None,
                has_prepared: unready.pending.is_some(),
                can_rollback: false,
                latest_input_revision: None,
                delivery_generation: DecimalId::new(1)?,
            }),
            None => actor.pump.session_control_state(&epoch),
        })
    }
}
struct Unready {
    pending: Option<Arc<ExecutionPlan>>,
    revision: u64,
}
impl Unready {
    fn next_revision(&self) -> Result<u64, CoreError> {
        self.revision
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "revision exhausted"))
    }
}
struct Actor {
    host: ExecutionHost,
    pump: ExecutionPump,
    receiver: Receiver<Command>,
    shared: Arc<Shared>,
    sessions: BTreeMap<SessionEpoch, Weak<SessionToken>>,
    unready: BTreeMap<SessionEpoch, Unready>,
    clock_revision: u64,
    #[cfg(test)]
    before_receive: Option<(mpsc::Sender<()>, Receiver<()>)>,
}
impl Actor {
    fn require_ready(&self, epoch: &SessionEpoch) -> Result<(), CoreError> {
        if self.unready.contains_key(epoch) {
            Err(CoreError::new(
                "no-active-plan",
                "session has no active plan",
            ))
        } else {
            Ok(())
        }
    }
    fn close_dead(&mut self) -> Result<(), CoreError> {
        let closed: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, token)| token.strong_count() == 0)
            .map(|(epoch, _)| epoch.clone())
            .collect();
        for epoch in closed {
            self.pump.close_session(&epoch)?;
            self.unready.remove(&epoch);
            self.sessions.remove(&epoch);
        }
        Ok(())
    }
    fn independent_events(&mut self) -> Result<(), CoreError> {
        let now = self.shared.clock.now()?;
        if self.pump.has_pending_events()
            || self.shared.clock.revision() != self.clock_revision
            || self
                .pump
                .next_deadline()
                .is_some_and(|deadline| deadline <= now)
        {
            self.pump.tick()?;
            self.clock_revision = self.shared.clock.revision();
        }
        Ok(())
    }
    fn can_stop(&self) -> bool {
        let used = self.host.usage();
        let mut life = self
            .shared
            .lifecycle
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if life.owners == 0
            && self.sessions.is_empty()
            && self.shared.requests.load(Ordering::Acquire) == 0
            && used.physical_jobs == 0
            && used.queued_jobs == 0
            && used.observers == 0
            && used.active_runs == 0
            && used.retained_runs == 0
        {
            life.closing = true;
            true
        } else {
            false
        }
    }
    fn run(&mut self) -> Result<(), CoreError> {
        loop {
            self.close_dead()?;
            self.independent_events()?;
            for _ in 0..MAX_PENDING_SERVICE_REQUESTS {
                self.close_dead()?;
                self.independent_events()?;
                #[cfg(test)]
                if let Some((entered, gate)) = self.before_receive.take() {
                    entered.send(()).unwrap();
                    gate.recv().unwrap();
                }
                match self.receiver.try_recv() {
                    Ok(command) => {
                        // A clock advance or completion can arrive after the
                        // pre-receive check but before this command was sent.
                        // Catch up after receipt so the command cannot overtake it.
                        self.independent_events()?;
                        let Command {
                            operation,
                            _permit,
                            _scope,
                        } = command;
                        operation(self);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        return Err(CoreError::new(
                            "service-closed",
                            "command receiver disconnected",
                        ));
                    }
                }
            }
            self.close_dead()?;
            self.independent_events()?;
            if self.can_stop() {
                return Ok(());
            }
            let now = self.shared.clock.now()?;
            if self
                .pump
                .next_deadline()
                .is_some_and(|deadline| deadline <= now)
            {
                continue;
            }
            let timeout = wait_timeout(&self.shared.clock, self.pump.next_deadline(), now);
            if self.shared.wake.wait(timeout)? == CompletionWakeStatus::Closed {
                return Err(CoreError::new(
                    "service-closed",
                    "control wake closed unexpectedly",
                ));
            }
        }
    }
}
fn run_actor(mut actor: Actor) {
    let shared = Arc::clone(&actor.shared);
    let error = match catch_unwind(AssertUnwindSafe(|| actor.run())) {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some(CoreError::new("service-fault", "control owner panicked")),
    };
    if let Some(error) = error {
        *shared.fault.lock().unwrap_or_else(|p| p.into_inner()) = Some(CoreError::new(
            "service-fault",
            format!("{}: {}", error.code, error.message),
        ));
        shared
            .lifecycle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closing = true;
    }
    shared.state_changed.notify_all();
    drop(actor);
    shared.stopped.store(true, Ordering::Release);
    shared.state_changed.notify_all();
}
fn wait_timeout(
    clock: &ExecutionClock,
    deadline: Option<Duration>,
    now: Duration,
) -> Option<Duration> {
    if clock.wall_time() {
        deadline.map(|deadline| deadline.saturating_sub(now).min(MAX_WAIT))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::time::Instant;
    fn setup(name: &str) -> (ExecutionHost, ExecutionClock, ExecutionService) {
        let mut profile = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!(
                "../../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        profile.retention_ms = super::super::Milliseconds::new(10).unwrap();
        let host = ExecutionHost::new(name, profile).unwrap();
        let clock = ExecutionClock::controlled(Duration::ZERO);
        let service = ExecutionService::with_clock(host.clone(), clock.clone()).unwrap();
        (host, clock, service)
    }
    fn plan() -> ExecutionPlan {
        let mut registry = crate::node::Registry::new();
        crate::builtins::register_demo_nodes(&mut registry).unwrap();
        let graph=crate::graph::Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"ok"}}}}],"edges":[]}).to_string()).unwrap();
        ExecutionPlan::compile(&graph, &registry).unwrap()
    }
    fn until(mut done: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < end, "service ownership did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    struct Release(Option<mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(send) = self.0.take() {
                let _ = send.send(());
            }
        }
    }
    #[test]
    fn clock_advance_before_command_send_is_processed_before_that_command() {
        let (_, clock, service) = setup("service-receive-clock-order");
        let (entered, waiting) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let guard = Release(Some(release));
        service
            .shared
            .call(None, move |actor| {
                actor.before_receive = Some((entered, gate));
                Ok(())
            })
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        clock.advance_to(Duration::from_millis(8)).unwrap();
        let (reply, result) = mpsc::channel();
        // Send while the control owner is gated after its pre-receive check.
        // The advance therefore happens before this command is received.
        service.shared.requests.fetch_add(1, Ordering::AcqRel);
        service
            .shared
            .sender
            .try_send(Command {
                operation: Box::new(move |actor| {
                    reply.send(actor.pump.now()).unwrap();
                }),
                _permit: RequestPermit(Arc::clone(&service.shared.requests)),
                _scope: None,
            })
            .unwrap();
        drop(guard);
        let processed = result.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(processed, Duration::from_millis(8));
    }
    #[test]
    fn pending_request_limit_includes_the_currently_executing_request() {
        let (_, _, service) = setup("service-request-bound");
        let shared = Arc::clone(&service.shared);
        let (entered, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let guard = Release(Some(release));
        let first = Arc::clone(&shared);
        let blocked = std::thread::spawn(move || {
            first.call(None, move |_| {
                entered.send(()).unwrap();
                gate.recv().unwrap();
                Ok(())
            })
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut calls = vec![];
        for _ in 0..15 {
            let shared = Arc::clone(&shared);
            calls.push(std::thread::spawn(move || shared.call(None, |_| Ok(()))));
        }
        until(|| shared.requests.load(Ordering::Acquire) == 16);
        assert_eq!(shared.call(None, |_| Ok(())).unwrap_err().code, "busy");
        assert_eq!(shared.requests.load(Ordering::Acquire), 16);
        drop(guard);
        blocked.join().unwrap().unwrap();
        for call in calls {
            call.join().unwrap().unwrap();
        }
        until(|| shared.requests.load(Ordering::Acquire) == 0);
        drop(service);
        until(|| shared.stopped.load(Ordering::Acquire));
    }
    #[test]
    fn a_closed_idle_service_does_not_form_a_host_or_clock_reference_cycle() {
        let (host, clock, service) = setup("service-no-cycle");
        let weak = Arc::downgrade(&service.shared);
        drop(service);
        until(|| weak.upgrade().is_none());
        clock.advance_to(Duration::from_millis(1)).unwrap();
        let queue = super::super::DispatchQueue::new(host).unwrap();
        drop(queue);
    }
    #[test]
    fn reopening_an_idle_host_preserves_its_original_clock() {
        let (host, clock, service) = setup("service-clock-survives-idle");
        let weak = Arc::downgrade(&service.shared);
        drop(service);
        until(|| weak.upgrade().is_none());
        clock.advance_to(Duration::from_millis(7)).unwrap();
        let reopened = ExecutionService::new(host.clone()).unwrap();
        assert!(reopened.clock().same_clock(&clock));
        assert_eq!(reopened.clock().now().unwrap(), Duration::from_millis(7));
        assert_eq!(
            ExecutionService::with_clock(
                host,
                ExecutionClock::controlled(Duration::from_millis(7))
            )
            .unwrap_err()
            .code,
            "clock-conflict"
        );
    }
    #[test]
    fn the_owner_survives_until_closed_sessions_retention_expires_without_a_ledger() {
        let (host, clock, service) = setup("service-cache-lifetime");
        let controller = service
            .open_ready(plan(), DecimalId::new(1).unwrap())
            .unwrap();
        let request=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}]})).unwrap();
        let key = controller.start_run(request).unwrap().key;
        until(|| host.usage().physical_jobs == 0);
        assert!(controller.get_run(&key).unwrap().result.is_some());
        let weak = Arc::downgrade(&service.shared);
        drop(controller);
        drop(service);
        until(|| host.usage().active_runs == 0);
        assert_eq!(host.usage().retained_runs, 1);
        assert!(weak.upgrade().is_some());
        clock.advance_to(Duration::from_millis(10)).unwrap();
        until(|| weak.upgrade().is_none());
        assert_eq!(host.usage().retained_runs, 0);
    }
    #[test]
    fn internal_faults_are_connection_faults_and_do_not_silently_restart() {
        let (host, clock, service) = setup("service-fault-boundary");
        let shared = Arc::clone(&service.shared);
        assert_eq!(
            shared
                .call::<()>(None, |_| panic!("test private Core fault"))
                .unwrap_err()
                .code,
            "service-fault"
        );
        until(|| shared.stopped.load(Ordering::Acquire));
        assert_eq!(
            ExecutionService::with_clock(host.clone(), clock.clone())
                .unwrap_err()
                .code,
            "service-fault"
        );
        assert_eq!(shared.requests.load(Ordering::Acquire), 0);
        drop(service);
        drop(shared);
        let retry = ExecutionService::with_clock(host, clock).unwrap();
        drop(retry);
    }
    #[test]
    fn start_key_issuance_reaches_u64_max_without_wrapping_or_reuse() {
        let (_, _, service) = setup("service-start-counter");
        let controller = service
            .open_ready(plan(), DecimalId::new(1).unwrap())
            .unwrap();
        controller
            .token
            .issued_start
            .store(u64::MAX - 1, Ordering::Release);
        assert_eq!(controller.next_start_key().unwrap().get(), u64::MAX);
        assert_eq!(
            controller.clone().next_start_key().unwrap_err().code,
            "id-exhausted"
        );
        assert_eq!(
            controller.token.issued_start.load(Ordering::Acquire),
            u64::MAX
        );
    }
    #[test]
    fn native_waits_are_split_without_changing_the_original_processing_deadline() {
        let native = ExecutionClock::native(Duration::ZERO).unwrap();
        assert_eq!(
            wait_timeout(&native, Some(Duration::MAX), Duration::ZERO),
            Some(MAX_WAIT)
        );
        assert_eq!(
            wait_timeout(
                &native,
                Some(Duration::from_millis(5)),
                Duration::from_millis(2)
            ),
            Some(Duration::from_millis(3))
        );
        assert_eq!(wait_timeout(&native, None, Duration::ZERO), None);
        let controlled = ExecutionClock::controlled(Duration::ZERO);
        assert_eq!(
            wait_timeout(&controlled, Some(Duration::from_millis(5)), Duration::ZERO),
            None
        );
    }
    #[test]
    fn empty_prepare_replacement_and_close_release_the_actual_plan_arcs() {
        let (_, _, service) = setup("empty-plan-lifetime");
        let controller = service.open_empty().unwrap();
        controller.prepare(plan()).unwrap();
        let epoch = controller.session_epoch().clone();
        let lookup = epoch.clone();
        let old = controller
            .call(move |actor| {
                Ok(Arc::downgrade(
                    actor.unready[&lookup].pending.as_ref().unwrap(),
                ))
            })
            .unwrap();
        controller.prepare(plan()).unwrap();
        assert!(old.upgrade().is_none());
        let lookup = epoch.clone();
        let pending = controller
            .call(move |actor| {
                Ok(Arc::downgrade(
                    actor.unready[&lookup].pending.as_ref().unwrap(),
                ))
            })
            .unwrap();
        assert_eq!(controller.control_state().unwrap().revision, 2);
        let clone = controller.clone();
        drop(controller);
        assert!(pending.upgrade().is_some());
        assert!(clone.activate().unwrap());
        let state = clone.control_state().unwrap();
        assert_eq!(state.revision, 3);
        assert!(!state.can_rollback);
        assert!(pending.upgrade().is_some());
        drop(clone);
        until(|| pending.upgrade().is_none());
        let unready = service.open_empty().unwrap();
        unready.prepare(plan()).unwrap();
        let lookup = unready.session_epoch().clone();
        let pending = unready
            .call(move |actor| {
                Ok(Arc::downgrade(
                    actor.unready[&lookup].pending.as_ref().unwrap(),
                ))
            })
            .unwrap();
        drop(unready);
        until(|| pending.upgrade().is_none());
    }
    #[test]
    fn exhausted_empty_revision_rejects_prepare_and_activation_without_losing_the_plan() {
        let (_, _, service) = setup("empty-revision-exhausted");
        let controller = service.open_empty().unwrap();
        controller.prepare(plan()).unwrap();
        let epoch = controller.session_epoch().clone();
        let pending = controller
            .call(move |actor| {
                let unready = actor.unready.get_mut(&epoch).unwrap();
                unready.revision = u64::MAX;
                Ok(Arc::downgrade(unready.pending.as_ref().unwrap()))
            })
            .unwrap();
        let before = controller.control_state().unwrap();
        assert_eq!(controller.prepare(plan()).unwrap_err().code, "id-exhausted");
        assert_eq!(controller.activate().unwrap_err().code, "id-exhausted");
        assert_eq!(controller.control_state().unwrap(), before);
        assert!(pending.upgrade().is_some());
        let epoch = controller.session_epoch().clone();
        controller
            .call(move |actor| {
                assert_eq!(
                    actor.pump.session_control_state(&epoch).unwrap_err().code,
                    "session-unknown"
                );
                actor.unready.get_mut(&epoch).unwrap().revision = u64::MAX - 1;
                Ok(())
            })
            .unwrap();
        assert!(controller.activate().unwrap());
        assert_eq!(controller.control_state().unwrap().revision, u64::MAX);
    }
}
