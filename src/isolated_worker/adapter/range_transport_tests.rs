use super::{
    tests::{limits, resources, values},
    *,
};
use crate::isolated_worker::{
    resource_transport::{self as wire, Client, ResourceReader},
    *,
};
use std::io::{self, Cursor, Write};

fn session(cancel: CancellationToken) -> ResourceRangeSession {
    let (gate, target, key) = values();
    ResourceRangeSession::open(gate, target, key, cancel, resources(), limits()).unwrap()
}
fn start() -> Vec<u8> {
    let mut b = wire::MAGIC.to_vec();
    b.extend(2u32.to_le_bytes());
    b.extend(b"{}");
    b
}
fn read(id: u64, resource: u32, offset: u64, length: u32) -> Vec<u8> {
    let mut b = vec![1];
    b.extend(id.to_le_bytes());
    b.extend(resource.to_le_bytes());
    b.extend(offset.to_le_bytes());
    b.extend(length.to_le_bytes());
    b
}
fn data(id: u64, bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![2];
    b.extend(id.to_le_bytes());
    b.extend((bytes.len() as u32).to_le_bytes());
    b.extend(bytes);
    b
}
fn result(bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![3];
    b.extend((bytes.len() as u32).to_le_bytes());
    b.extend(bytes);
    b
}

#[test]
fn client_exact_frames_and_multiple_reads() {
    let mut host = start();
    host.extend(data(1, b"ab"));
    host.extend(data(2, b"78"));
    let mut output = Vec::new();
    let (mut client, input) = Client::start(Cursor::new(host), &mut output).unwrap();
    assert_eq!(input, b"{}");
    assert_eq!(client.read_resource(1, 0, 2).unwrap(), b"ab");
    assert_eq!(client.read_resource(2, 6, 2).unwrap(), b"78");
    client.finish(b"raw reply").unwrap();
    let mut expected = wire::MAGIC.to_vec();
    expected.extend(read(1, 1, 0, 2));
    expected.extend(read(2, 2, 6, 2));
    expected.extend(result(b"raw reply"));
    assert_eq!(output, expected);
}

#[test]
fn client_rejects_bad_version_truncation_and_oversized_start() {
    let mut oversized = wire::MAGIC.to_vec();
    oversized.extend(((wire::MAX_INPUT + 1) as u32).to_le_bytes());
    let mut bad = start();
    bad[7] = b'2';
    for bytes in [
        vec![],
        start()[..11].to_vec(),
        start()[..13].to_vec(),
        oversized,
        bad,
    ] {
        assert!(Client::start(Cursor::new(bytes), Vec::new()).is_err());
    }
}

#[test]
fn client_poisoned_after_bad_data_never_retries_or_finishes() {
    let mut badtag = data(1, b"ab");
    badtag[0] = 9;
    let mut huge = vec![2];
    huge.extend(1u64.to_le_bytes());
    huge.extend(65537u32.to_le_bytes());
    for bad in [
        badtag,
        data(2, b"ab"),
        data(1, b"a"),
        huge,
        data(1, b"ab")[..14].to_vec(),
        vec![],
    ] {
        let mut host = start();
        host.extend(bad);
        let mut output = Vec::new();
        let (mut client, _) = Client::start(Cursor::new(host), &mut output).unwrap();
        assert!(client.read_resource(1, 0, 2).is_err());
        assert!(client.read_resource(1, 0, 2).is_err());
        assert!(client.finish(b"success").is_err());
        assert_eq!(output.len(), 8 + 25);
    }
}

#[test]
fn client_rejects_invalid_local_requests_and_trailing_host_data() {
    for (id, length) in [(0, 1), (1, 0), (1, 65537)] {
        let (mut client, _) = Client::start(Cursor::new(start()), Vec::new()).unwrap();
        assert!(client.read_resource(id, 0, length).is_err());
        assert!(client.finish(b"ok").is_err());
    }
    let mut host = start();
    host.push(99);
    let (client, _) = Client::start(Cursor::new(host), Vec::new()).unwrap();
    assert!(client.finish(b"ok").is_err());
}

#[test]
fn host_serves_exact_ranges_and_preserves_raw_reply_bytes() {
    let mut child = wire::MAGIC.to_vec();
    child.extend(read(1, 1, 0, 2));
    child.extend(read(2, 2, 6, 2));
    child.extend(result(br#"{"a":1,"a":2}"#));
    let mut output = Vec::new();
    let reply = wire::serve(
        Cursor::new(child),
        &mut output,
        b"{}",
        &mut session(CancellationToken::default()),
    )
    .unwrap();
    assert_eq!(reply, br#"{"a":1,"a":2}"#);
    let mut expected = start();
    expected.extend(data(1, b"ab"));
    expected.extend(data(2, b"78"));
    assert_eq!(output, expected);
}

#[test]
fn host_rejects_unknown_order_duplicate_resource_and_range_frames() {
    for frame in [
        vec![9],
        read(0, 1, 0, 2),
        read(2, 1, 0, 2),
        read(1, 0, 0, 2),
        read(1, 3, 0, 2),
        read(1, 1, 0, 0),
        read(1, 1, u64::MAX, 2),
        read(1, 1, 7, 2),
        read(1, 1, 0, 65537),
        [read(1, 1, 0, 2), read(1, 1, 2, 2)].concat(),
    ] {
        let mut child = wire::MAGIC.to_vec();
        child.extend(frame);
        assert!(
            wire::serve(
                Cursor::new(child),
                Vec::new(),
                b"{}",
                &mut session(CancellationToken::default())
            )
            .is_err()
        );
    }
}

#[test]
fn host_rejects_every_truncation_and_trailing_result_data() {
    let mut child = wire::MAGIC.to_vec();
    child.extend(read(1, 1, 0, 2));
    child.extend(result(b"ok"));
    for end in 0..child.len() {
        assert!(
            wire::serve(
                Cursor::new(&child[..end]),
                Vec::new(),
                b"{}",
                &mut session(CancellationToken::default())
            )
            .is_err(),
            "truncation {end}"
        );
    }
    child.push(0);
    assert!(
        wire::serve(
            Cursor::new(child),
            Vec::new(),
            b"{}",
            &mut session(CancellationToken::default())
        )
        .is_err()
    );
    let mut huge = wire::MAGIC.to_vec();
    huge.push(3);
    huge.extend(((wire::MAX_INPUT + 1) as u32).to_le_bytes());
    assert!(
        wire::serve(
            Cursor::new(huge),
            Vec::new(),
            b"{}",
            &mut session(CancellationToken::default())
        )
        .is_err()
    );
}

struct RevokeWriter {
    gate: ExecutionGate,
    target: GrantTarget,
    count: usize,
    bytes: Vec<u8>,
}
impl Write for RevokeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend(bytes);
        self.count += bytes.len();
        if self.count >= 29 {
            self.gate.revoke(&self.target);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn revocation_between_reads_refuses_final_reply() {
    let (gate, target, key) = values();
    let cancel = CancellationToken::default();
    let mut session = ResourceRangeSession::open(
        gate.clone(),
        target.clone(),
        key,
        cancel.clone(),
        resources(),
        limits(),
    )
    .unwrap();
    let mut child = wire::MAGIC.to_vec();
    child.extend(read(1, 1, 0, 2));
    child.extend(read(2, 1, 2, 2));
    child.extend(result(b"ok"));
    let mut output = RevokeWriter {
        gate,
        target,
        count: 0,
        bytes: Vec::new(),
    };
    assert!(wire::serve(Cursor::new(child), &mut output, b"{}", &mut session).is_err());
    assert_eq!(output.bytes.len(), 29);
    assert!(cancel.is_cancelled());
}

fn wat(body: &str, imports: &str) -> Vec<u8> {
    let reply = br#"{"out":"abcd"}"#;
    let escaped: String = reply.iter().map(|b| format!("\\{b:02x}")).collect();
    format!(
        r#"(module
        {imports}
        (memory (export "memory") 1)
        (data (i32.const 128) "{escaped}")
        (func (export "diyime_api_version") (result i32) i32.const 1)
        (func (export "diyime_alloc") (param i32) (result i32) i32.const 4096)
        (func (export "diyime_dealloc") (param i32 i32))
        (func (export "diyime_release_reply"))
        (func (export "diyime_call") (param i32 i32) (result i64)
            {body}
            i64.const {}))"#,
        ((reply.len() as u64) << 32) | 128
    )
    .into_bytes()
}
const IMPORT: &str =
    r#"(import "diyime_resource_v1" "read" (func $read (param i32 i64 i32 i32) (result i32)))"#;
const TWO_READS: &str = r#"
    i32.const 1 i64.const 0 i32.const 2 i32.const 1024 call $read
    i32.const 2 i32.ne if unreachable end
    i32.const 1 i64.const 2 i32.const 2 i32.const 1026 call $read
    i32.const 2 i32.ne if unreachable end
    i32.const 1024 i32.load i32.const 1684234849 i32.ne if unreachable end
"#;

/// CI runs this explicitly after building the real native Worker and verifying
/// the pinned Wasmtime archive. No fake isolation or silent missing-tool skip.
#[test]
#[ignore = "requires real AppContainer and explicit pinned Worker/runtime paths"]
fn real_appcontainer_ranges() {
    let worker = std::path::PathBuf::from(
        std::env::var_os("DIY_RANGE_WORKER_BINARY").expect("Worker path required"),
    );
    let dll = std::path::PathBuf::from(
        std::env::var_os("DIY_RANGE_RUNTIME_DLL").expect("pinned DLL path required"),
    );
    let cases = [
        ("multiple", TWO_READS.to_owned(), IMPORT.to_owned(), true),
        (
            "memory-end",
            "i32.const 1 i64.const 0 i32.const 2 i32.const 65535 call $read drop".into(),
            IMPORT.into(),
            false,
        ),
        (
            "memory-wrap",
            "i32.const 1 i64.const 0 i32.const 2 i32.const -1 call $read drop".into(),
            IMPORT.into(),
            false,
        ),
        (
            "negative-offset",
            "i32.const 1 i64.const -1 i32.const 2 i32.const 1024 call $read drop".into(),
            IMPORT.into(),
            false,
        ),
        (
            "oversized",
            "i32.const 1 i64.const 0 i32.const 65537 i32.const 1024 call $read drop".into(),
            IMPORT.into(),
            false,
        ),
        (
            "unknown-resource",
            "i32.const 3 i64.const 0 i32.const 2 i32.const 1024 call $read drop".into(),
            IMPORT.into(),
            false,
        ),
        (
            "resource-end",
            "i32.const 1 i64.const 7 i32.const 2 i32.const 1024 call $read drop".into(),
            IMPORT.into(),
            false,
        ),
        (
            "unknown-import",
            "".into(),
            IMPORT.replace("\"read\"", "\"other\""),
            false,
        ),
        (
            "wrong-signature",
            "".into(),
            IMPORT.replace("(param i32 i64 i32 i32)", "(param i32)"),
            false,
        ),
        (
            "no-wasi",
            "".into(),
            r#"(import "wasi_snapshot_preview1" "path_open" (func))"#.into(),
            false,
        ),
        ("cancel-after-read", TWO_READS.into(), IMPORT.into(), false),
        ("revoke-after-read", TWO_READS.into(), IMPORT.into(), false),
        (
            "protected-after-read",
            TWO_READS.into(),
            IMPORT.into(),
            false,
        ),
        (
            "deadline",
            "(loop $busy br $busy)".into(),
            IMPORT.into(),
            false,
        ),
        ("v1-without-imports", "".into(), "".into(), true),
    ];
    let mut pending_exits = 0;
    for (name, body, imports, success) in cases {
        let profile = Profile::new().unwrap();
        let job = Job::new().unwrap();
        let cache = Cache::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
        let program = cache.copy(&worker, "worker.exe").unwrap();
        let runtime = cache.copy(&dll, "wasmtime.dll").unwrap();
        let module = wat(&body, &imports);
        let module_path = cache.write("fixture.wat", &module).unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let mut args: Vec<std::ffi::OsString> = vec![
            "--resource-ranges-v1".into(),
            runtime.into_os_string(),
            module_path.into_os_string(),
            module.len().to_string().into(),
            "50000000".into(),
            "134217728".into(),
        ];
        let control = RunControl {
            termination_grace: Duration::ZERO,
            ..RunControl::default()
        };
        if name == "multiple" {
            let wrong_cancel = CancellationToken::default();
            let mismatch = execute_resources(
                &profile,
                &job,
                &cache,
                &program,
                &args,
                ResourceWorkerRequest {
                    input: b"{}".to_vec(),
                    resources: session(wrong_cancel.clone()),
                },
                &control,
            );
            assert!(matches!(mismatch,Err(ref message) if message.contains("invocation owner")));
            assert!(wrong_cancel.is_cancelled());
            assert!(!control.cancel.is_cancelled());
        }
        let (gate, target, key) = values();
        let snapshots = resources();
        let weak = Arc::downgrade(snapshots.get("a.bin").unwrap());
        let mut policy = limits();
        if name == "deadline" {
            policy.budget = Duration::from_secs(2);
            args[4] = "none".into();
        }
        let mut resource_session = ResourceRangeSession::open(
            gate.clone(),
            target.clone(),
            key,
            control.cancel.clone(),
            snapshots,
            policy,
        )
        .unwrap();
        let hook_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if matches!(
            name,
            "cancel-after-read" | "revoke-after-read" | "protected-after-read"
        ) {
            let hook_called = hook_called.clone();
            let cancel = control.cancel.clone();
            resource_session.after_read = Some(Box::new(move || {
                hook_called.store(true, std::sync::atomic::Ordering::Release);
                match name {
                    "cancel-after-read" => cancel.cancel(),
                    "revoke-after-read" => gate.revoke(&target),
                    _ => gate.set_protection(false, true),
                }
            }));
        }
        let outcome = if name == "v1-without-imports" {
            args.remove(0);
            drop(resource_session);
            execute(
                &profile,
                &job,
                &cache,
                &program,
                &args,
                b"{}".to_vec(),
                &control,
            )
        } else {
            execute_resources(
                &profile,
                &job,
                &cache,
                &program,
                &args,
                ResourceWorkerRequest {
                    input: b"{}".to_vec(),
                    resources: resource_session,
                },
                &control,
            )
        }
        .unwrap();
        let receipt = match outcome {
            Outcome::Exited(r) => r,
            Outcome::Unconfirmed(r) => {
                pending_exits += 1;
                assert!(
                    weak.upgrade().is_some(),
                    "{name}: resource released before exit"
                );
                match r.retry(Duration::from_secs(10)) {
                    Outcome::Exited(r) => r,
                    _ => panic!("{name}: exit unconfirmed"),
                }
            }
            _ => panic!("{name}: not started"),
        };
        assert!(
            weak.upgrade().is_none(),
            "{name}: retained resources after actual exit/IO join"
        );
        if matches!(
            name,
            "cancel-after-read" | "revoke-after-read" | "protected-after-read"
        ) {
            assert!(
                hook_called.load(std::sync::atomic::Ordering::Acquire),
                "{name}: not cancelled during read"
            );
            assert!(control.cancel.is_cancelled());
        }
        if name == "deadline" {
            assert_eq!(receipt.reason(), EndReason::Deadline);
        }
        if success {
            assert_eq!(receipt.exit_code(), Some(0), "{name}: {receipt:?}");
            assert!(receipt.input_delivered(), "{name}: {receipt:?}");
            assert_eq!(receipt.output().unwrap(), br#"{"out":"abcd"}"#, "{name}");
        } else {
            assert!(receipt.output().is_err(), "{name}: {receipt:?}");
        }
        cache.revoke_read_execute(&profile).unwrap();
    }
    eprintln!(
        "range Worker: 15 cases passed; {pending_exits} original process owners retained until retry confirmed exit"
    );
}
