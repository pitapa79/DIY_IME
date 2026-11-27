//! Client for the existing consented, isolated execution Host. Only its
//! background owner performs pipe I/O, waits, or reaps processes.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use diy_ime2_runtime::{
    Candidate, Packet,
    execution_control::{DecimalId, RunKey, RunPhase},
    node::RuntimeInput,
};
use serde_json::{Value, json};

use crate::{Conversion, Plan};

const REQUEST_LIMIT: usize = 64 * 1024;
const RESPONSE_LIMIT: u64 = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(45);
const WAIT: Duration = Duration::from_millis(10);
const GRAPH: &str = include_str!("../../../examples/graphs/official-dictionary.json");

// Diagnostics contain only fixed stage names and numeric OS/exit codes.
#[cfg(all(windows, feature = "stage-b-diagnostics"))]
fn startup_diagnostic(stage: &'static str, code: Option<i32>) {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OutputDebugStringW(message: *const u16);
    }
    let message = match code {
        Some(code) => format!("DIY IME Stage B: {stage}, code={code}"),
        None => format!("DIY IME Stage B: {stage}"),
    };
    let wide: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe { OutputDebugStringW(wide.as_ptr()) };
}

macro_rules! startup_trace {
    ($stage:literal, $code:expr) => {{
        #[cfg(all(windows, feature = "stage-b-diagnostics"))]
        startup_diagnostic($stage, $code);
    }};
}

#[derive(Clone)]
pub struct Configuration {
    pub executable: PathBuf,
    pub file: PathBuf,
}

pub enum Event {
    Ready(Plan),
    Started(RunKey),
    Converted(RunKey, Output),
    Unavailable,
}

pub struct Output {
    pub conversion: Conversion,
    pub candidates: Vec<String>,
}

#[derive(Clone, Default)]
pub struct Observation {
    dispatched: Arc<AtomicU64>,
    reaped: Arc<AtomicU64>,
}

impl Observation {
    pub fn dispatched(&self) -> u64 {
        self.dispatched.load(Ordering::Acquire)
    }

    pub fn reaped(&self) -> u64 {
        self.reaped.load(Ordering::Acquire)
    }
}

pub struct Stopped(Arc<AtomicBool>);

impl Stopped {
    pub fn confirmed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct Background {
    requests: SyncSender<RuntimeInput>,
    events: Receiver<Event>,
    revision: Arc<AtomicU64>,
    acknowledged: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

impl Background {
    /// Paths come from the trusted application/test launcher. The client
    /// submits the compiled-in graph and can never grant its own permissions.
    pub fn spawn(config: Configuration, lifetime: impl Send + 'static) -> io::Result<Self> {
        Self::spawn_observed(config, Observation::default(), lifetime)
    }

    pub fn spawn_observed(
        config: Configuration,
        observation: Observation,
        lifetime: impl Send + 'static,
    ) -> io::Result<Self> {
        if !config.executable.is_absolute() || !config.file.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute Host paths required",
            ));
        }
        let (requests, inbox) = mpsc::sync_channel(1);
        let (outbox, events) = mpsc::sync_channel(2);
        let revision = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let active_revision = revision.clone();
        let acknowledged = Arc::new(AtomicU64::new(0));
        let applied_revision = acknowledged.clone();
        let closing = shutdown.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let witnessed = stopped.clone();
        thread::Builder::new()
            .name("DIY IME Host".into())
            .spawn(move || {
                let _lifetime = lifetime;
                if supervise(
                    config,
                    inbox,
                    &outbox,
                    Signals {
                        revision: active_revision,
                        acknowledged: applied_revision,
                        shutdown: closing,
                        stopped: witnessed,
                        observation,
                    },
                )
                .is_none()
                {
                    startup_trace!("Host background operation failed", None);
                    let _ = outbox.try_send(Event::Unavailable);
                }
            })
            .inspect_err(|_error| {
                startup_trace!(
                    "Host background thread creation failed",
                    _error.raw_os_error()
                );
            })?;
        Ok(Self {
            requests,
            events,
            revision,
            acknowledged,
            shutdown,
            stopped,
        })
    }

    /// Invalidate even when the next work item cannot be enqueued. The
    /// UI retains its local raw preedit and can submit a newer revision.
    pub fn submit(&self, input: RuntimeInput) -> bool {
        !self.shutdown.load(Ordering::Acquire)
            && input.validate().is_ok()
            && self.advance(input.revision)
            && self.requests.try_send(input).is_ok()
    }

    pub fn invalidate(&self, revision: u64) {
        self.advance(revision);
    }

    fn advance(&self, revision: u64) -> bool {
        self.revision.fetch_max(revision, Ordering::AcqRel) < revision
    }

    pub fn request_stop(&self) -> Stopped {
        self.shutdown.store(true, Ordering::Release);
        Stopped(self.stopped.clone())
    }

    pub fn acknowledged(&self) -> u64 {
        self.acknowledged.load(Ordering::Acquire)
    }

    pub fn poll(&self) -> Result<Option<Event>, TryRecvError> {
        loop {
            match self.events.try_recv() {
                Ok(Event::Started(ref key) | Event::Converted(ref key, _))
                    if key.input_revision != Some(self.revision.load(Ordering::Acquire)) => {}
                Ok(event) => return Ok(Some(event)),
                Err(TryRecvError::Empty) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

struct Signals {
    revision: Arc<AtomicU64>,
    acknowledged: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    observation: Observation,
}

fn supervise(
    config: Configuration,
    inbox: Receiver<RuntimeInput>,
    outbox: &SyncSender<Event>,
    signals: Signals,
) -> Option<()> {
    let Signals {
        revision,
        acknowledged,
        shutdown,
        stopped,
        observation,
    } = signals;
    let mut host = Transport::open(
        &config.executable,
        &config.file,
        shutdown.clone(),
        stopped,
        observation.clone(),
    )
    .ok()?;
    let graph = serde_json::from_str::<Value>(GRAPH).ok()?;
    startup_trace!("Host official IME preparation started", None);
    let Some(review) = host.call(json!({"method": "prepareOfficialIme", "graph": graph})) else {
        startup_trace!("Host official IME preparation failed", None);
        return None;
    };
    if !review["missing"].as_array()?.is_empty() {
        startup_trace!("Host official IME requires permissions", None);
        return None;
    }
    let review_id = review["reviewId"].as_str()?;
    startup_trace!("Host plan activation started", None);
    let Some(active) = host.call(json!({"method": "activate", "reviewId": review_id})) else {
        startup_trace!("Host plan activation failed", None);
        return None;
    };
    if active["phase"] != "ready" || active["sessionEpoch"] != review["sessionEpoch"] {
        return None;
    }
    let plan = Plan {
        session: serde_json::from_value(active["sessionEpoch"].clone()).ok()?,
        version: serde_json::from_value(active["state"]["activePlanVersion"].clone()).ok()?,
    };
    outbox.try_send(Event::Ready(plan.clone())).ok()?;
    startup_trace!("Host ready event sent", None);
    let mut applied = 0;
    while !shutdown.load(Ordering::Acquire) {
        let latest = revision.load(Ordering::Acquire);
        if latest > applied {
            host.call(json!({
                "method": "invalidateInput", "expectedPlanVersion": plan.version, "revision": latest
            }))?;
            applied = latest;
            acknowledged.store(applied, Ordering::Release);
        }
        let input = match inbox.recv_timeout(WAIT) {
            Ok(input) => input,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if input.revision != revision.load(Ordering::Acquire) {
            continue;
        }
        if input.revision > applied {
            host.call(json!({
                "method": "invalidateInput", "expectedPlanVersion": plan.version, "revision": input.revision
            }))?;
            applied = input.revision;
            acknowledged.store(applied, Ordering::Release);
        }
        if input.revision != revision.load(Ordering::Acquire) {
            continue;
        }
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        observation.dispatched.fetch_add(1, Ordering::Relaxed);
        let key = start(&mut host, &plan, &input)?;
        let _ = outbox.try_send(Event::Started(key.clone()));
        match settle(&mut host, &key, &revision)? {
            Some(result) => {
                let _ = outbox.try_send(Event::Converted(key, result));
            }
            None if !shutdown.load(Ordering::Acquire) => {
                let newest = revision.load(Ordering::Acquire);
                host.call(json!({
                    "method": "invalidateInput", "expectedPlanVersion": plan.version, "revision": newest
                }))?;
                applied = newest;
                acknowledged.store(applied, Ordering::Release);
            }
            None => break,
        }
    }
    Some(())
}

fn start(host: &mut Transport, plan: &Plan, input: &RuntimeInput) -> Option<RunKey> {
    let start_key = DecimalId::new(input.revision).ok()?;
    let receipt = host.call(json!({"method": "run", "request": {
        "startKey": start_key, "expectedPlanVersion": plan.version,
        "targets": [{"node": "result", "port": "out"}, {"node": "dictionary", "port": "out"}], "input": input
    }}))?;
    let key: RunKey = serde_json::from_value(receipt["receipt"]["key"].clone()).ok()?;
    (key.session_epoch == plan.session
        && key.plan_version == plan.version
        && key.input_revision == Some(input.revision)
        && receipt["receipt"]["replayed"] == false)
        .then_some(key)
}

fn settle(host: &mut Transport, key: &RunKey, revision: &AtomicU64) -> Option<Option<Output>> {
    let deadline = Instant::now() + TIMEOUT;
    while !host.shutdown.load(Ordering::Acquire)
        && key.input_revision == Some(revision.load(Ordering::Acquire))
    {
        let reply = host.call(json!({"method": "getRun", "key": key}))?;
        let snapshot = &reply["snapshot"];
        let returned: RunKey = serde_json::from_value(snapshot["key"].clone()).ok()?;
        if &returned != key || snapshot["current"] != true {
            return None;
        }
        let phase: RunPhase = serde_json::from_value(snapshot["state"].clone()).ok()?;
        let idle = ["running", "cancelRequested", "stalled", "queued"]
            .into_iter()
            .all(|field| snapshot["physical"][field].as_u64() == Some(0));
        if phase == RunPhase::Terminal && idle {
            let result = &snapshot["result"];
            return Some(Some(Output {
                conversion: delivery(result),
                candidates: candidates(result),
            }));
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(WAIT);
    }
    Some(None)
}

fn candidates(result: &Value) -> Vec<String> {
    if result["reason"] != "completed"
        || !matches!(result["delivery"].as_str(), Some("complete" | "recovered"))
    {
        return Vec::new();
    }
    let Some(value) = output_port(result, "dictionary") else {
        return Vec::new();
    };
    if value["status"] != "data" {
        return Vec::new();
    }
    let Ok(packet) = serde_json::from_value::<Packet>(value["packet"].clone()) else {
        return Vec::new();
    };
    if packet.validate().is_err() {
        return Vec::new();
    }
    let Candidate::Choice { options } = packet.root else {
        return Vec::new();
    };
    options
        .into_iter()
        .take(64)
        .filter_map(|option| match option.value {
            Candidate::Text { text, .. }
                if !text.is_empty() && text.encode_utf16().count() <= 4096 =>
            {
                Some(text)
            }
            _ => None,
        })
        .collect()
}

fn output_port<'a>(result: &'a Value, node: &str) -> Option<&'a Value> {
    result["ports"]
        .as_array()?
        .iter()
        .find(|port| port["endpoint"]["node"] == node && port["endpoint"]["port"] == "out")
        .map(|port| &port["value"])
}

fn delivery(result: &Value) -> Conversion {
    if result["reason"] != "completed" {
        return Conversion::Invalid;
    }
    let Some(value) = output_port(result, "result") else {
        return Conversion::Invalid;
    };
    match (result["delivery"].as_str(), value["status"].as_str()) {
        (Some("no-output"), Some("no-output")) => Conversion::NoOutput,
        (Some("complete" | "recovered" | "no-output"), Some("data")) => {
            let parsed = serde_json::from_value::<Packet>(value["packet"].clone());
            if let Ok(packet) = parsed
                && packet.validate().is_ok()
                && let Candidate::Text { text, .. } = packet.root
            {
                return Conversion::Text(text);
            }
            Conversion::Invalid
        }
        _ => Conversion::Invalid,
    }
}

struct Transport {
    child: Child,
    writer: Option<SyncSender<Vec<u8>>>,
    reader: Receiver<Value>,
    pipes: Vec<JoinHandle<()>>,
    next_id: u64,
    shutdown: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    observation: Observation,
}

impl Transport {
    fn open(
        exe: &Path,
        config: &Path,
        shutdown: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
        observation: Observation,
    ) -> io::Result<Self> {
        let mut command = Command::new(exe);
        command.current_dir(exe.parent().expect("absolute executable path"));
        command
            .arg(config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                startup_trace!("Host process launch failed", error.raw_os_error());
                stopped.store(true, Ordering::Release);
                return Err(error);
            }
        };
        let mut stdin = child.stdin.take().expect("requested piped stdin");
        let stdout = child.stdout.take().expect("requested piped stdout");
        let (writer, writes) = mpsc::sync_channel::<Vec<u8>>(1);
        let (responses, reader) = mpsc::sync_channel(1);
        let mut transport = Self {
            child,
            writer: Some(writer),
            reader,
            pipes: Vec::new(),
            next_id: 0,
            shutdown,
            stopped,
            observation,
        };
        let writer_shutdown = transport.shutdown.clone();
        transport
            .pipes
            .push(
                thread::Builder::new()
                    .name("IME Host input".into())
                    .spawn(move || {
                        for frame in writes {
                            if writer_shutdown.load(Ordering::Acquire)
                                || stdin.write_all(&frame).is_err()
                            {
                                break;
                            }
                        }
                    })?,
            );
        transport.pipes.push(
            thread::Builder::new()
                .name("IME Host output".into())
                .spawn(move || {
                    let mut stdout = BufReader::new(stdout);
                    let mut line = Vec::new();
                    loop {
                        line.clear();
                        if !(&mut stdout)
                            .take(RESPONSE_LIMIT + 1)
                            .read_until(b'\n', &mut line)
                            .is_ok_and(|size| size > 0 && size as u64 <= RESPONSE_LIMIT)
                        {
                            break;
                        }
                        let Ok(value) = serde_json::from_slice(&line) else {
                            break;
                        };
                        if responses.try_send(value).is_err() {
                            break;
                        }
                    }
                })?,
        );
        Ok(transport)
    }

    fn call(&mut self, operation: Value) -> Option<Value> {
        self.next_id = self.next_id.checked_add(1)?;
        let id = self.next_id.to_string();
        let body = serde_json::to_vec(&json!({"id": id, "operation": operation})).ok()?;
        if body.len() > REQUEST_LIMIT || self.shutdown.load(Ordering::Acquire) {
            return None;
        }
        let mut frame = Vec::with_capacity(12 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&0_u64.to_le_bytes());
        frame.extend_from_slice(&body);
        self.writer.as_ref()?.try_send(frame).ok()?;
        let deadline = Instant::now() + TIMEOUT;
        while !self.shutdown.load(Ordering::Acquire) && Instant::now() < deadline {
            match self.reader.recv_timeout(WAIT) {
                Ok(mut value) if value["id"] == id && value["ok"] == true => {
                    return value.as_object_mut()?.remove("data");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(_) => {
                    startup_trace!("Host protocol response rejected", None);
                    return None;
                }
                Err(_) => {
                    startup_trace!("Host response pipe closed", None);
                    return None;
                }
            }
        }
        startup_trace!("Host response wait ended", None);
        None
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.writer.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        let clean_exit = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    startup_trace!("Host process exited", status.code());
                    break status.success();
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(WAIT),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    // A forced Host exit does not acknowledge its Worker recovery.
                    break false;
                }
            }
        };
        for pipe in self.pipes.drain(..) {
            let _ = pipe.join();
        }
        if clean_exit {
            self.observation.reaped.fetch_add(1, Ordering::Release);
            self.stopped.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_output_and_failure_are_distinct_from_empty_text() {
        for (report, port, expected) in [
            (
                "no-output",
                json!({"status": "no-output"}),
                Conversion::NoOutput,
            ),
            ("failed", json!({"status": "error"}), Conversion::Invalid),
            (
                "complete",
                json!({"status": "data", "packet": {"version": 1, "root": {"type": "text", "text": ""}}}),
                Conversion::Text(String::new()),
            ),
        ] {
            let result = json!({"reason": "completed", "delivery": report, "ports": [{
                "endpoint": {"node": "result", "port": "out"}, "value": port
            }]});
            assert_eq!(delivery(&result), expected);
        }
    }
}
