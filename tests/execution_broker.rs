#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    execution_control::{
        CancellationToken, CompletionPermit, DeferredNode, ExecutionClock, ExecutionHost,
        ExecutionService, OwnedInvocation, ResourcePolicy,
        broker::{self, Broker},
        protocol::MAX_REQUEST_BYTES,
    },
    node::{CoreError, NodeDefinition, Port, Registry},
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Cursor, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use test_nodes::register_builtin_nodes;
fn policy() -> ResourcePolicy {
    let mut p =
        ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
    p.max_physical_jobs = 1;
    p.max_queued_jobs = 0;
    p
}
fn service(name: &str) -> (ExecutionHost, ExecutionClock, ExecutionService) {
    let host = ExecutionHost::new(name, policy()).unwrap();
    let clock = ExecutionClock::controlled(Duration::ZERO);
    let service = ExecutionService::with_clock(host.clone(), clock.clone()).unwrap();
    (host, clock, service)
}
fn registry() -> Registry {
    let mut r = Registry::new();
    register_builtin_nodes(&mut r).unwrap();
    r
}
fn frame(request: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"broker":1,"id":"18446744073709551615","request":request})).unwrap()
}
fn call(b: &mut Broker, request: Value) -> Value {
    serde_json::from_slice(&b.reply(&frame(request)).unwrap()).unwrap()
}
fn ok(b: &mut Broker, request: Value) -> Value {
    let r = call(b, request);
    assert_eq!(r["ok"], true, "{r}");
    r["data"].clone()
}
fn open(b: &mut Broker) -> Value {
    ok(b, json!({"method":"open-session"}))
}
fn wrapped(epoch: &Value, request: Value) -> Value {
    json!({"method":"request","sessionEpoch":epoch,"message":{"protocol":3,"id":"18446744073709551615","request":request}})
}
fn inner(b: &mut Broker, epoch: &Value, request: Value) -> Value {
    ok(b, wrapped(epoch, request))
}
fn prepare(b: &mut Broker, epoch: &Value, kind: &str) {
    let reply = inner(
        b,
        epoch,
        json!({"method":"prepare","graph":{"version":1,"nodes":[{"id":"source","kind":kind,"version":1}],"edges":[]}}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let active = inner(
        b,
        epoch,
        json!({"method":"activate","token":reply["data"]["token"]}),
    );
    assert_eq!(active["ok"], true, "{active}");
}
fn start() -> Value {
    json!({"method":"start-run","startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}]})
}
fn until(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "broker did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn sessions_share_one_domain_but_keep_plan_state_and_global_four_session_limit() {
    let (_, _, service) = service("broker-sessions");
    let mut a = Broker::new(&service, registry());
    let mut b = Broker::new(&service, registry());
    let first = open(&mut a);
    let second = open(&mut b);
    assert_eq!(first["resourceDomain"], second["resourceDomain"]);
    assert_eq!(
        first["resourcePolicy"],
        serde_json::to_value(policy()).unwrap()
    );
    assert_ne!(first["sessionEpoch"], second["sessionEpoch"]);
    prepare(&mut a, &first["sessionEpoch"], "input.text");
    let untouched = inner(&mut b, &second["sessionEpoch"], json!({"method":"status"}));
    assert_eq!(untouched["state"]["revision"], "0");
    assert!(untouched["state"]["activePlanVersion"].is_null());
    assert_eq!(
        call(
            &mut a,
            wrapped(&second["sessionEpoch"], json!({"method":"hello"}))
        )["error"]["code"],
        "broker-session-unknown"
    );
    let third = open(&mut a);
    let fourth = open(&mut a);
    let denied = call(&mut b, json!({"method":"open-session"}));
    assert_eq!(denied["error"]["code"], "busy");
    ok(
        &mut a,
        json!({"method":"close-session","sessionEpoch":third["sessionEpoch"]}),
    );
    let reopened = open(&mut b);
    assert_ne!(third["sessionEpoch"], reopened["sessionEpoch"]);
    assert_eq!(first["resourceDomain"], reopened["resourceDomain"]);
    // Dropping an entire broker leaves the other broker's existing Sessions alive.
    drop(a);
    assert_eq!(
        inner(&mut b, &second["sessionEpoch"], json!({"method":"hello"}))["ok"],
        true
    );
    let extra = open(&mut b);
    assert_ne!(fourth["sessionEpoch"], extra["sessionEpoch"]);
}
#[test]
fn invalid_outer_and_inner_frames_are_distinct_and_never_allocate_a_session() {
    let (_, _, service) = service("broker-validation");
    let mut broker = Broker::new(&service, registry());
    for (request, code) in [
        (
            json!({"method":"open-session","extra":true}),
            "broker-request",
        ),
        (json!({"method":"unknown"}), "broker-request"),
        (
            json!({"method":"close-session","sessionEpoch":"not-an-epoch"}),
            "broker-request",
        ),
        (
            json!({"method":"close-session","sessionEpoch":"00000000000000000000000000000000"}),
            "broker-session-unknown",
        ),
    ] {
        let reply = call(&mut broker, request);
        assert_eq!(reply["id"], "18446744073709551615");
        assert_eq!(reply["error"]["code"], code);
    }
    for (input, code, id) in [
        (
            json!({"broker":2,"id":"7","request":{"method":"open-session"}}),
            "broker-version",
            json!("7"),
        ),
        (
            json!({"broker":1,"id":7,"request":{"method":"open-session"}}),
            "broker-envelope",
            Value::Null,
        ),
        (
            json!({"broker":1,"id":"01","request":{"method":"open-session"}}),
            "broker-envelope",
            Value::Null,
        ),
        (
            json!({"broker":1,"id":"7","extra":true,"request":{"method":"open-session"}}),
            "broker-envelope",
            Value::Null,
        ),
        (
            json!({"protocol":3,"id":"7","request":{"method":"hello"}}),
            "broker-envelope",
            Value::Null,
        ),
    ] {
        let reply: Value =
            serde_json::from_slice(&broker.reply(&serde_json::to_vec(&input).unwrap()).unwrap())
                .unwrap();
        assert_eq!(reply["error"]["code"], code);
        assert_eq!(reply["id"], id);
    }
    let opened = open(&mut broker);
    let rejected = inner(
        &mut broker,
        &opened["sessionEpoch"],
        json!({"method":"hello","extra":true}),
    );
    assert_eq!(rejected["error"]["code"], "protocol-request");
    assert_eq!(rejected["id"], "18446744073709551615");
    for _ in 0..3 {
        open(&mut broker);
    }
    assert_eq!(
        call(&mut broker, json!({"method":"open-session"}))["error"]["code"],
        "busy"
    );
}
struct Producer {
    permit: CompletionPermit,
    cancel: CancellationToken,
    release: mpsc::Sender<()>,
}
impl Drop for Producer {
    fn drop(&mut self) {
        self.permit.finish_cancelled();
        let _ = self.release.send(());
    }
}
struct Held(mpsc::Sender<Producer>);
impl DeferredNode for Held {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.held", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        _: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let (release, gate) = mpsc::channel();
        let _ = self.0.send(Producer {
            permit,
            cancel,
            release,
        });
        let _ = gate.recv();
        Ok(())
    }
}
fn held_registry(sender: mpsc::Sender<Producer>) -> Registry {
    let mut r = Registry::new();
    r.register_deferred(Held(sender)).unwrap();
    r
}
#[test]
fn reconnect_keeps_live_work_and_budget_rejects_old_keys_and_does_not_replay() {
    let (host, clock, service) = service("broker-reconnect");
    let (entered, producer) = mpsc::channel();
    let mut broker = Broker::new(&service, held_registry(entered));
    let old = open(&mut broker);
    let sibling = open(&mut broker);
    prepare(&mut broker, &old["sessionEpoch"], "test.held");
    let accepted = inner(&mut broker, &old["sessionEpoch"], start());
    let work = producer.recv_timeout(Duration::from_secs(5)).unwrap();
    let key = accepted["data"]["key"].clone();
    ok(
        &mut broker,
        json!({"method":"close-session","sessionEpoch":old["sessionEpoch"]}),
    );
    until(|| work.cancel.is_cancelled());
    let fresh = open(&mut broker);
    assert_eq!(fresh["resourceDomain"], old["resourceDomain"]);
    assert_ne!(fresh["sessionEpoch"], old["sessionEpoch"]);
    let rejected = inner(
        &mut broker,
        &fresh["sessionEpoch"],
        json!({"method":"get-run","key":key}),
    );
    assert_eq!(rejected["error"]["code"], "session-conflict");
    let unseen = inner(
        &mut broker,
        &fresh["sessionEpoch"],
        json!({"method":"get-start","startKey":"1"}),
    );
    assert_eq!(unseen["error"]["code"], "start-not-seen");
    assert_eq!(
        inner(
            &mut broker,
            &sibling["sessionEpoch"],
            json!({"method":"status"})
        )["state"]["revision"],
        "0"
    );
    prepare(&mut broker, &fresh["sessionEpoch"], "test.held");
    assert_eq!(
        inner(&mut broker, &fresh["sessionEpoch"], start())["error"]["code"],
        "busy"
    );
    assert_eq!(host.usage().physical_jobs, 1);
    assert!(
        producer.try_recv().is_err(),
        "reconnect must not replay old work"
    );
    work.permit.finish_cancelled();
    assert_eq!(host.usage().physical_jobs, 1);
    work.release.send(()).unwrap();
    until(|| host.usage().physical_jobs == 0);
    let retry = inner(&mut broker, &fresh["sessionEpoch"], start());
    assert_eq!(retry["ok"], true, "{retry}");
    let next = producer.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(producer.try_recv().is_err());
    drop(broker);
    until(|| next.cancel.is_cancelled());
    drop(next);
    until(|| host.usage().physical_jobs == 0 && host.usage().active_runs == 0);
    clock.advance_to(Duration::from_secs(5)).unwrap();
    until(|| host.usage().retained_runs == 0);
}
struct FlushCount {
    bytes: Vec<u8>,
    flushes: usize,
}
impl Write for FlushCount {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}
#[test]
fn framing_flushes_exact_crlf_and_refuses_oversized_suffix_and_handles_final_frame() {
    let (_, _, service) = service("broker-framing");
    let mut exact = frame(json!({"method":"open-session"}));
    exact.resize(MAX_REQUEST_BYTES, b' ');
    exact.extend_from_slice(b"\r\n");
    let mut malformed = b"{\n".to_vec();
    malformed.extend(frame(json!({"method":"open-session"})));
    let mut oversized = vec![b'x'; MAX_REQUEST_BYTES + 3];
    oversized.push(b'\n');
    oversized.extend(frame(json!({"method":"open-session"})));
    for (bytes, count, last_error) in [
        (exact, 1, None),
        (malformed, 2, None),
        (oversized, 1, Some("request-limit")),
    ] {
        let mut output = FlushCount {
            bytes: vec![],
            flushes: 0,
        };
        broker::serve(&service, registry(), Cursor::new(bytes), &mut output).unwrap();
        let replies: Vec<Value> = output
            .bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        assert_eq!(replies.len(), count);
        assert_eq!(output.flushes, count);
        if let Some(code) = last_error {
            assert_eq!(replies[0]["error"]["code"], code);
        }
    }
}
// An interactive test pipe allows real epochs to be read before subsequent
// frames are sent. Only the test transport is synthetic; the Host/worker is real.
enum Input {
    Frame(Vec<u8>),
    Fail,
}
struct PipeRead {
    input: mpsc::Receiver<Input>,
    cursor: Cursor<Vec<u8>>,
}
impl Read for PipeRead {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let bytes = self.fill_buf()?;
        let n = bytes.len().min(out.len());
        out[..n].copy_from_slice(&bytes[..n]);
        self.consume(n);
        Ok(n)
    }
}
impl BufRead for PipeRead {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.cursor.position() as usize == self.cursor.get_ref().len() {
            match self.input.recv() {
                Ok(Input::Frame(bytes)) => self.cursor = Cursor::new(bytes),
                Ok(Input::Fail) => return Err(io::Error::other("test read fault")),
                Err(_) => return Ok(&[]),
            }
        }
        self.cursor.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.cursor.consume(n);
    }
}
struct PipeWrite {
    output: mpsc::Sender<Value>,
    bytes: Vec<u8>,
    fault: Arc<AtomicU8>,
}
impl Write for PipeWrite {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fault.load(Ordering::Acquire) == 1 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test write fault",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fault.load(Ordering::Acquire) == 2 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test flush fault",
            ));
        }
        let reply = serde_json::from_slice(&self.bytes).unwrap();
        self.bytes.clear();
        let _ = self.output.send(reply);
        Ok(())
    }
}
struct PipeClient {
    input: Option<mpsc::Sender<Input>>,
    output: mpsc::Receiver<Value>,
    result: mpsc::Receiver<io::Result<()>>,
    fault: Arc<AtomicU8>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl PipeClient {
    fn new(registry: Registry) -> Self {
        let (send, input) = mpsc::channel();
        let (output, receive) = mpsc::channel();
        let (done, result) = mpsc::channel();
        let fault = Arc::new(AtomicU8::new(0));
        let writer = PipeWrite {
            output,
            bytes: vec![],
            fault: fault.clone(),
        };
        let thread = std::thread::spawn(move || {
            let outcome = broker::serve_host(
                policy(),
                registry,
                PipeRead {
                    input,
                    cursor: Cursor::new(vec![]),
                },
                writer,
            );
            let _ = done.send(outcome);
        });
        Self {
            input: Some(send),
            output: receive,
            result,
            fault,
            thread: Some(thread),
        }
    }
    fn send(&mut self, request: Value) {
        let mut bytes = frame(request);
        bytes.push(b'\n');
        self.input
            .as_ref()
            .unwrap()
            .send(Input::Frame(bytes))
            .unwrap();
    }
    fn call(&mut self, request: Value) -> Value {
        self.send(request);
        self.output.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    fn request(&mut self, epoch: &Value, request: Value) -> Value {
        let reply = self.call(wrapped(epoch, request));
        assert_eq!(reply["ok"], true, "{reply}");
        reply["data"].clone()
    }
}
impl Drop for PipeClient {
    fn drop(&mut self) {
        self.input.take();
    }
}
#[test]
fn standalone_broker_retains_producer_and_callback_after_every_transport_ending() {
    for fault in 0..4 {
        let (entered, producer) = mpsc::channel();
        let mut client = PipeClient::new(held_registry(entered));
        let opened = client.call(json!({"method":"open-session"}));
        let epoch = opened["data"]["sessionEpoch"].clone();
        let prepared=client.request(&epoch,json!({"method":"prepare","graph":{"version":1,"nodes":[{"id":"source","kind":"test.held","version":1}],"edges":[]}}));
        client.request(
            &epoch,
            json!({"method":"activate","token":prepared["data"]["token"]}),
        );
        assert_eq!(client.request(&epoch, start())["ok"], true);
        let work = producer.recv_timeout(Duration::from_secs(5)).unwrap();
        match fault {
            0 => {
                client.input.take();
            }
            1 => client.input.as_ref().unwrap().send(Input::Fail).unwrap(),
            2 | 3 => {
                client.fault.store(fault - 1, Ordering::Release);
                client.send(wrapped(&epoch, json!({"method":"status"})));
            }
            _ => unreachable!(),
        }
        until(|| work.cancel.is_cancelled());
        assert!(matches!(
            client.result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(work.permit.finish_cancelled());
        assert!(matches!(
            client.result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        work.release.send(()).unwrap();
        let outcome = client.result.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(outcome.is_err(), fault != 0);
        client.thread.take().unwrap().join().unwrap();
    }
}
