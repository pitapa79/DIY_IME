//! Deterministic, bounded generated corpus against real v2/v3 APIs.
use diy_ime2_core::{
    execution_control::{
        ExecutionClock, ExecutionHost, ExecutionService, ResourcePolicy,
        protocol::MAX_RESPONSE_BYTES, session::SessionV3, stream,
    },
    node::{
        CoreError, Inputs, Node, NodeDefinition, Outputs, Port, PrepareResources, PreparedNode,
        Registry, RunContext,
    },
    session::{self, MAX_REQUEST_BYTES, Session},
};
use serde_json::{Value, json};
use std::{
    io::{self, BufReader, Cursor, Read},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
const SEED: u64 = 0xd1_79_20_26_11_01_24_93;
#[derive(Default)]
struct Calls {
    prepared: AtomicUsize,
    executed: AtomicUsize,
}
struct Probe(Arc<Calls>);
impl Node for Probe {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.probe", 1, vec![], vec![Port::packet("out")])
    }
    fn validate_config(&self, value: &Value) -> Result<(), CoreError> {
        if value.as_object().is_some_and(|map| map.is_empty()) {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "empty object required"))
        }
    }
    fn prepare(
        &self,
        _: &Value,
        _: &mut dyn PrepareResources,
    ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        self.0.prepared.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        self.0.executed.fetch_add(1, Ordering::SeqCst);
        Ok([(
            "out".into(),
            diy_ime2_core::node::OutputValue::Data(Arc::new(diy_ime2_core::Packet::new(
                diy_ime2_core::Candidate::Text {
                    text: "generated-normal-canary".into(),
                    meta: None,
                },
            ))),
        )]
        .into())
    }
}
fn registry(calls: &Arc<Calls>) -> Registry {
    let mut r = Registry::new();
    r.register(Probe(calls.clone())).unwrap();
    r
}
fn service() -> (ExecutionHost, ExecutionService) {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let p =
        ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
    let h = ExecutionHost::new(
        format!("generated-corpus-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        p,
    )
    .unwrap();
    let s = ExecutionService::with_clock(h.clone(), ExecutionClock::controlled(Duration::ZERO))
        .unwrap();
    (h, s)
}
fn frame(protocol: u32, request: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"protocol":protocol,"id":if protocol==2{json!(7)}else{json!("7")},"request":request})).unwrap()
}
fn status(protocol: u32) -> Vec<u8> {
    frame(protocol, json!({"method":"status"}))
}
fn append_line(wire: &mut Vec<u8>, frame: &[u8]) {
    wire.extend_from_slice(frame);
    wire.push(b'\n');
}
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
fn probe_graph() -> Value {
    json!({"version":1,"nodes":[{"id":"probe","kind":"test.probe","version":1}],"edges":[]})
}
fn v3_ok(session: &mut SessionV3, request: Value) -> Value {
    let reply: Value = serde_json::from_slice(&session.reply(&frame(3, request)).unwrap()).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    reply
}
fn corpus(protocol: u32) -> Vec<Vec<u8>> {
    let base = status(protocol);
    let mut cases = vec![
        base.clone(),
        vec![],
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        vec![0xff],
        vec![0xef, 0xbb, 0xbf],
    ];
    for end in 0..base.len() {
        cases.push(base[..end].to_vec());
    }
    for index in 0..base.len() {
        let mut removed = base.clone();
        removed.remove(index);
        cases.push(removed);
        for byte in [
            0, 13, 32, b'"', b'\\', b'{', b'}', b'[', b']', 0x7f, 0x80, 0xff,
        ] {
            let mut changed = base.clone();
            changed[index] = byte;
            cases.push(changed);
        }
    }
    let mut unicode = frame(protocol, json!({"method":"status","偽追加":"😀𠮷"}));
    for end in 0..unicode.len() {
        cases.push(unicode[..end].to_vec());
    }
    cases.push(std::mem::take(&mut unicode));
    for depth in [1, 16, 64, 127, 128, 256, 512] {
        let nested = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        let id = if protocol == 2 { "7" } else { "\"7\"" };
        cases.push(
            format!("{{\"protocol\":{protocol},\"id\":{id},\"request\":{nested}}}").into_bytes(),
        );
    }
    let graph =
        json!({"version":1,"nodes":[{"id":"probe","kind":"test.probe","version":1}],"edges":[]});
    for change in 0..5 {
        let mut graph = graph.clone();
        match change {
            0 => graph["version"] = json!(0),
            1 => graph["nodes"][0]["config"] = json!({"extra":true}),
            2 => {
                let duplicate = graph["nodes"][0].clone();
                graph["nodes"].as_array_mut().unwrap().push(duplicate)
            }
            3 => graph["nodes"][0]["permissions"] = json!(["*"]),
            _ => {
                graph["edges"] = json!([{"from":{"node":"probe","port":"missing"},"to":{"node":"probe","port":"missing"}}])
            }
        };
        cases.push(frame(protocol, json!({"method":"prepare","graph":graph})));
    }
    cases.push(frame(
        protocol,
        json!({"method":"prepare","graph":graph,"permissions":["*"]}),
    ));
    cases.push(frame(
        protocol,
        json!({"method":"activate","token":"unknown","trusted":true}),
    ));
    let mut seed = SEED;
    for _ in 0..512 {
        let length = (next(&mut seed) % 257) as usize;
        let mut bytes = vec![0; length];
        for byte in &mut bytes {
            *byte = next(&mut seed) as u8;
            if *byte == b'\n' {
                *byte = 0;
            }
        }
        cases.push(bytes);
    }
    assert!(cases.len() > 1000);
    assert!(
        cases
            .iter()
            .all(|bytes| !bytes.contains(&b'\n') && bytes.len() < 4096)
    );
    cases
}
fn replies(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            assert!(line.len() < MAX_RESPONSE_BYTES);
            serde_json::from_slice(line).unwrap()
        })
        .collect()
}
#[test]
fn generated_v2_frames_preserve_state_do_not_prepare_or_execute_and_recover_after_every_case() {
    let calls = Arc::new(Calls::default());
    let cases = corpus(2);
    let normal = status(2);
    let mut wire = vec![];
    append_line(
        &mut wire,
        &frame(2, json!({"method":"prepare","graph":probe_graph()})),
    );
    append_line(
        &mut wire,
        &frame(2, json!({"method":"activate","token":"1"})),
    );
    append_line(&mut wire, &normal);
    for case in &cases {
        append_line(&mut wire, case);
        append_line(&mut wire, &normal);
    }
    append_line(
        &mut wire,
        &frame(2, json!({"method":"run","expectedPlanVersion":1})),
    );
    let mut output = vec![];
    session::serve_session(
        Session::new(registry(&calls)),
        BufReader::with_capacity(7, Cursor::new(wire)),
        &mut output,
    )
    .unwrap();
    let rows = replies(&output);
    assert_eq!(rows.len(), 4 + 2 * cases.len());
    assert_eq!(rows[0]["ok"], true);
    assert_eq!(rows[1]["ok"], true);
    let initial = &rows[2]["state"];
    for (index, row) in rows.iter().skip(2).take(1 + 2 * cases.len()).enumerate() {
        assert_eq!(&row["state"], initial, "case/response {index}");
        if index % 2 == 0 {
            assert_eq!(row["ok"], true);
        }
    }
    assert_eq!(rows.last().unwrap()["ok"], true);
    assert_eq!(rows.last().unwrap()["data"]["succeeded"], true);
    assert_eq!(
        rows.last().unwrap()["data"]["nodes"]["probe"]["outputs"]["out"]["packet"]["root"]["text"],
        "generated-normal-canary"
    );
    assert_eq!(calls.prepared.load(Ordering::SeqCst), 1);
    assert_eq!(calls.executed.load(Ordering::SeqCst), 1);
    println!("v2 seed={SEED:016x} cases={}", cases.len());
}
#[test]
fn generated_v3_frames_preserve_state_resources_and_real_worker_admission() {
    let calls = Arc::new(Calls::default());
    let (host, service) = service();
    let mut session = SessionV3::new(&service, registry(&calls)).unwrap();
    let prepared = v3_ok(
        &mut session,
        json!({"method":"prepare","graph":probe_graph()}),
    );
    v3_ok(
        &mut session,
        json!({"method":"activate","token":prepared["data"]["token"]}),
    );
    let normal = status(3);
    let initial: Value = serde_json::from_slice(&session.reply(&normal).unwrap()).unwrap();
    let usage = host.usage();
    let cases = corpus(3);
    for (index, case) in cases.iter().enumerate() {
        let raw = session.reply(case).unwrap();
        assert!(raw.len() < MAX_RESPONSE_BYTES);
        let row: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(row["state"], initial["state"], "case {index}");
        let recovered: Value = serde_json::from_slice(&session.reply(&normal).unwrap()).unwrap();
        assert_eq!(recovered["ok"], true);
        assert_eq!(recovered["state"], initial["state"]);
        assert_eq!(host.usage(), usage);
    }
    assert_eq!(calls.prepared.load(Ordering::SeqCst), 1);
    assert_eq!(calls.executed.load(Ordering::SeqCst), 0);
    let accepted = v3_ok(
        &mut session,
        json!({"method":"start-run","startKey":"1","expectedPlanVersion":"1","targets":[{"node":"probe","port":"out"}]}),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let result = v3_ok(
            &mut session,
            json!({"method":"get-run","key":accepted["data"]["key"]}),
        );
        if result["data"]["state"] == "terminal" && host.usage().physical_jobs == 0 {
            assert_eq!(result["data"]["result"]["delivery"], "complete");
            assert_eq!(
                result["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
                "generated-normal-canary"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "normal worker did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(calls.prepared.load(Ordering::SeqCst), 1);
    assert_eq!(calls.executed.load(Ordering::SeqCst), 1);
    println!("v3 seed={SEED:016x} cases={}", cases.len());
}
fn framed(protocol: u32, wire: &[u8], chunk: usize, service: &ExecutionService) -> Vec<Value> {
    let mut output = vec![];
    let reader = BufReader::with_capacity(chunk, Cursor::new(wire));
    if protocol == 2 {
        session::serve_session(Session::new(Registry::new()), reader, &mut output).unwrap();
    } else {
        stream::serve(service, Registry::new(), reader, &mut output).unwrap();
    }
    replies(&output)
}
#[test]
fn every_truncated_unicode_prefix_and_fragmented_transport_recovers_at_the_next_line() {
    let (_, service) = service();
    for protocol in [2, 3] {
        let seed = frame(protocol, json!({"method":"status","偽追加":"😀𠮷"}));
        let good = status(protocol);
        for chunk in [1, 2, 7, 64] {
            for end in 0..seed.len() {
                let mut wire = vec![];
                append_line(&mut wire, &seed[..end]);
                append_line(&mut wire, &good);
                let rows = framed(protocol, &wire, chunk, &service);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0]["ok"], false, "v{protocol} chunk{chunk} prefix{end}");
                assert_eq!(rows[1]["ok"], true);
            }
        }
        assert!(framed(protocol, &[], 1, &service).is_empty());
        let rows = framed(protocol, &good, 1, &service);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ok"], true);
    }
}
#[test]
fn exact_limits_and_oversized_suffixes_preserve_each_existing_transport_boundary() {
    let (_, service) = service();
    for protocol in [2, 3] {
        let good = status(protocol);
        let mut exact = good.clone();
        exact.resize(MAX_REQUEST_BYTES, b' ');
        let mut lf = exact.clone();
        lf.push(b'\n');
        assert_eq!(framed(protocol, &lf, 7, &service)[0]["ok"], true);
        let mut crlf = exact;
        crlf.extend_from_slice(b"\r\n");
        let rows = framed(protocol, &crlf, 7, &service);
        assert_eq!(rows.len(), 1);
        if protocol == 3 {
            assert_eq!(rows[0]["ok"], true);
        } else {
            assert_eq!(rows[0]["error"]["code"], "request-limit");
        }
        let mut oversized = vec![b'x'; MAX_REQUEST_BYTES + 1];
        append_line(&mut oversized, &good);
        append_line(&mut oversized, &good);
        let rows = framed(protocol, &oversized, 7, &service);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["error"]["code"], "request-limit");
    }
}
struct CutOff {
    bytes: Cursor<Vec<u8>>,
    limit: usize,
}
impl Read for CutOff {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let pos = self.bytes.position() as usize;
        if pos >= self.limit {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "fake transport cut",
            ));
        }
        let size = out.len().min(self.limit - pos);
        self.bytes.read(&mut out[..size])
    }
}
#[test]
fn read_failure_after_a_complete_reply_does_not_publish_a_partial_or_following_reply() {
    let (host, service) = service();
    for protocol in [2, 3] {
        let good = status(protocol);
        let mut wire = vec![];
        append_line(&mut wire, &good);
        append_line(&mut wire, &good);
        append_line(&mut wire, &good);
        let limit = good.len() + 1 + good.len() / 2;
        let reader = BufReader::with_capacity(
            1,
            CutOff {
                bytes: Cursor::new(wire),
                limit,
            },
        );
        let mut output = vec![];
        let result = if protocol == 2 {
            session::serve_session(Session::new(Registry::new()), reader, &mut output)
        } else {
            stream::serve(&service, Registry::new(), reader, &mut output)
        };
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
        let rows = replies(&output);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ok"], true);
        let mut next = good.clone();
        next.push(b'\n');
        assert_eq!(framed(protocol, &next, 1, &service)[0]["ok"], true);
        assert_eq!(host.usage(), Default::default());
    }
}
