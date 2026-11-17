//! Opt-in, session-owned metadata diagnostics. Never execution authority.
use super::{ExecutionClock, RunKey, SessionEpoch};
use crate::node::CoreError;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, RwLock, Weak,
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
// Escrow tree/Box overhead and 8-byte completion tags in the existing budget.
const EVENT_INDEX_BYTES: usize = MAX_EVENTS * 128;
static AUDIT_REPORTS: AtomicU64 = AtomicU64::new(0);
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
#[derive(Clone, Copy)]
struct Stamp {
    sequence: u64,
    at_us: u64,
}
struct RecordSignals {
    origin: Duration,
    probe: AtomicBool,
    closed: AtomicBool,
    issued: AtomicU64,
    visible: AtomicU64,
    completed: Box<[AtomicU64]>,
    lost: AtomicU64,
    publication_loss_through: AtomicU64,
    first_run: AtomicU64,
    audit: LossAudit,
}
#[derive(Default)]
struct LossAudit {
    probe_busy: AtomicU64,
    reservation_full: AtomicU64,
    clock_unavailable: AtomicU64,
    recorder_busy: AtomicU64,
    host_queue_full: AtomicU64,
    invalid_native: AtomicU64,
    unfinished: AtomicU64,
}
struct ProbeGuard<'a>(&'a AtomicBool);
impl Drop for ProbeGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl RecordSignals {
    fn new(origin: Duration) -> Result<Self, CoreError> {
        let mut completed = Vec::new();
        completed
            .try_reserve_exact(MAX_EVENTS)
            .map_err(|_| CoreError::new("trace-limit", "diagnostic index unavailable"))?;
        completed.extend((0..MAX_EVENTS).map(|_| AtomicU64::new(0)));
        Ok(Self {
            origin,
            probe: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            issued: AtomicU64::new(0),
            visible: AtomicU64::new(0),
            completed: completed.into_boxed_slice(),
            lost: AtomicU64::new(0),
            publication_loss_through: AtomicU64::new(0),
            first_run: AtomicU64::new(0),
            audit: LossAudit::default(),
        })
    }
    fn note_first_run(&self, run: u64) {
        // Zero alone is invalid as a RunKey. Preserve every positive64-bit ID
        // and handle reordered producers in at most two atomic operations.
        if self
            .first_run
            .compare_exchange(0, run, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.first_run.fetch_min(run, Ordering::AcqRel);
        }
    }
    fn capture(&self, clock: &ExecutionClock) -> Option<Stamp> {
        self.advance(16);
        // Clock read and integer reservation only: no allocation, OS wait,
        // yield, recorder serialization or unbounded retry on a producer.
        let mut acquired = false;
        for _ in 0..8 {
            if self
                .probe
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                acquired = true;
                break;
            }
        }
        if !acquired {
            self.audit.probe_busy.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let _guard = ProbeGuard(&self.probe);
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let visible = self.visible.load(Ordering::Acquire);
        let issued = self.issued.load(Ordering::Acquire);
        if issued.saturating_sub(visible) >= MAX_EVENTS as u64 {
            self.audit.reservation_full.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let sequence = issued.checked_add(1)?;
        let at_us = match clock
            .now()
            .ok()
            .and_then(|now| u64::try_from(now.saturating_sub(self.origin).as_micros()).ok())
        {
            Some(at) => at,
            None => {
                self.audit.clock_unavailable.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        self.issued.store(sequence, Ordering::Release);
        Some(Stamp { sequence, at_us })
    }
    fn advance(&self, limit: usize) {
        for _ in 0..limit {
            let old = self.visible.load(Ordering::Acquire);
            let Some(next) = old.checked_add(1) else {
                break;
            };
            if self.completed[(next % MAX_EVENTS as u64) as usize].load(Ordering::Acquire) != next {
                break;
            }
            let _ = self
                .visible
                .compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire);
        }
    }
    fn complete(&self, sequence: u64) {
        self.completed[(sequence % MAX_EVENTS as u64) as usize].store(sequence, Ordering::Release);
        self.advance(16);
    }
    fn lose(&self, event: TraceKind, run: u64) {
        self.lost.fetch_add(1, Ordering::Relaxed);
        if event == TraceKind::HostResponsePublished {
            self.publication_loss_through
                .fetch_max(run, Ordering::AcqRel);
        }
    }
    fn freeze(&self) {
        self.closed.store(true, Ordering::Release);
        let end = self.issued.load(Ordering::Acquire);
        let begin = self.visible.load(Ordering::Acquire);
        for sequence in begin.saturating_add(1)..=end {
            if self.completed[(sequence % MAX_EVENTS as u64) as usize].load(Ordering::Acquire)
                != sequence
            {
                self.lost.fetch_add(1, Ordering::Relaxed);
                self.audit.unfinished.fetch_add(1, Ordering::Relaxed);
                self.completed[(sequence % MAX_EVENTS as u64) as usize]
                    .store(sequence, Ordering::Release);
            }
        }
        self.advance(MAX_EVENTS);
    }
}
struct CompleteStamp {
    signals: Arc<RecordSignals>,
    sequence: u64,
}
impl Drop for CompleteStamp {
    fn drop(&mut self) {
        self.signals.complete(self.sequence);
    }
}
struct HostBundle {
    generation: u64,
    run: u64,
    stamp: Stamp,
    event: TraceKind,
    node: Option<(usize, Box<str>, u32)>,
    job: Option<u64>,
}
struct AcceptedBundle {
    generation: u64,
    run: u64,
    input_revision: Option<u64>,
    plan_version: u64,
    stamp: Stamp,
}
enum ReceivedMetadata {
    Worker(WorkerBundle),
    Host(HostBundle),
    Accepted(AcceptedBundle),
}
struct WorkerSink {
    sender: SyncSender<ReceivedMetadata>,
    lost: AtomicU64,
}
struct WorkerInbox {
    sink: Arc<WorkerSink>,
    receiver: Receiver<ReceivedMetadata>,
}
struct HostRoute {
    generation: u64,
    sink: Option<Weak<WorkerSink>>,
    signals: Weak<RecordSignals>,
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
    // Box the old event payload: unused tree slots contain pointers, not full
    // TraceEvents. The fixed index reservation covers tree/Box overhead.
    events: BTreeMap<u64, Box<(TraceEvent, usize)>>,
    signals: Arc<RecordSignals>,
    bytes: usize,
    dropped: u64,
    calls: usize,
    worker_inbox: Option<WorkerInbox>,
}
impl Recording {
    fn accept_alias(&mut self, run: u64, input_revision: Option<u64>, version: u64) -> Option<()> {
        if self.runs.len() >= MAX_RUNS {
            if let Some(old) = self
                .runs
                .iter()
                .filter(|(_, a)| a.ended)
                .min_by_key(|(_, a)| a.run)
                .map(|(k, _)| *k)
            {
                let old = self.runs.remove(&old).expect("selected diagnostic run");
                self.calls -= old.calls.len();
                self.events.retain(|_, entry| {
                    let (e, n) = entry.as_ref();
                    if e.run == old.run {
                        self.bytes -= *n;
                        self.dropped = self.dropped.saturating_add(1);
                        false
                    } else {
                        true
                    }
                });
            } else {
                self.dropped = self.dropped.saturating_add(1);
                return None;
            }
        }
        self.next_run = self.next_run.checked_add(1)?;
        self.signals.note_first_run(run);
        let input = if let Some(revision) = input_revision {
            let found = self
                .runs
                .values()
                .find_map(|a| a.input.filter(|(v, _)| *v == revision));
            Some(match found {
                Some(pair) => pair,
                None => {
                    self.next_input = self.next_input.checked_add(1)?;
                    (revision, self.next_input)
                }
            })
        } else {
            None
        };
        let plan = if let Some(a) = self.runs.values().find(|a| a.plan.0 == version) {
            a.plan
        } else {
            self.next_plan = self.next_plan.checked_add(1)?;
            (version, self.next_plan)
        };
        self.runs.insert(
            run,
            RunAlias {
                run: self.next_run,
                input,
                plan,
                calls: BTreeMap::new(),
                ended: false,
                published: false,
            },
        );
        Some(())
    }
    fn byte_limit(&self) -> usize {
        MAX_BYTES
            - 4096
            - EVENT_INDEX_BYTES
            - usize::from(self.worker_inbox.is_some()) * WORKER_QUEUE_BYTES
    }
    fn drain_workers(&mut self, clock: &ExecutionClock) {
        // Continuous producers cannot make a Host event consume an unbounded
        // amount of diagnostic work while it owns the recorder.
        for _ in 0..WORKER_QUEUE_SLOTS {
            let Some(received) = self
                .worker_inbox
                .as_ref()
                .and_then(|inbox| inbox.receiver.try_recv().ok())
            else {
                break;
            };
            let bundle = match received {
                ReceivedMetadata::Accepted(bundle) => {
                    if !self.enabled || bundle.generation != self.generation {
                        self.dropped = self.dropped.saturating_add(1);
                        self.signals.complete(bundle.stamp.sequence);
                        continue;
                    }
                    let Ok(now) = clock.now() else {
                        self.dropped = self.dropped.saturating_add(1);
                        self.signals.complete(bundle.stamp.sequence);
                        continue;
                    };
                    if self
                        .accept_alias(bundle.run, bundle.input_revision, bundle.plan_version)
                        .is_none()
                    {
                        self.signals.complete(bundle.stamp.sequence);
                        continue;
                    }
                    // append_event_at owns the only successful completion;
                    // never complete an old reservation twice after tag reuse.
                    append_event_at(
                        self,
                        (
                            u64::try_from(now.saturating_sub(self.origin).as_micros())
                                .unwrap_or(u64::MAX),
                            bundle.stamp,
                        ),
                        bundle.run,
                        TraceKind::RunAccepted,
                        None,
                        None,
                        None,
                    );
                    continue;
                }
                ReceivedMetadata::Worker(bundle) => bundle,
                ReceivedMetadata::Host(bundle) => {
                    if !self.enabled || bundle.generation != self.generation {
                        self.dropped = self.dropped.saturating_add(1);
                        continue;
                    }
                    let Ok(now) = clock.now() else {
                        self.dropped = self.dropped.saturating_add(1);
                        continue;
                    };
                    let node = bundle
                        .node
                        .as_ref()
                        .map(|(index, kind, version)| (*index, kind.as_ref(), *version));
                    append_event_at(
                        self,
                        (
                            u64::try_from(now.saturating_sub(self.origin).as_micros())
                                .unwrap_or(u64::MAX),
                            bundle.stamp,
                        ),
                        bundle.run,
                        bundle.event,
                        node,
                        bundle.job,
                        None,
                    );
                    continue;
                }
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
        while self.events.first_key_value().is_some_and(|(_, entry)| {
            entry
                .0
                .at_us
                .parse::<u64>()
                .expect("owned timestamp")
                .saturating_add(RETENTION_US)
                < at
        }) {
            let sequence = *self.events.first_key_value().unwrap().0;
            self.remove_event(sequence);
        }
    }
    fn remove_event(&mut self, sequence: u64) {
        if let Some(value) = self.events.remove(&sequence) {
            self.bytes -= value.1;
            self.dropped = self.dropped.saturating_add(1);
        }
    }
    fn evict(&mut self) {
        if let Some((&sequence, _)) = self.events.first_key_value() {
            self.remove_event(sequence);
        }
    }
}
pub(crate) struct TraceSession {
    clock: ExecutionClock,
    enabled: AtomicBool,
    detailed: AtomicBool,
    generation: AtomicU64,
    state: Mutex<Option<Recording>>,
    host_route: RwLock<Option<HostRoute>>,
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
            host_route: RwLock::new(None),
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
        let signals = Arc::new(RecordSignals::new(origin)?);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| CoreError::new("trace-limit", "diagnostic generation exhausted"))?
            + 1;
        self.enabled.store(false, Ordering::Release);
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
            events: BTreeMap::new(),
            signals,
            bytes: 0,
            dropped: 0,
            calls: 0,
            worker_inbox: detailed.then(WorkerInbox::new),
        });
        *self.host_route.write().unwrap_or_else(|p| p.into_inner()) =
            state.as_ref().map(|r| HostRoute {
                generation,
                sink: r
                    .worker_inbox
                    .as_ref()
                    .map(|inbox| Arc::downgrade(&inbox.sink)),
                signals: Arc::downgrade(&r.signals),
            });
        self.detailed.store(detailed, Ordering::Release);
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }
    pub(crate) fn stop(&self) {
        self.enabled.store(false, Ordering::Release);
        self.host_route
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(r) = state.as_mut().filter(|r| r.enabled) {
            r.drain_workers(&self.clock);
            if let Some(inbox) = r.worker_inbox.take() {
                // A producer can have captured the old sink just before stop.
                // Disconnect it and snapshot its loss count so late producers
                // cannot mutate the frozen recording, even on queue overflow.
                r.dropped = r
                    .dropped
                    .saturating_add(inbox.sink.lost.load(Ordering::Relaxed));
            }
            r.signals.freeze();
            r.sequence = r.signals.visible.load(Ordering::Acquire);
            r.dropped = r
                .dropped
                .saturating_add(r.signals.lost.load(Ordering::Acquire));
            r.enabled = false;
            r.stopped = self.clock.now().ok();
            if let Some(stopped) = r.stopped {
                r.trim(
                    u64::try_from(stopped.saturating_sub(r.origin).as_micros()).unwrap_or(u64::MAX),
                );
            }
            // Bounded opt-in counter audit. No user values or public schema.
            if r.dropped != 0
                && std::env::var_os("DIY_EXPERIMENT_HOST_TRACE_LOSS_AUDIT").as_deref()
                    == Some(std::ffi::OsStr::new("1"))
                && AUDIT_REPORTS
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                        if n < 64 { Some(n + 1) } else { None }
                    })
                    .is_ok()
            {
                let a = &r.signals.audit;
                eprintln!(
                    "host-trace-loss-audit total={} probeBusy={} reservationFull={} clockUnavailable={} recorderBusy={} hostQueueFull={} invalidNative={} unfinished={}",
                    r.dropped,
                    a.probe_busy.load(Ordering::Relaxed),
                    a.reservation_full.load(Ordering::Relaxed),
                    a.clock_unavailable.load(Ordering::Relaxed),
                    a.recorder_busy.load(Ordering::Relaxed),
                    a.host_queue_full.load(Ordering::Relaxed),
                    a.invalid_native.load(Ordering::Relaxed),
                    a.unfinished.load(Ordering::Relaxed)
                );
            }
            self.enabled.store(false, Ordering::Release);
        }
    }
    pub(crate) fn clear(&self) {
        self.enabled.store(false, Ordering::Release);
        self.host_route
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        *state = None;
        self.enabled.store(false, Ordering::Release);
    }
    fn ticket_for_run(self: &Arc<Self>, run: u64) -> Option<TraceTicket> {
        if !self.enabled.load(Ordering::Acquire) {
            return None;
        }
        let routes = match self.host_route.try_read() {
            Ok(routes) => routes,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        let route = routes
            .as_ref()
            .filter(|r| r.generation == self.generation.load(Ordering::Acquire))?;
        Some(TraceTicket {
            session: Arc::downgrade(self),
            generation: route.generation,
            run,
            worker_sink: route.sink.clone(),
            signals: route.signals.clone(),
        })
    }
    pub(crate) fn accept(self: &Arc<Self>, key: &RunKey) -> Option<TraceTicket> {
        let ticket = self.ticket_for_run(key.run_id.get())?;
        let signals = ticket.signals.upgrade()?;
        let mut state = match self.state.try_lock() {
            Ok(s) => s,
            Err(_) => {
                signals.audit.recorder_busy.fetch_add(1, Ordering::Relaxed);
                if self.detailed.load(Ordering::Acquire) {
                    let sink = ticket.worker_sink.as_ref().and_then(Weak::upgrade)?;
                    let Some(stamp) = signals.capture(&self.clock) else {
                        signals.lose(TraceKind::RunAccepted, key.run_id.get());
                        return None;
                    };
                    let bundle = AcceptedBundle {
                        generation: ticket.generation,
                        run: key.run_id.get(),
                        input_revision: key.input_revision,
                        plan_version: key.plan_version.get(),
                        stamp,
                    };
                    if sink
                        .sender
                        .try_send(ReceivedMetadata::Accepted(bundle))
                        .is_err()
                    {
                        signals
                            .audit
                            .host_queue_full
                            .fetch_add(1, Ordering::Relaxed);
                        signals.lose(TraceKind::RunAccepted, key.run_id.get());
                        signals.complete(stamp.sequence);
                        return None;
                    }
                    // Allow physical-ending/publication to route before the
                    // consumer has allocated the pending run's private alias.
                    signals.note_first_run(key.run_id.get());
                    return Some(ticket);
                }
                signals.lost.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        let r = state
            .as_mut()
            .filter(|r| r.enabled && r.generation == ticket.generation)?;
        // Adopt earlier queued acceptance before allocating later aliases.
        r.drain_workers(&self.clock);
        r.accept_alias(key.run_id.get(), key.input_revision, key.plan_version.get())?;
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
        let Some(ticket) = self.ticket_for_run(key.run_id.get()) else {
            return;
        };
        let Some(signals) = ticket.signals.upgrade() else {
            return;
        };
        let first = signals.first_run.load(Ordering::Acquire);
        if first == 0 || key.run_id.get() < first {
            return;
        }
        if ticket.is_detailed() {
            ticket.queue_host(event, None, None);
        } else {
            ticket.emit(event, None, None);
        }
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
            r.signals.advance(MAX_EVENTS);
            r.sequence = r.signals.visible.load(Ordering::Acquire);
        }
        let events = r
            .events
            .range((std::ops::Bound::Excluded(after), std::ops::Bound::Unbounded))
            .take_while(|(sequence, _)| **sequence <= r.sequence)
            .take(limit)
            .map(|(_, entry)| entry.0.clone())
            .collect();
        Ok(TraceSnapshot {
            coverage: "new-runs-only",
            format_version: 1,
            recording_id: Some(r.id.clone()),
            recording: r.enabled,
            now_us: at.to_string(),
            first_sequence: r.events.range(..=r.sequence).next().map_or_else(
                || r.sequence.to_string(),
                |(_, entry)| entry.0.sequence.clone(),
            ),
            next_sequence: r.sequence.to_string(),
            dropped: r
                .dropped
                .saturating_add(if r.enabled {
                    r.signals.lost.load(Ordering::Relaxed)
                } else {
                    0
                })
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
    signals: Weak<RecordSignals>,
}
impl std::fmt::Debug for TraceTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TraceTicket(private scope)")
    }
}
impl TraceTicket {
    fn queue_host(&self, event: TraceKind, node: Option<(usize, &str, u32)>, job: Option<u64>) {
        if !self.is_detailed() {
            return;
        }
        let Some(registry) = self.session.upgrade() else {
            return;
        };
        let Some(signals) = self.signals.upgrade() else {
            return;
        };
        let Some(sink) = self.worker_sink.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        if event == TraceKind::HostResponsePublished
            && self.run <= signals.publication_loss_through.load(Ordering::Acquire)
        {
            return;
        }
        let Some(stamp) = signals.capture(&registry.clock) else {
            signals.lose(event, self.run);
            return;
        };
        let bundle = HostBundle {
            generation: self.generation,
            run: self.run,
            stamp,
            event,
            job,
            node: node.map(|(index, kind, version)| {
                (
                    index,
                    kind.chars().take(128).collect::<String>().into_boxed_str(),
                    version,
                )
            }),
        };
        if sink
            .sender
            .try_send(ReceivedMetadata::Host(bundle))
            .is_err()
        {
            signals
                .audit
                .host_queue_full
                .fetch_add(1, Ordering::Relaxed);
            signals.lose(event, self.run);
            signals.complete(stamp.sequence);
        }
    }
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
        if sink
            .sender
            .try_send(ReceivedMetadata::Worker(bundle))
            .is_err()
        {
            sink.lost.fetch_add(count, Ordering::Relaxed);
        }
    }
    pub(crate) fn lost_diagnostics(&self) {
        if self.is_detailed()
            && let Some(signals) = self.signals.upgrade()
        {
            signals.audit.invalid_native.fetch_add(1, Ordering::Relaxed);
            signals.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn emit_data(
        &self,
        event: TraceKind,
        node: Option<(usize, &str, u32)>,
        job: Option<u64>,
        worker: Option<WorkerTraceInterval>,
    ) {
        if self.is_detailed()
            && matches!(
                event,
                TraceKind::HostResponsePublished
                    | TraceKind::HostRequestStarted
                    | TraceKind::HostRequestFinished
                    | TraceKind::WorkerStagingStarted
                    | TraceKind::WorkerStagingFinished
                    | TraceKind::WorkerProcessStarted
                    | TraceKind::WorkerProcessFinished
                    | TraceKind::WorkerCleanupStarted
                    | TraceKind::WorkerCleanupFinished
                    | TraceKind::BrokerExchangeStarted
                    | TraceKind::BrokerExchangeFinished
            )
        {
            self.queue_host(event, node, job);
            return;
        }
        let Some(registry) = self.session.upgrade() else {
            return;
        };
        if !registry.enabled.load(Ordering::Acquire)
            || registry.generation.load(Ordering::Acquire) != self.generation
        {
            return;
        }
        let Some(signals) = self.signals.upgrade() else {
            return;
        };
        if event == TraceKind::HostResponsePublished
            && self.run <= signals.publication_loss_through.load(Ordering::Acquire)
        {
            return;
        }
        let mut sessions = match registry.state.try_lock() {
            Ok(s) => s,
            Err(_) => {
                signals.audit.recorder_busy.fetch_add(1, Ordering::Relaxed);
                if registry.detailed.load(Ordering::Acquire) && worker.is_none() {
                    // Core/deferred notifications may also arrive off the
                    // recorder's current thread. Preserve their measurement
                    // through the same bounded path, not a blocking retry.
                    self.queue_host(event, node, job);
                } else {
                    signals.lose(event, self.run);
                }
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
    let Some(stamp) = r.signals.capture(clock) else {
        r.signals.lose(event, run_key);
        return;
    };
    append_event_at(r, (stamp.at_us, stamp), run_key, event, node, job, worker);
}
fn append_event_at(
    r: &mut Recording,
    (current, stamp): (u64, Stamp),
    run_key: u64,
    event: TraceKind,
    node: Option<(usize, &str, u32)>,
    job: Option<u64>,
    worker: Option<WorkerTraceInterval>,
) {
    let _complete = CompleteStamp {
        signals: r.signals.clone(),
        sequence: stamp.sequence,
    };
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
    let at = stamp.at_us;
    r.trim(current);
    if at.saturating_add(RETENTION_US) < current {
        r.dropped = r.dropped.saturating_add(1);
        return;
    }
    let sequence = stamp.sequence;
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
    r.events.insert(sequence, Box::new((e, bytes)));
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
    #[test]
    fn detailed_host_phases_and_publication_do_not_wait_and_keep_measurement_times() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let guard = s.state.lock().unwrap();
        let producer_session = s.clone();
        let producer_clock = clock.clone();
        let (done, observed) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            producer_clock
                .advance_to(Duration::from_millis(10))
                .unwrap();
            t.emit(
                TraceKind::HostRequestStarted,
                Some((0, "fixture.node", 1)),
                None,
            );
            producer_clock
                .advance_to(Duration::from_millis(20))
                .unwrap();
            t.emit(
                TraceKind::HostRequestFinished,
                Some((0, "fixture.node", 1)),
                None,
            );
            producer_clock
                .advance_to(Duration::from_millis(30))
                .unwrap();
            producer_session.physical_ended(&key(9001));
            producer_session.response_published(&key(9001));
            let _ = done.send(());
        });
        let result = observed.recv_timeout(Duration::from_secs(2));
        drop(guard);
        producer.join().unwrap();
        result.expect("Host diagnostics waited for the recorder mutex");
        clock.advance_to(Duration::from_millis(100)).unwrap();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        let phases: Vec<_> = page
            .events
            .iter()
            .map(|e| (e.event, e.at_us.as_str()))
            .collect();
        assert_eq!(
            phases,
            vec![
                (TraceKind::RunAccepted, "0"),
                (TraceKind::HostRequestStarted, "10000"),
                (TraceKind::HostRequestFinished, "20000"),
                (TraceKind::PhysicalEnded, "30000"),
                (TraceKind::HostResponsePublished, "30000")
            ]
        );
    }
    #[test]
    fn contended_detailed_run_acceptance_keeps_aliases_without_waiting() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        s.accept(&key(9001)).unwrap();
        let guard = s.state.lock().unwrap();
        clock.advance_to(Duration::from_millis(10)).unwrap();
        let session = s.clone();
        let (done, observed) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            let _ = done.send(session.accept(&key(9002)));
        });
        let result = observed.recv_timeout(Duration::from_secs(2));
        drop(guard);
        producer.join().unwrap();
        let ticket = result
            .expect("run acceptance blocked on the recorder")
            .expect("contended detailed run acceptance lost its diagnostic ticket");
        clock.advance_to(Duration::from_millis(20)).unwrap();
        ticket.emit(
            TraceKind::NodeStarted,
            Some((0, "fixture.node", 1)),
            Some(77),
        );
        s.stop();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 3);
        assert_eq!(page.events[1].event, TraceKind::RunAccepted);
        assert_eq!(page.events[1].at_us, "10000");
        assert_eq!(page.events[1].run, 2);
        assert_eq!(page.events[2].event, TraceKind::NodeStarted);
        assert_eq!(page.events[2].at_us, "20000");
        assert_eq!(page.events[2].run, 2);
        assert_eq!(page.events[2].plan, page.events[0].plan);
        assert_eq!(page.events[2].input, page.events[0].input);
        assert_eq!(page.events[2].call, Some(1));
    }
    #[test]
    fn contended_detailed_core_notification_is_queued_with_its_call_identity() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let signals = t.signals.upgrade().unwrap();
        let guard = s.state.lock().unwrap();
        clock.advance_to(Duration::from_millis(10)).unwrap();
        let (done, observed) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            t.emit(
                TraceKind::NodeStarted,
                Some((0, "fixture.node", 1)),
                Some(991),
            );
            let _ = done.send(());
        });
        let result = observed.recv_timeout(Duration::from_secs(2));
        drop(guard);
        producer.join().unwrap();
        result.expect("Core notification blocked on the recorder");
        clock.advance_to(Duration::from_millis(100)).unwrap();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 2);
        let event = &page.events[1];
        assert_eq!(event.event, TraceKind::NodeStarted);
        assert_eq!(event.at_us, "10000");
        assert_eq!(event.node.as_deref(), Some("1:1"));
        assert_eq!(event.call, Some(1));
        assert_eq!(signals.audit.recorder_busy.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn queued_first_admission_routes_ending_and_precedes_uncontended_aliases() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let guard = s.state.lock().unwrap();
        clock.advance_to(Duration::from_millis(10)).unwrap();
        s.accept(&key(9001)).unwrap();
        clock.advance_to(Duration::from_millis(11)).unwrap();
        s.physical_ended(&key(9001));
        drop(guard);
        clock.advance_to(Duration::from_millis(20)).unwrap();
        s.accept(&key(9002)).unwrap();
        s.stop();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 3);
        assert_eq!(page.events[0].event, TraceKind::RunAccepted);
        assert_eq!(page.events[0].run, 1);
        assert_eq!(page.events[0].at_us, "10000");
        assert_eq!(page.events[1].event, TraceKind::PhysicalEnded);
        assert_eq!(page.events[1].run, 1);
        assert_eq!(page.events[1].at_us, "11000");
        assert_eq!(page.events[2].event, TraceKind::RunAccepted);
        assert_eq!(page.events[2].run, 2);
        assert_eq!(page.events[2].at_us, "20000");
        assert_eq!(page.events[0].input, page.events[2].input);
        assert_eq!(page.events[0].plan, page.events[2].plan);
    }
    #[test]
    fn queued_admission_overflow_is_explicit_and_completes_its_reservation() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let guard = s.state.lock().unwrap();
        let first = s.accept(&key(9001)).unwrap();
        let signals = first.signals.upgrade().unwrap();
        for n in 1..WORKER_QUEUE_SLOTS {
            assert!(s.accept(&key(9001 + n as u64)).is_some());
        }
        assert!(s.accept(&key(9001 + WORKER_QUEUE_SLOTS as u64)).is_none());
        drop(guard);
        s.stop();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.events.len(), WORKER_QUEUE_SLOTS);
        assert_eq!(page.dropped, 1);
        assert_eq!(signals.audit.host_queue_full.load(Ordering::Relaxed), 1);
        assert_eq!(signals.audit.unfinished.load(Ordering::Relaxed), 0);
        assert_eq!(
            signals.visible.load(Ordering::Acquire),
            WORKER_QUEUE_SLOTS as u64 + 1
        );
    }
    #[test]
    fn clear_discards_pending_admission_without_contaminating_new_recording() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let guard = s.state.lock().unwrap();
        let old = s.accept(&key(9001)).unwrap();
        drop(guard);
        s.clear();
        s.start_detailed().unwrap();
        s.accept(&key(9002)).unwrap();
        old.emit(TraceKind::NodeStarted, Some((0, "old.node", 1)), Some(77));
        s.stop();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event, TraceKind::RunAccepted);
        assert_eq!(page.events[0].run, 1);
    }
    #[test]
    fn reordered_acceptance_does_not_hide_earlier_admitted_run_ending() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        s.accept(&key(9002)).unwrap();
        s.accept(&key(9001)).unwrap();
        s.physical_ended(&key(9001));
        s.stop();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 3);
        assert_eq!(page.events[2].event, TraceKind::PhysicalEnded);
        assert_eq!(page.events[2].run, 2);
    }
    #[test]
    fn highest_run_id_remains_recordable_after_queued_acceptance() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let guard = s.state.lock().unwrap();
        let last = key(u64::MAX);
        s.accept(&last).unwrap();
        s.physical_ended(&last);
        drop(guard);
        s.stop();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].event, TraceKind::RunAccepted);
        assert_eq!(page.events[1].event, TraceKind::PhysicalEnded);
        assert_eq!(page.events[1].run, 1);
    }
    fn send_host_stamp(s: &TraceSession, run: u64, stamp: Stamp, event: TraceKind) {
        let routes = s.host_route.read().unwrap();
        let route = routes.as_ref().unwrap();
        assert!(
            route
                .sink
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap()
                .sender
                .try_send(ReceivedMetadata::Host(HostBundle {
                    generation: route.generation,
                    run,
                    stamp,
                    event,
                    node: None,
                    job: None,
                }))
                .is_ok()
        );
    }
    #[test]
    fn delayed_host_reservation_keeps_clock_order_cursor_and_retention() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        clock.advance_to(Duration::from_secs(1)).unwrap();
        let stamp = t.signals.upgrade().unwrap().capture(&clock).unwrap();
        clock.advance_to(Duration::from_secs(2)).unwrap();
        t.emit(TraceKind::NodeStarted, None, None);
        let pending = s.read(0, 256).unwrap();
        assert_eq!(pending.next_sequence, "1");
        assert_eq!(pending.first_sequence, "1");
        assert_eq!(
            pending.events.len(),
            1,
            "later events cannot skip a pending measurement"
        );
        clock.advance_to(Duration::from_secs(3)).unwrap();
        send_host_stamp(&s, 9001, stamp, TraceKind::HostRequestStarted);
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(
            page.events
                .iter()
                .map(|e| (e.sequence.as_str(), e.at_us.as_str()))
                .collect::<Vec<_>>(),
            vec![("1", "0"), ("2", "1000000"), ("3", "2000000")]
        );
        assert_eq!(s.read(2, 1).unwrap().events[0].sequence, "3");
        clock.advance_to(Duration::from_millis(61500)).unwrap();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 2);
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].sequence, "3");
        assert!(s.read(3, 256).unwrap().events.is_empty());
        let guard = s.state.lock().unwrap();
        let r = guard.as_ref().unwrap();
        assert!(r.bytes + EVENT_INDEX_BYTES + WORKER_QUEUE_BYTES + 4096 <= MAX_BYTES);
        assert_eq!(r.signals.completed.len(), MAX_EVENTS);
    }
    #[test]
    fn full_host_queue_counts_loss_and_never_relabels_a_publication_retry() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let first = key(9001);
        let t = s.accept(&first).unwrap();
        for _ in 0..WORKER_QUEUE_SLOTS {
            t.emit(TraceKind::HostRequestStarted, None, None);
        }
        s.response_published(&first);
        assert_eq!(s.read(0, 256).unwrap().dropped, 1);
        s.response_published(&first);
        let next = key(9002);
        s.accept(&next).unwrap();
        s.response_published(&next);
        s.response_published(&next);
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 1);
        let published: Vec<_> = page
            .events
            .iter()
            .filter(|e| e.event == TraceKind::HostResponsePublished)
            .collect();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].run, 2);
    }
    #[test]
    fn expired_first_publication_is_missing_not_a_later_retry() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let first = key(9001);
        s.accept(&first).unwrap();
        clock.advance_to(Duration::from_secs(1)).unwrap();
        s.response_published(&first);
        clock.advance_to(Duration::from_secs(62)).unwrap();
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 2);
        assert!(page.events.is_empty());
        s.response_published(&first);
        assert!(s.read(0, 256).unwrap().events.is_empty());
        s.accept(&key(9002)).unwrap();
        s.response_published(&key(9002));
        let page = s.read(0, 256).unwrap();
        assert_eq!(
            page.events
                .iter()
                .filter(|e| e.event == TraceKind::HostResponsePublished)
                .count(),
            1
        );
        assert_eq!(page.events.last().unwrap().run, 2);
    }
    #[test]
    fn old_host_sinks_cannot_mutate_frozen_or_new_recordings() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let old = s.accept(&key(9001)).unwrap();
        let sink = old.worker_sink.as_ref().unwrap().upgrade().unwrap();
        old.emit(TraceKind::HostRequestStarted, None, None);
        s.stop();
        let frozen = serde_json::to_value(s.read(0, 256).unwrap()).unwrap();
        sink.lost.fetch_add(9, Ordering::Relaxed);
        old.signals
            .upgrade()
            .unwrap()
            .publication_loss_through
            .store(u64::MAX, Ordering::Release);
        old.emit(TraceKind::HostRequestFinished, None, None);
        assert_eq!(
            frozen,
            serde_json::to_value(s.read(0, 256).unwrap()).unwrap()
        );
        s.clear();
        s.start_detailed().unwrap();
        s.accept(&key(9002)).unwrap();
        old.emit(TraceKind::HostResponsePublished, None, None);
        s.response_published(&key(9001));
        s.response_published(&key(9002));
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[1].event, TraceKind::HostResponsePublished);
        assert_eq!(page.events[1].run, 1);
        let weak = Arc::downgrade(&s);
        drop(s);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn host_queue_loss_is_local_to_its_session() {
        let (_, a) = setup();
        let (_, b) = setup();
        a.start_detailed().unwrap();
        b.start_detailed().unwrap();
        let t = a.accept(&key(9001)).unwrap();
        b.accept(&key(9001)).unwrap();
        for _ in 0..WORKER_QUEUE_SLOTS + 1 {
            t.emit(TraceKind::HostRequestStarted, None, None);
        }
        b.response_published(&key(9001));
        assert_eq!(a.read(0, 256).unwrap().dropped, 1);
        let page = b.read(0, 256).unwrap();
        assert_eq!(page.dropped, 0);
        assert_eq!(page.events.len(), 2);
    }
    #[test]
    fn reservation_capacity_is_fixed_and_reused_only_after_the_prefix_resolves() {
        let (clock, _) = setup();
        let signals = RecordSignals::new(Duration::ZERO).unwrap();
        for sequence in 1..=MAX_EVENTS as u64 {
            assert_eq!(signals.capture(&clock).unwrap().sequence, sequence);
        }
        assert!(signals.capture(&clock).is_none());
        for sequence in 2..=MAX_EVENTS as u64 {
            signals.complete(sequence);
        }
        assert_eq!(signals.visible.load(Ordering::Acquire), 0);
        assert!(signals.capture(&clock).is_none());
        signals.complete(1);
        signals.advance(MAX_EVENTS);
        let next = signals.capture(&clock).unwrap();
        assert_eq!(next.sequence, MAX_EVENTS as u64 + 1);
        signals.complete(next.sequence);
        assert_eq!(signals.visible.load(Ordering::Acquire), next.sequence);
        assert_eq!(signals.completed.len(), MAX_EVENTS);
    }
    #[test]
    fn busy_clock_probe_drops_metadata_without_waiting_or_reserving_a_gap() {
        let (_, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let signals = t.signals.upgrade().unwrap();
        signals.probe.store(true, Ordering::Release);
        let (done, observed) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            t.emit(TraceKind::HostRequestStarted, None, None);
            let _ = done.send(());
        });
        let result = observed.recv_timeout(Duration::from_secs(2));
        signals.probe.store(false, Ordering::Release);
        producer.join().unwrap();
        result.expect("diagnostic clock probe blocked execution");
        let page = s.read(0, 256).unwrap();
        assert_eq!(page.dropped, 1);
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.next_sequence, "1");
    }
    #[test]
    fn stop_resolves_missing_reservations_and_freezes_late_completion_and_loss() {
        let (clock, s) = setup();
        s.start_detailed().unwrap();
        let t = s.accept(&key(9001)).unwrap();
        let signals = t.signals.upgrade().unwrap();
        clock.advance_to(Duration::from_secs(1)).unwrap();
        let missing = signals.capture(&clock).unwrap();
        clock.advance_to(Duration::from_secs(2)).unwrap();
        t.emit(TraceKind::NodeStarted, None, None);
        assert_eq!(s.read(0, 256).unwrap().next_sequence, "1");
        s.stop();
        let stopped = s.read(0, 256).unwrap();
        assert_eq!(stopped.next_sequence, "3");
        assert_eq!(stopped.dropped, 1);
        assert_eq!(
            stopped
                .events
                .iter()
                .map(|e| e.sequence.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "3"]
        );
        let frozen = serde_json::to_value(stopped).unwrap();
        signals.complete(missing.sequence);
        signals.lose(TraceKind::HostResponsePublished, 9001);
        clock.advance_to(Duration::from_secs(100)).unwrap();
        s.stop();
        assert_eq!(
            frozen,
            serde_json::to_value(s.read(0, 256).unwrap()).unwrap()
        );
        assert!(signals.capture(&clock).is_none());
        s.clear();
        s.start().unwrap();
        s.accept(&key(9002)).unwrap();
        signals.lose(TraceKind::HostResponsePublished, u64::MAX);
        let guard = s.state.lock().unwrap();
        t.emit(TraceKind::NodeStarted, None, None);
        drop(guard);
        s.response_published(&key(9002));
        let next = s.read(0, 256).unwrap();
        assert_eq!(next.dropped, 0);
        assert_eq!(next.events.len(), 2);
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
                .filter(|(_, entry)| entry.0.worker.is_some())
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
                .try_send(ReceivedMetadata::Worker(WorkerBundle {
                    run: 9001,
                    received: Duration::ZERO,
                    node: 0,
                    kind: "old".into(),
                    version: 1,
                    instance: 1,
                    intervals: native_intervals(),
                }))
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
                .try_send(ReceivedMetadata::Worker(WorkerBundle {
                    run: 9001,
                    received: Duration::ZERO,
                    node: 0,
                    kind: "late".into(),
                    version: 1,
                    instance: 1,
                    intervals: native_intervals(),
                }))
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
        assert!(
            r.events
                .first_key_value()
                .unwrap()
                .1
                .0
                .sequence
                .parse::<u64>()
                .unwrap()
                > 1
        );
        assert_eq!(
            r.events
                .last_key_value()
                .unwrap()
                .1
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
        assert_eq!(
            r.events.last_key_value().unwrap().1.0.event,
            TraceKind::NodeStarted
        );
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
