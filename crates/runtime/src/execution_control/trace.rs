//! Opt-in, session-owned metadata diagnostics. Never execution authority.
use super::{ExecutionClock, RunKey, SessionEpoch};
use crate::node::CoreError;
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::Duration,
};
const MAX_EVENTS: usize = 20_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_RUNS: usize = 200;
const MAX_CALLS: usize = 20_000;
const RETENTION_US: u64 = 60_000_000;
pub(crate) const MAX_WORKER_INTERVALS: usize = 12;
const WORKER_QUEUE_SLOTS: usize = 128;
// Reserve conservative storage for each bounded native metadata bundle from
// the existing diagnostic budget, rather than growing it with concurrency.
const WORKER_QUEUE_BYTES: usize = WORKER_QUEUE_SLOTS * 4096;
pub(crate) type QueuedWorkerInterval = (WorkerTracePhase, u64, Option<u64>);
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TraceKind {
    RunAccepted,
    DependencyWait,
    NodeReady,
    NodeStarted,
    NodeResolved,
    NodeSkipped,
    NodeFailed,
    NodeNoOutput,
    NodeDiscarded,
    ResourceWait,
    ResultSuperseded,
    ResultUnavailable,
    CallCancelled,
    CallStalled,
    CallEnded,
    CoreResultReady,
    HostResponsePublished,
    RunCompleted,
    NoOutput,
    RunCancelled,
    RunExpired,
    RunFailed,
    PhysicalEnded,
    HostRequestStarted,
    HostRequestFinished,
    WorkerStagingStarted,
    WorkerStagingFinished,
    WorkerProcessStarted,
    WorkerProcessFinished,
    WorkerCleanupStarted,
    WorkerCleanupFinished,
    BrokerExchangeStarted,
    BrokerExchangeFinished,
    WorkerInterval,
}
/// Fixed trusted-Worker phases; independent of guest SDK self-reporting.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum WorkerTracePhase {
    Main = 0,
    DllLoad = 1,
    ModuleRead = 2,
    InputRead = 3,
    EngineSetup = 4,
    Compile = 5,
    Instantiate = 6,
    GuestSetup = 7,
    GuestCall = 8,
    ReplyRead = 9,
    ReplyWrite = 10,
    Cleanup = 11,
}
impl TryFrom<u8> for WorkerTracePhase {
    type Error = ();
    fn try_from(n: u8) -> Result<Self, ()> {
        Ok(match n {
            0 => Self::Main,
            1 => Self::DllLoad,
            2 => Self::ModuleRead,
            3 => Self::InputRead,
            4 => Self::EngineSetup,
            5 => Self::Compile,
            6 => Self::Instantiate,
            7 => Self::GuestSetup,
            8 => Self::GuestCall,
            9 => Self::ReplyRead,
            10 => Self::ReplyWrite,
            11 => Self::Cleanup,
            _ => return Err(()),
        })
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTraceInterval {
    pub instance: u32,
    pub phase: WorkerTracePhase,
    pub started_us: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_us: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    pub sequence: String,
    pub at_us: String,
    pub run: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<u32>,
    pub plan: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<u32>,
    pub event: TraceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerTraceInterval>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSnapshot {
    pub coverage: &'static str,
    pub format_version: u32,
    pub recording_id: Option<String>,
    pub recording: bool,
    pub now_us: String,
    pub first_sequence: String,
    pub next_sequence: String,
    pub dropped: u64,
    pub events: Vec<TraceEvent>,
}
struct RunAlias {
    run: u32,
    input: Option<(u64, u32)>,
    plan: (u64, u32),
    calls: BTreeMap<u64, u32>,
    ended: bool,
    published: bool,
}
struct WorkerBundle {
    run: u64,
    received: Duration,
    node: usize,
    kind: Box<str>,
    version: u32,
    instance: u32,
    intervals: Box<[QueuedWorkerInterval]>,
}
struct WorkerSink {
    sender: SyncSender<WorkerBundle>,
    lost: AtomicU64,
}
struct WorkerInbox {
    sink: Arc<WorkerSink>,
    receiver: Receiver<WorkerBundle>,
}
impl WorkerInbox {
    fn new() -> Self {
        let (sender, receiver) = sync_channel(WORKER_QUEUE_SLOTS);
        Self {
            sink: Arc::new(WorkerSink {
                sender,
                lost: AtomicU64::new(0),
            }),
            receiver,
        }
    }
}
struct Recording {
    id: String,
    generation: u64,
    enabled: bool,
    origin: Duration,
    stopped: Option<Duration>,
    sequence: u64,
    next_run: u32,
    next_input: u32,
    next_plan: u32,
    next_call: u32,
    runs: BTreeMap<u64, RunAlias>,
    events: VecDeque<(TraceEvent, usize)>,
    bytes: usize,
    dropped: u64,
    calls: usize,
    worker_inbox: Option<WorkerInbox>,
}
impl Recording {
    fn byte_limit(&self) -> usize {
        MAX_BYTES - 4096 - usize::from(self.worker_inbox.is_some()) * WORKER_QUEUE_BYTES
    }
    fn drain_workers(&mut self, clock: &ExecutionClock) {
        // Continuous producers cannot make a Host event consume an unbounded
        // amount of diagnostic work while it owns the recorder.
        for _ in 0..WORKER_QUEUE_SLOTS {
            let Some(bundle) = self
                .worker_inbox
                .as_ref()
                .and_then(|inbox| inbox.receiver.try_recv().ok())
            else {
                break;
            };
            let expired = clock.now().map_or(true, |now| {
                now.saturating_sub(bundle.received).as_micros() > u128::from(RETENTION_US)
            });
            if !self.enabled || expired || !self.runs.contains_key(&bundle.run) {
                self.dropped = self.dropped.saturating_add(bundle.intervals.len() as u64);
                continue;
            }
            for &(phase, started, finished) in &bundle.intervals {
                // This Host timestamp is when the recorder consumes metadata,
                // never a reconstructed process/guest timestamp. Worker-local
                // durations remain exactly the original received evidence.
                append_event(
                    self,
                    clock,
                    bundle.run,
                    TraceKind::WorkerInterval,
                    Some((bundle.node, &bundle.kind, bundle.version)),
                    None,
                    Some(WorkerTraceInterval {
                        instance: bundle.instance,
                        phase,
                        started_us: started.to_string(),
                        finished_us: finished.map(|end| end.to_string()),
                    }),
                );
            }
        }
    }
    fn trim(&mut self, at: u64) {
        while self.events.front().is_some_and(|(e, _)| {
            e.at_us
                .parse::<u64>()
                .unwrap_or(0)
                .saturating_add(RETENTION_US)
                < at
        }) {
            self.evict();
        }
    }
    fn evict(&mut self) {
        if let Some((_, bytes)) = self.events.pop_front() {
            self.bytes -= bytes;
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}
pub(crate) struct TraceSession {
    clock: ExecutionClock,
    enabled: AtomicBool,
    detailed: AtomicBool,
    generation: AtomicU64,
    state: Mutex<Option<Recording>>,
    contention: AtomicU64,
    publication_loss_through: AtomicU64,
}
impl std::fmt::Debug for TraceSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TraceSession(metadata only)")
    }
}
pub(crate) struct TraceRegistry {
    clock: ExecutionClock,
    sessions: Mutex<BTreeMap<String, Weak<TraceSession>>>,
}
impl std::fmt::Debug for TraceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TraceRegistry(private scopes)")
    }
}
impl TraceRegistry {
    pub(crate) fn new() -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self {
            clock: ExecutionClock::native(Duration::ZERO)?,
            sessions: Mutex::new(BTreeMap::new()),
        }))
    }
    pub(crate) fn session(&self, epoch: &SessionEpoch) -> Arc<TraceSession> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        sessions.retain(|_, s| s.strong_count() > 0);
        if let Some(s) = sessions.get(epoch.as_str()).and_then(Weak::upgrade) {
            return s;
        }
        let s = TraceSession::with_clock(self.clock.clone());
        sessions.insert(epoch.as_str().into(), Arc::downgrade(&s));
        s
    }
}
impl TraceSession {
    fn with_clock(clock: ExecutionClock) -> Arc<Self> {
        Arc::new(Self {
            clock,
            enabled: AtomicBool::new(false),
            detailed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            state: Mutex::new(None),
            contention: AtomicU64::new(0),
            publication_loss_through: AtomicU64::new(0),
        })
    }
    pub(crate) fn start(&self) -> Result<(), CoreError> {
        self.start_mode(false)
    }
    pub(crate) fn start_detailed(&self) -> Result<(), CoreError> {
        self.start_mode(true)
    }
    fn start_mode(&self, detailed: bool) -> Result<(), CoreError> {
        let id = super::epoch::generate()?.as_str().to_owned();
        let origin = self.clock.now()?;
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| CoreError::new("trace-limit", "diagnostic generation exhausted"))?
            + 1;
        *state = Some(Recording {
            id,
            generation,
            enabled: true,
            origin,
            stopped: None,
            sequence: 0,
            next_run: 0,
            next_input: 0,
            next_plan: 0,
            next_call: 0,
            runs: BTreeMap::new(),
            events: VecDeque::new(),
            bytes: 0,
            dropped: 0,
            calls: 0,
            worker_inbox: detailed.then(WorkerInbox::new),
        });
        self.contention.store(0, Ordering::Release);
        self.publication_loss_through.store(0, Ordering::Release);
        self.detailed.store(detailed, Ordering::Release);
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }
    pub(crate) fn stop(&self) {
        self.enabled.store(false, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(r) = state.as_mut() {
            r.drain_workers(&self.clock);
            if let Some(inbox) = r.worker_inbox.take() {
                // A producer can have captured the old sink just before stop.
                // Disconnect it and snapshot its loss count so late producers
                // cannot mutate the frozen recording, even on queue overflow.
                r.dropped = r
                    .dropped
                    .saturating_add(inbox.sink.lost.load(Ordering::Relaxed));
            }
            r.enabled = false;
            r.stopped = self.clock.now().ok();
            self.enabled.store(false, Ordering::Release);
        }
    }
    pub(crate) fn clear(&self) {
        self.enabled.store(false, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        *state = None;
        self.enabled.store(false, Ordering::Release);
        self.contention.store(0, Ordering::Release);
    }
    pub(crate) fn accept(self: &Arc<Self>, key: &RunKey) -> Option<TraceTicket> {
        if !self.enabled.load(Ordering::Acquire) {
            return None;
        }
        let mut state = match self.state.try_lock() {
            Ok(s) => s,
            Err(_) => {
                self.contention.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        let r = state.as_mut().filter(|r| r.enabled)?;
        if r.runs.len() >= MAX_RUNS {
            if let Some(old) = r
                .runs
                .iter()
                .filter(|(_, a)| a.ended)
                .min_by_key(|(_, a)| a.run)
                .map(|(k, _)| *k)
            {
                let old = r.runs.remove(&old).expect("selected diagnostic run");
                r.calls -= old.calls.len();
                r.events.retain(|(e, n)| {
                    if e.run == old.run {
                        r.bytes -= *n;
                        r.dropped = r.dropped.saturating_add(1);
                        false
                    } else {
                        true
                    }
                });
            } else {
                r.dropped = r.dropped.saturating_add(1);
                return None;
            }
        }
        r.next_run = r.next_run.checked_add(1)?;
        let input = if let Some(revision) = key.input_revision {
            let found = r
                .runs
                .values()
                .find_map(|a| a.input.filter(|(v, _)| *v == revision));
            Some(match found {
                Some(pair) => pair,
                None => {
                    r.next_input = r.next_input.checked_add(1)?;
                    (revision, r.next_input)
                }
            })
        } else {
            None
        };
        let version = key.plan_version.get();
        let plan = if let Some(a) = r.runs.values().find(|a| a.plan.0 == version) {
            a.plan
        } else {
            r.next_plan = r.next_plan.checked_add(1)?;
            (version, r.next_plan)
        };
        r.runs.insert(
            key.run_id.get(),
            RunAlias {
                run: r.next_run,
                input,
                plan,
                calls: BTreeMap::new(),
                ended: false,
                published: false,
            },
        );
        let ticket = TraceTicket {
            session: Arc::downgrade(self),
            generation: r.generation,
            run: key.run_id.get(),
            worker_sink: r
                .worker_inbox
                .as_ref()
                .map(|inbox| Arc::downgrade(&inbox.sink)),
        };
        drop(state);
        ticket.emit(TraceKind::RunAccepted, None, None);
        Some(ticket)
    }
    pub(crate) fn is_recording(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub(crate) fn response_published(self: &Arc<Self>, key: &RunKey) {
        self.run_event(key, TraceKind::HostResponsePublished);
    }
    pub(crate) fn physical_ended(self: &Arc<Self>, key: &RunKey) {
        self.run_event(key, TraceKind::PhysicalEnded);
    }
    fn run_event(self: &Arc<Self>, key: &RunKey, event: TraceKind) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let state = match self.state.try_lock() {
            Ok(s) => s,
            Err(_) => {
                if event == TraceKind::HostResponsePublished {
                    self.publication_loss_through
                        .fetch_max(key.run_id.get(), Ordering::AcqRel);
                }
                self.contention.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        let Some(r) = state
            .as_ref()
            .filter(|r| r.enabled && r.runs.contains_key(&key.run_id.get()))
        else {
            return;
        };
        // Run IDs are monotonic. Never relabel a later retry as the first response
        // after losing an earlier publication; newer runs remain measurable.
        if event == TraceKind::HostResponsePublished
            && key.run_id.get() <= self.publication_loss_through.load(Ordering::Acquire)
        {
            return;
        }
        let t = TraceTicket {
            session: Arc::downgrade(self),
            generation: r.generation,
            run: key.run_id.get(),
            worker_sink: r
                .worker_inbox
                .as_ref()
                .map(|inbox| Arc::downgrade(&inbox.sink)),
        };
        drop(state);
        t.emit(event, None, None);
    }
    pub(crate) fn read(&self, after: u64, limit: usize) -> Result<TraceSnapshot, CoreError> {
        if limit == 0 || limit > 256 {
            return Err(CoreError::new(
                "trace-limit",
                "diagnostic page size must be 1 through 256",
            ));
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(r) = state.as_mut() else {
            return Ok(TraceSnapshot {
                coverage: "new-runs-only",
                format_version: 1,
                recording_id: None,
                recording: false,
                now_us: "0".into(),
                first_sequence: "0".into(),
                next_sequence: "0".into(),
                dropped: 0,
                events: vec![],
            });
        };
        r.drain_workers(&self.clock);
        let now = self.clock.now()?;
        let at = u64::try_from(
            r.stopped
                .unwrap_or(now)
                .saturating_sub(r.origin)
                .as_micros(),
        )
        .unwrap_or(u64::MAX);
        if r.enabled {
            r.trim(at);
        }
        let events = r
            .events
            .iter()
            .filter(|(e, _)| e.sequence.parse::<u64>().is_ok_and(|n| n > after))
            .take(limit)
            .map(|(e, _)| e.clone())
            .collect();
        Ok(TraceSnapshot {
            coverage: "new-runs-only",
            format_version: 1,
            recording_id: Some(r.id.clone()),
            recording: r.enabled,
            now_us: at.to_string(),
            first_sequence: r
                .events
                .front()
                .map_or_else(|| r.sequence.to_string(), |(e, _)| e.sequence.clone()),
            next_sequence: r.sequence.to_string(),
            dropped: r
                .dropped
                .saturating_add(self.contention.load(Ordering::Relaxed))
                .saturating_add(
                    r.worker_inbox
                        .as_ref()
                        .map_or(0, |inbox| inbox.sink.lost.load(Ordering::Relaxed)),
                ),
            events,
        })
    }
}
#[derive(Clone)]
pub(crate) struct TraceTicket {
    session: Weak<TraceSession>,
    generation: u64,
    run: u64,
    worker_sink: Option<Weak<WorkerSink>>,
}
impl std::fmt::Debug for TraceTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TraceTicket(private scope)")
    }
}
impl TraceTicket {
    pub(crate) fn is_detailed(&self) -> bool {
        self.session.upgrade().is_some_and(|s| {
            s.enabled.load(Ordering::Acquire)
                && s.detailed.load(Ordering::Acquire)
                && s.generation.load(Ordering::Acquire) == self.generation
        })
    }
    pub(crate) fn is_recording(&self) -> bool {
        self.session.upgrade().is_some_and(|s| {
            s.enabled.load(Ordering::Acquire)
                && s.generation.load(Ordering::Acquire) == self.generation
        })
    }
    pub(crate) fn emit(
        &self,
        event: TraceKind,
        node: Option<(usize, &str, u32)>,
        job: Option<u64>,
    ) {
        self.emit_data(event, node, job, None);
    }
    pub(crate) fn emit_worker(&self, node: (usize, &str, u32), worker: WorkerTraceInterval) {
        self.emit_data(TraceKind::WorkerInterval, Some(node), None, Some(worker));
    }
    pub(crate) fn queue_workers(
        &self,
        node: (usize, &str, u32),
        instance: u32,
        intervals: Box<[QueuedWorkerInterval]>,
    ) {
        if !self.is_detailed() {
            return;
        }
        if intervals.is_empty()
            || intervals.len() > MAX_WORKER_INTERVALS
            || instance == 0
            || instance > 20_000
            || intervals
                .iter()
                .any(|(_, start, end)| end.is_some_and(|end| end < *start))
        {
            self.lost_diagnostics();
            return;
        }
        let Some(registry) = self.session.upgrade() else {
            return;
        };
        let Some(sink) = self.worker_sink.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let Ok(received) = registry.clock.now() else {
            return;
        };
        let count = intervals.len() as u64;
        let bundle = WorkerBundle {
            run: self.run,
            received,
            node: node.0,
            kind: node
                .1
                .chars()
                .take(128)
                .collect::<String>()
                .into_boxed_str(),
            version: node.2,
            instance,
            intervals,
        };
        // Never acquire the recording mutex or wait for the consumer. A full
        // or disconnected queue loses diagnostics, not execution ownership.
        if sink.sender.try_send(bundle).is_err() {
            sink.lost.fetch_add(count, Ordering::Relaxed);
        }
    }
    pub(crate) fn lost_diagnostics(&self) {
        let Some(s) = self.session.upgrade() else {
            return;
        };
        if !self.is_detailed() {
            return;
        }
        match s.state.try_lock() {
            Ok(mut state) => {
                if let Some(r) = state
                    .as_mut()
                    .filter(|r| r.enabled && r.generation == self.generation)
                {
                    r.dropped = r.dropped.saturating_add(1);
                }
            }
            Err(_) => {
                s.contention.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    fn emit_data(
        &self,
        event: TraceKind,
        node: Option<(usize, &str, u32)>,
        job: Option<u64>,
        worker: Option<WorkerTraceInterval>,
    ) {
        let Some(registry) = self.session.upgrade() else {
            return;
        };
        if !registry.enabled.load(Ordering::Acquire) {
            return;
        }
        let mut sessions = match registry.state.try_lock() {
            Ok(s) => s,
            Err(_) => {
                if event == TraceKind::HostResponsePublished {
                    registry
                        .publication_loss_through
                        .fetch_max(self.run, Ordering::AcqRel);
                }
                registry.contention.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        let Some(r) = sessions
            .as_mut()
            .filter(|r| r.enabled && r.generation == self.generation)
        else {
            return;
        };
        // A bundle is queued before its invocation can complete. Adopt pending
        // native metadata before the next recorded Host event, preserving the
        // existing ordering before call-ended/resolved/publication whenever
        // those anchors are recorded. Lock acquisition remains try_lock only.
        r.drain_workers(&registry.clock);
        append_event(r, &registry.clock, self.run, event, node, job, worker);
    }
}
fn append_event(
    r: &mut Recording,
    clock: &ExecutionClock,
    run_key: u64,
    event: TraceKind,
    node: Option<(usize, &str, u32)>,
    job: Option<u64>,
    worker: Option<WorkerTraceInterval>,
) {
    let Some(alias) = r.runs.get_mut(&run_key) else {
        return;
    };
    if event == TraceKind::HostResponsePublished {
        if alias.published {
            return;
        }
        alias.published = true;
    }
    if event == TraceKind::PhysicalEnded {
        if alias.ended {
            return;
        }
        alias.ended = true;
    }
    let call = if let Some(job) = job {
        if let Some(c) = alias.calls.get(&job) {
            Some(*c)
        } else {
            if r.calls >= MAX_CALLS {
                r.dropped = r.dropped.saturating_add(1);
                return;
            }
            let Some(next) = r.next_call.checked_add(1) else {
                return;
            };
            r.next_call = next;
            r.calls += 1;
            alias.calls.insert(job, next);
            Some(next)
        }
    } else {
        None
    };
    let (run, input, plan) = (alias.run, alias.input.map(|(_, n)| n), alias.plan.1);
    let Ok(now) = clock.now() else {
        return;
    };
    let at = u64::try_from(now.saturating_sub(r.origin).as_micros()).unwrap_or(u64::MAX);
    r.trim(at);
    let Some(sequence) = r.sequence.checked_add(1) else {
        return;
    };
    r.sequence = sequence;
    let e = TraceEvent {
        sequence: sequence.to_string(),
        at_us: at.to_string(),
        run,
        input,
        plan,
        node: node.map(|(i, _, _)| format!("{plan}:{}", i + 1)),
        call,
        node_kind: node.map(|(_, kind, _)| kind.chars().take(128).collect()),
        node_version: node.map(|(_, _, v)| v),
        event,
        worker,
    };
    let Ok(bytes) = serde_json::to_vec(&e).map(|v| v.len() + 1) else {
        return;
    };
    while r.events.len() >= MAX_EVENTS || r.bytes.saturating_add(bytes) > r.byte_limit() {
        r.evict();
    }
    r.bytes += bytes;
    r.events.push_back((e, bytes));
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_control::DecimalId;
    fn setup() -> (ExecutionClock, Arc<TraceSession>) {
        let clock = ExecutionClock::controlled(Duration::ZERO);
        (clock.clone(), TraceSession::with_clock(clock))
    }
    fn key(n: u64) -> RunKey {
        RunKey {
            session_epoch: SessionEpoch::new("fedcba9876543210fedcba9876543210").unwrap(),
            plan_version: DecimalId::new(991).unwrap(),
            run_id: DecimalId::new(n).unwrap(),
            input_revision: Some(800_000),
            delivery_generation: DecimalId::new(777).unwrap(),
        }
    }
    #[test]
    fn disabled_path_never_waits_for_recording_lock() {
        let (_, s) = setup();
        let guard = s.state.lock().unwrap();
        assert!(s.accept(&key(9001)).is_none());
        drop(guard);
        assert!(s.read(0, 256).unwrap().events.is_empty());
    }
    #[test]
    fn clear_and_restart_reject_late_old_tickets() {
        let (_, s) = setup();
        s.start().unwrap();
        let old = s.accept(&key(9001)).unwrap();
        let first = s.read(0, 256).unwrap().recording_id;
        s.clear();
        old.emit(TraceKind::NodeStarted, None, None);
        assert!(s.read(0, 256).unwrap().recording_id.is_none());
        s.start().unwrap();
        s.accept(&key(9001)).unwrap();
        old.emit(TraceKind::CoreResultReady, None, None);
        let now = s.read(0, 256).unwrap();
        assert_ne!(first, now.recording_id);
        assert_eq!(now.events.len(), 1);
        assert_eq!(now.events[0].run, 1);
    }
    #[test]
    fn stop_freezes_elapsed_and_never_restarts_on_late_events() {
        let (c, s) = setup();
        s.start().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        c.advance_to(Duration::from_millis(11)).unwrap();
        s.stop();
        c.advance_to(Duration::from_secs(90)).unwrap();
        t.emit(TraceKind::CallEnded, None, None);
        let r = s.read(0, 256).unwrap();
        assert_eq!(r.now_us, "11000");
        assert!(!r.recording);
        assert_eq!(r.events.len(), 1);
    }
    #[test]
    fn aliases_do_not_export_raw_keys_and_preserve_input_identity() {
        let (_, s) = setup();
        s.start().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        t.emit(
            TraceKind::NodeStarted,
            Some((0, "declared.node", 1)),
            Some(994_433),
        );
        s.accept(&key(9002)).unwrap();
        let r = s.read(0, 256).unwrap();
        let mut j = serde_json::to_value(&r).unwrap();
        j.as_object_mut().unwrap().remove("recordingId");
        let j = j.to_string();
        for secret in [
            "fedcba9876543210fedcba9876543210",
            "9001",
            "800000",
            "994433",
            "deliveryGeneration",
        ] {
            assert!(!j.contains(secret), "leaked {secret}");
        }
        assert_eq!(r.events[0].input, r.events[2].input);
        assert_eq!(r.events[1].call, Some(1));
        assert_eq!(r.events[1].node.as_deref(), Some("1:1"));
    }
    #[test]
    fn contention_drops_only_diagnostics_without_blocking() {
        let (_, s) = setup();
        s.start().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let guard = s.state.lock().unwrap();
        t.emit(TraceKind::NodeStarted, None, None);
        drop(guard);
        let r = s.read(0, 256).unwrap();
        assert_eq!(r.dropped, 1);
        assert_eq!(r.events.len(), 1);
    }
    fn native_intervals() -> Box<[QueuedWorkerInterval]> {
        vec![
            (WorkerTracePhase::Main, 0, Some(100)),
            (WorkerTracePhase::GuestCall, 11, Some(32)),
        ]
        .into_boxed_slice()
    }
    #[test]
    fn native_metadata_producer_does_not_wait_for_the_recording_mutex() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let guard = s.state.lock().unwrap();
        let (done, observed) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            t.queue_workers((0, "fixture.wasm", 1), 1, native_intervals());
            let _ = done.send(());
        });
        let result = observed.recv_timeout(Duration::from_secs(2));
        drop(guard);
        producer.join().unwrap();
        result.expect("native metadata waited behind the execution-event recorder");
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        let intervals: Vec<_> = page
            .events
            .iter()
            .filter_map(|event| event.worker.as_ref())
            .collect();
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[1].started_us, "11");
        assert_eq!(intervals[1].finished_us.as_deref(), Some("32"));
    }
    #[test]
    fn native_metadata_queue_is_bounded_counts_loss_and_keeps_the_existing_budget() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        for _ in 0..WORKER_QUEUE_SLOTS + 2 {
            t.queue_workers((0, "fixture.wasm", 1), 1, native_intervals());
        }
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 4, "two bundles of two intervals were refused");
        let state = s.state.lock().unwrap();
        let r = state.as_ref().unwrap();
        assert_eq!(
            r.events
                .iter()
                .filter(|(event, _)| event.worker.is_some())
                .count(),
            2 * WORKER_QUEUE_SLOTS
        );
        assert!(r.bytes + WORKER_QUEUE_BYTES <= MAX_BYTES);
        assert!(
            std::mem::size_of::<WorkerBundle>()
                + 128 * 4
                + MAX_WORKER_INTERVALS * std::mem::size_of::<QueuedWorkerInterval>()
                < 4096
        );
    }
    #[test]
    fn captured_native_metadata_precedes_its_recorded_terminal_anchors() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        t.emit(
            TraceKind::NodeStarted,
            Some((0, "fixture.wasm", 1)),
            Some(1),
        );
        t.queue_workers((0, "fixture.wasm", 1), 1, native_intervals());
        t.emit(TraceKind::CallEnded, Some((0, "fixture.wasm", 1)), Some(1));
        t.emit(TraceKind::NodeResolved, Some((0, "fixture.wasm", 1)), None);
        t.emit(TraceKind::HostResponsePublished, None, None);
        let page = s.read(0, 256).unwrap();
        let ended = page
            .events
            .iter()
            .position(|event| event.event == TraceKind::CallEnded)
            .unwrap();
        assert_eq!(
            page.events[..ended]
                .iter()
                .filter(|event| event.worker.is_some())
                .count(),
            2
        );
        assert!(
            page.events[ended..]
                .iter()
                .all(|event| event.worker.is_none())
        );
        assert_eq!(page.dropped, 0);
    }
    #[test]
    fn cleared_generation_cannot_receive_old_native_metadata_or_its_loss_counter() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let old = s.accept(&key(9001)).unwrap();
        let retained = old.worker_sink.as_ref().unwrap().upgrade().unwrap();
        old.queue_workers((0, "old", 1), 1, native_intervals());
        s.clear();
        s.start_detailed().unwrap();
        let current = s.accept(&key(9001)).unwrap();
        old.queue_workers((0, "old", 1), 1, native_intervals());
        retained.lost.store(999, Ordering::Release);
        assert!(
            retained
                .sender
                .try_send(WorkerBundle {
                    run: 9001,
                    received: Duration::ZERO,
                    node: 0,
                    kind: "old".into(),
                    version: 1,
                    instance: 1,
                    intervals: native_intervals(),
                })
                .is_err()
        );
        current.queue_workers((0, "current", 1), 1, native_intervals());
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(
            page.events
                .iter()
                .filter(|event| event.worker.is_some())
                .count(),
            2
        );
        assert!(
            page.events
                .iter()
                .filter(|event| event.worker.is_some())
                .all(|event| event.node_kind.as_deref() == Some("current"))
        );
    }
    #[test]
    fn stopping_drains_captured_native_metadata_and_then_freezes_it() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let in_flight = t.worker_sink.as_ref().unwrap().upgrade().unwrap();
        t.queue_workers((0, "fixture.wasm", 1), 1, native_intervals());
        clock.advance_to(Duration::from_millis(10)).unwrap();
        s.stop();
        let before = serde_json::to_value(s.read(0, 256).unwrap()).unwrap();
        clock.advance_to(Duration::from_millis(20)).unwrap();
        t.queue_workers((0, "fixture.wasm", 1), 1, native_intervals());
        in_flight.lost.fetch_add(999, Ordering::Relaxed);
        assert!(
            in_flight
                .sender
                .try_send(WorkerBundle {
                    run: 9001,
                    received: Duration::ZERO,
                    node: 0,
                    kind: "late".into(),
                    version: 1,
                    instance: 1,
                    intervals: native_intervals(),
                })
                .is_err()
        );
        assert_eq!(
            before,
            serde_json::to_value(s.read(0, 256).unwrap()).unwrap()
        );
        assert_eq!(before["events"].as_array().unwrap().len(), 3);
    }
    #[test]
    fn native_metadata_expires_and_basic_recording_allocates_no_queue() {
        let (clock, s) = setup();
        s.start().unwrap();
        assert!(
            s.state
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .worker_inbox
                .is_none()
        );
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        t.queue_workers((0, "fixture.wasm", 1), 1, native_intervals());
        clock
            .advance_to(Duration::from_micros(RETENTION_US + 1))
            .unwrap();
        let page = s.read(0, 256).unwrap();
        assert!(page.events.iter().all(|event| event.worker.is_none()));
        assert!(page.dropped >= 2);
    }
    #[test]
    fn sessions_have_independent_queues_and_loss_counts() {
        let (_, a) = setup();
        let (_, b) = setup();
        a.start().unwrap();
        b.start().unwrap();
        let t = a.accept(&key(9001)).unwrap();
        let guard = a.state.lock().unwrap();
        t.emit(TraceKind::NodeStarted, None, None);
        assert!(b.accept(&key(9001)).is_some());
        drop(guard);
        assert_eq!(a.read(0, 256).unwrap().dropped, 1);
        assert_eq!(b.read(0, 256).unwrap().dropped, 0);
    }
    #[test]
    fn live_ring_limits_bytes_events_and_marks_truncation() {
        let (_, s) = setup();
        s.start().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let label = "\0".repeat(300);
        for _ in 0..MAX_EVENTS + 5 {
            t.emit(TraceKind::NodeStarted, Some((0, &label, 1)), Some(12));
        }
        let g = s.state.lock().unwrap();
        let r = g.as_ref().unwrap();
        assert!(r.events.len() <= MAX_EVENTS);
        assert!(r.bytes <= MAX_BYTES - 4096);
        assert!(r.dropped > 0);
        assert!(r.events.front().unwrap().0.sequence.parse::<u64>().unwrap() > 1);
        assert_eq!(
            r.events
                .back()
                .unwrap()
                .0
                .node_kind
                .as_ref()
                .unwrap()
                .chars()
                .count(),
            128
        );
    }
    #[test]
    fn completed_run_slots_roll_over_but_active_run_is_preserved() {
        let (_, s) = setup();
        s.start().unwrap();
        let active = s.accept(&key(9999)).unwrap();
        for n in 1..=(MAX_RUNS + 20) as u64 {
            let t = s.accept(&key(n)).unwrap();
            t.emit(TraceKind::PhysicalEnded, None, None);
        }
        active.emit(TraceKind::NodeStarted, None, None);
        let g = s.state.lock().unwrap();
        let r = g.as_ref().unwrap();
        assert_eq!(r.runs.len(), MAX_RUNS);
        assert!(r.runs.contains_key(&9999));
        assert!(r.dropped > 0);
        assert_eq!(r.events.back().unwrap().0.event, TraceKind::NodeStarted);
    }
    #[test]
    fn all_active_runs_reach_explicit_limit_instead_of_eviction() {
        let (_, s) = setup();
        s.start().unwrap();
        for n in 1..=MAX_RUNS as u64 {
            assert!(s.accept(&key(n)).is_some());
        }
        assert!(s.accept(&key(9001)).is_none());
        assert!(s.read(0, 256).unwrap().dropped > 0);
    }
    #[test]
    fn old_events_expire_and_pages_never_exceed_requested_size() {
        let (c, s) = setup();
        s.start().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        for _ in 0..300 {
            t.emit(TraceKind::NodeStarted, None, None);
        }
        assert_eq!(s.read(0, 17).unwrap().events.len(), 17);
        assert!(s.read(0, 257).is_err());
        assert!(s.read(0, 0).is_err());
        c.advance_to(Duration::from_secs(61)).unwrap();
        let r = s.read(0, 256).unwrap();
        assert!(r.events.is_empty());
        assert_eq!(r.dropped, 301);
    }
    #[test]
    fn outstanding_tickets_do_not_retain_disposed_session_memory() {
        let (_, s) = setup();
        s.start().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let weak = Arc::downgrade(&s);
        drop(s);
        assert!(weak.upgrade().is_none());
        t.emit(TraceKind::NodeStarted, None, None);
    }

    #[test]
    fn lost_first_publication_is_never_replaced_with_a_later_retry() {
        let (_, s) = setup();
        s.start().unwrap();
        let first = key(9001);
        s.accept(&first).unwrap();
        let guard = s.state.lock().unwrap();
        s.response_published(&first);
        drop(guard);
        s.response_published(&first);
        let next = key(9002);
        s.accept(&next).unwrap();
        s.response_published(&next);
        s.response_published(&next);
        let r = s.read(0, 256).unwrap();
        assert!(r.dropped > 0);
        let public: Vec<_> = r
            .events
            .iter()
            .filter(|e| e.event == TraceKind::HostResponsePublished)
            .collect();
        assert_eq!(public.len(), 1);
        assert_eq!(public[0].run, 2);
    }
    #[test]
    fn publication_can_follow_physical_end_but_is_emitted_once() {
        let (_, s) = setup();
        s.start().unwrap();
        let key = key(9001);
        s.accept(&key).unwrap();
        s.physical_ended(&key);
        s.response_published(&key);
        s.response_published(&key);
        let r = s.read(0, 256).unwrap();
        let kinds: Vec<_> = r.events.iter().map(|e| e.event).collect();
        assert_eq!(
            kinds,
            vec![
                TraceKind::RunAccepted,
                TraceKind::PhysicalEnded,
                TraceKind::HostResponsePublished
            ]
        );
    }
}
