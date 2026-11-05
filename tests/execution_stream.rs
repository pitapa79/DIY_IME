use diy_ime2_core::{
    execution_control::{
        CancellationToken, CompletionPermit, DeferredNode, ExecutionClock, ExecutionHost,
        ExecutionService, OwnedInvocation, ResourcePolicy, protocol::MAX_REQUEST_BYTES, stream,
    },
    node::{CoreError, NodeDefinition, Port, Registry},
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Cursor, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
fn policy() -> ResourcePolicy {
    let mut policy =
        ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
    policy.max_physical_jobs = 1;
    policy.max_queued_jobs = 0;
    policy
}
fn service(name: &str) -> (ExecutionHost, ExecutionService) {
    let host = ExecutionHost::new(name, policy()).unwrap();
    let service =
        ExecutionService::with_clock(host.clone(), ExecutionClock::controlled(Duration::ZERO))
            .unwrap();
    (host, service)
}
fn line(id: u64, request: Value) -> Vec<u8> {
    let mut bytes =
        serde_json::to_vec(&json!({"protocol":3,"id":id.to_string(),"request":request})).unwrap();
    bytes.push(b'\n');
    bytes
}
fn requests() -> Vec<u8> {
    [line(1,json!({"method":"prepare","graph":{"version":1,"nodes":[{"id":"s","kind":"test.held","version":1}],"edges":[]}})),
        line(2,json!({"method":"activate","token":"1"})),
        line(3,json!({"method":"start-run","startKey":"1","expectedPlanVersion":"1","targets":[{"node":"s","port":"out"}]}))].concat()
}
fn replies(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}
fn until(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "stream ownership did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
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
struct Held {
    entered: mpsc::Sender<Producer>,
    started: Arc<AtomicBool>,
}
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
        self.started.store(true, Ordering::Release);
        let _ = self.entered.send(Producer {
            permit,
            cancel,
            release,
        });
        let _ = gate.recv();
        Ok(())
    }
}
fn held_registry(entered: mpsc::Sender<Producer>, started: Arc<AtomicBool>) -> Registry {
    let mut registry = Registry::new();
    registry
        .register_deferred(Held { entered, started })
        .unwrap();
    registry
}
struct AtEof {
    cursor: Cursor<Vec<u8>>,
    started: Arc<AtomicBool>,
    fail: bool,
}
impl Read for AtEof {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.cursor, bytes)
    }
}
impl BufRead for AtEof {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.cursor.position() as usize == self.cursor.get_ref().len() {
            until(|| self.started.load(Ordering::Acquire));
            if self.fail {
                return Err(io::Error::other("test read failure"));
            }
        }
        self.cursor.fill_buf()
    }
    fn consume(&mut self, count: usize) {
        self.cursor.consume(count);
    }
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
fn every_response_is_flushed_and_a_correlated_method_error_does_not_stop_the_stream() {
    let (_, service) = service("stream-flush");
    let bytes = [
        line(1, json!({"method":"hello"})),
        line(2, json!({"method":"unknown"})),
        line(3, json!({"method":"status"})),
    ]
    .concat();
    let mut output = FlushCount {
        bytes: vec![],
        flushes: 0,
    };
    stream::serve(&service, Registry::new(), Cursor::new(bytes), &mut output).unwrap();
    assert_eq!(output.flushes, 3);
    let replies = replies(&output.bytes);
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["id"], "1");
    assert_eq!(replies[1]["error"]["code"], "protocol-request");
    assert_eq!(replies[2]["ok"], true);
    assert_eq!(replies[0]["state"], replies[2]["state"]);
}
#[test]
fn exact_crlf_frame_is_accepted_but_an_oversized_lines_suffix_is_never_executed() {
    let (_, service) = service("stream-byte-boundaries");
    let mut exact = line(1, json!({"method":"hello"}));
    exact.pop();
    exact.resize(MAX_REQUEST_BYTES, b' ');
    exact.extend_from_slice(b"\r\n");
    let mut output = vec![];
    stream::serve(&service, Registry::new(), Cursor::new(exact), &mut output).unwrap();
    assert_eq!(replies(&output)[0]["ok"], true);
    let mut oversized = vec![b'x'; MAX_REQUEST_BYTES + 5];
    oversized.push(b'\n');
    oversized.extend(line(99, json!({"method":"hello"})));
    let mut output = vec![];
    stream::serve(
        &service,
        Registry::new(),
        Cursor::new(oversized),
        &mut output,
    )
    .unwrap();
    let replies = replies(&output);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], "request-limit");
    assert!(replies[0]["id"].is_null());
}
#[test]
fn malformed_json_is_one_error_and_final_unterminated_frame_is_still_one_request() {
    let (_, service) = service("stream-frames");
    let mut bytes = b"{\n".to_vec();
    let mut hello = line(5, json!({"method":"hello"}));
    hello.pop();
    bytes.extend(hello);
    let mut output = vec![];
    stream::serve(&service, Registry::new(), Cursor::new(bytes), &mut output).unwrap();
    let replies = replies(&output);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], "protocol-envelope");
    assert_eq!(replies[1]["id"], "5");
}
#[test]
fn standalone_host_waits_for_real_callback_return_after_eof_or_read_failure() {
    for fail in [false, true] {
        let (entered, producer) = mpsc::channel();
        let started = Arc::new(AtomicBool::new(false));
        let registry = held_registry(entered, started.clone());
        let reader = AtEof {
            cursor: Cursor::new(requests()),
            started,
            fail,
        };
        let (done, result) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut output = vec![];
            let outcome = stream::serve_host(policy(), registry, reader, &mut output);
            done.send((outcome, output)).unwrap();
        });
        let producer = producer.recv_timeout(Duration::from_secs(5)).unwrap();
        until(|| producer.cancel.is_cancelled());
        assert!(result.try_recv().is_err());
        assert!(producer.permit.finish_cancelled());
        assert!(
            result.try_recv().is_err(),
            "permit acknowledgement is not callback return"
        );
        producer.release.send(()).unwrap();
        let (outcome, bytes) = result.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(outcome.is_err(), fail);
        assert_eq!(replies(&bytes)[2]["data"]["state"], "accepted");
        thread.join().unwrap();
    }
}
struct FailOnAcceptance {
    lines: usize,
    started: Arc<AtomicBool>,
    fail_flush: bool,
}
impl Write for FailOnAcceptance {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.lines == 2 && !self.fail_flush {
            until(|| self.started.load(Ordering::Acquire));
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test lost acceptance",
            ));
        }
        self.lines += bytes.iter().filter(|b| **b == b'\n').count();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fail_flush && self.lines == 3 {
            until(|| self.started.load(Ordering::Acquire));
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "test lost flush"));
        }
        Ok(())
    }
}
#[test]
fn lost_output_closes_only_its_session_and_shared_host_capacity_stays_owned() {
    let (host, service) = service("stream-output-loss");
    let (entered, producer) = mpsc::channel();
    let started = Arc::new(AtomicBool::new(false));
    let registry = held_registry(entered.clone(), started.clone());
    let child_service = service.clone();
    let child = std::thread::spawn(move || {
        stream::serve(
            &child_service,
            registry,
            Cursor::new(requests()),
            FailOnAcceptance {
                lines: 0,
                started,
                fail_flush: false,
            },
        )
    });
    let producer = producer.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        child.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    until(|| producer.cancel.is_cancelled());
    assert_eq!(host.usage().physical_jobs, 1);
    let mut output = vec![];
    stream::serve(
        &service,
        held_registry(entered, Arc::new(AtomicBool::new(false))),
        Cursor::new(requests()),
        &mut output,
    )
    .unwrap();
    let replies = replies(&output);
    assert_eq!(replies[2]["error"]["code"], "busy");
    assert_eq!(host.usage().physical_jobs, 1);
    producer.permit.finish_cancelled();
    assert_eq!(host.usage().physical_jobs, 1);
    producer.release.send(()).unwrap();
    until(|| host.usage().physical_jobs == 0);
}

#[test]
fn standalone_host_waits_for_real_ending_after_write_or_flush_failure() {
    for fail_flush in [false, true] {
        let (entered, producer) = mpsc::channel();
        let started = Arc::new(AtomicBool::new(false));
        let registry = held_registry(entered, started.clone());
        let (done, result) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let outcome = stream::serve_host(
                policy(),
                registry,
                Cursor::new(requests()),
                FailOnAcceptance {
                    lines: 0,
                    started,
                    fail_flush,
                },
            );
            done.send(outcome).unwrap();
        });
        let producer = producer.recv_timeout(Duration::from_secs(5)).unwrap();
        until(|| producer.cancel.is_cancelled());
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(producer.permit.finish_cancelled());
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        producer.release.send(()).unwrap();
        let error = result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        thread.join().unwrap();
    }
}
