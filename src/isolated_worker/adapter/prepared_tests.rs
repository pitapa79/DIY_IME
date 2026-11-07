use super::*;
use crate::isolated_worker::{
    resource_transport::{self as wire, Client, ResourceReader},
    *,
};
use std::{io::Cursor, time::Duration};

fn limits() -> PreparationLimits {
    PreparationLimits {
        max_artifact_bytes: 8,
        max_chunks: 4,
        max_resident_bytes: 65552,
    }
}
fn sources(cancel: CancellationToken) -> (PreparationSources, ExecutionGate, GrantTarget) {
    source_bytes(cancel, Arc::from(&b"abcdefgh"[..]))
}
fn source_bytes(
    cancel: CancellationToken,
    original: Arc<[u8]>,
) -> (PreparationSources, ExecutionGate, GrantTarget) {
    let key:RunKey=serde_json::from_value(serde_json::json!({"sessionEpoch":"11111111111111111111111111111111","planVersion":"1","runId":"1","deliveryGeneration":"1"})).unwrap();
    let target = GrantTarget::new(
        "fixture-code".into(),
        &key.session_epoch,
        key.plan_version,
        "dictionary".into(),
    );
    let gate = ExecutionGate::default();
    gate.approve(target.clone());
    let sources = PreparationSources::open(
        gate.clone(),
        target.clone(),
        key,
        cancel,
        BTreeMap::from([("source.bin".into(), original)]),
        ResourceRangeLimits {
            max_resources: 1,
            max_snapshot_bytes: 8,
            max_read_bytes: 4,
            max_total_read_bytes: 8,
            max_reads: 4,
            budget: Duration::from_secs(30),
        },
    )
    .unwrap();
    (sources, gate, target)
}
fn start() -> Vec<u8> {
    let mut b = b"DIYBLD01".to_vec();
    b.extend(2u32.to_le_bytes());
    b.extend(b"{}");
    b
}
fn chunk(offset: u64, bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![4];
    b.extend(offset.to_le_bytes());
    b.extend((bytes.len() as u32).to_le_bytes());
    b.extend(bytes);
    b
}
fn ack(offset: u64, length: u32) -> Vec<u8> {
    let mut b = vec![5];
    b.extend(offset.to_le_bytes());
    b.extend(length.to_le_bytes());
    b
}
fn result(bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![3];
    b.extend((bytes.len() as u32).to_le_bytes());
    b.extend(bytes);
    b
}
fn data(id: u64, bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![2];
    b.extend(id.to_le_bytes());
    b.extend((bytes.len() as u32).to_le_bytes());
    b.extend(bytes);
    b
}

#[test]
fn artifact_budgets_include_finalization_and_chunk_storage() {
    for bad in [
        PreparationLimits {
            max_artifact_bytes: 0,
            ..limits()
        },
        PreparationLimits {
            max_chunks: 0,
            ..limits()
        },
        PreparationLimits {
            max_resident_bytes: 65551,
            ..limits()
        },
        PreparationLimits {
            max_artifact_bytes: usize::MAX,
            max_resident_bytes: usize::MAX,
            ..limits()
        },
    ] {
        assert!(ArtifactCapture::new(bad).is_err());
    }
    let mut capture = ArtifactCapture::new(limits()).unwrap();
    capture.reserve(0, 4).unwrap();
    capture.append(b"abcd").unwrap();
    capture.reserve(4, 4).unwrap();
    capture.append(b"efgh").unwrap();
    capture.complete().unwrap();
    assert_eq!(capture.bytes, b"abcdefgh");
    assert!(capture.reserve(8, 1).is_err());
}
#[test]
fn chunks_reject_missing_duplicate_truncated_and_out_of_budget_data() {
    for (offset, length) in [(1, 2), (u64::MAX, 1), (0, 0), (0, 9), (0, 65537)] {
        assert!(
            ArtifactCapture::new(limits())
                .unwrap()
                .reserve(offset, length)
                .is_err()
        );
    }
    let mut capture = ArtifactCapture::new(PreparationLimits {
        max_chunks: 1,
        ..limits()
    })
    .unwrap();
    assert!(capture.complete().is_err());
    capture.reserve(0, 2).unwrap();
    assert!(capture.reserve(0, 2).is_err());
    capture.append(b"ab").unwrap();
    assert!(capture.reserve(2, 1).is_err());
    let mut capture = ArtifactCapture::new(limits()).unwrap();
    capture.reserve(0, 2).unwrap();
    assert!(capture.append(b"a").is_err());
}
#[test]
fn client_interleaves_reads_and_chunks_without_reusing_offsets() {
    let mut host = start();
    host.extend(data(1, b"ab"));
    host.extend(ack(0, 2));
    host.extend(data(2, b"cd"));
    host.extend(ack(2, 2));
    let mut output = Vec::new();
    let (mut client, _) = Client::start_build(Cursor::new(host), &mut output).unwrap();
    let bytes = client.read_resource(1, 0, 2).unwrap();
    client.emit_artifact(&bytes).unwrap();
    let bytes = client.read_resource(1, 2, 2).unwrap();
    client.emit_artifact(&bytes).unwrap();
    client.finish(b"raw").unwrap();
    assert!(output.starts_with(b"DIYBLD01"));
    assert!(output.windows(15).any(|b| b == chunk(0, b"ab")));
    assert!(output.windows(15).any(|b| b == chunk(2, b"cd")));
}
#[test]
fn client_poisoned_by_wrong_ack_and_read_only_mode_denies_emission() {
    let mut wrong = ack(0, 2);
    wrong[0] = 9;
    for bytes in [
        wrong,
        ack(1, 2),
        ack(0, 1),
        ack(0, 2)[..12].to_vec(),
        vec![],
    ] {
        let mut host = start();
        host.extend(bytes);
        let (mut client, _) = Client::start_build(Cursor::new(host), Vec::new()).unwrap();
        assert!(client.emit_artifact(b"ab").is_err());
        assert!(client.emit_artifact(b"ab").is_err());
        assert!(client.finish(b"ok").is_err());
    }
    let mut host = wire::MAGIC.to_vec();
    host.extend(2u32.to_le_bytes());
    host.extend(b"{}");
    let (mut client, _) = Client::start(Cursor::new(host), Vec::new()).unwrap();
    assert!(client.emit_artifact(b"ab").is_err());
    assert!(client.finish(b"ok").is_err());
}
#[test]
fn host_collects_opaque_chunks_and_refuses_every_truncation() {
    let mut child = b"DIYBLD01".to_vec();
    child.extend(chunk(0, b"ab"));
    child.extend(chunk(2, b"cd"));
    child.extend(result(b"raw"));
    let mut capture = ArtifactCapture::new(limits()).unwrap();
    let (mut source, _, _) = sources(CancellationToken::default());
    assert_eq!(
        wire::serve_build(
            Cursor::new(&child),
            Vec::new(),
            b"{}",
            &mut source.ranges,
            &mut capture
        )
        .unwrap(),
        b"raw"
    );
    assert_eq!(capture.bytes, b"abcd");
    assert!(capture.completed);
    for end in 0..child.len() {
        let (mut source, _, _) = sources(CancellationToken::default());
        let mut capture = ArtifactCapture::new(limits()).unwrap();
        assert!(
            wire::serve_build(
                Cursor::new(&child[..end]),
                Vec::new(),
                b"{}",
                &mut source.ranges,
                &mut capture
            )
            .is_err(),
            "truncated {end}"
        );
    }
}
#[test]
fn host_rejects_duplicate_offset_budget_excess_and_trailing_bytes() {
    for frames in [
        [chunk(1, b"ab"), result(b"raw")].concat(),
        [chunk(0, b"ab"), chunk(0, b"cd"), result(b"raw")].concat(),
        [chunk(0, b"abcdefgh"), chunk(8, b"x"), result(b"raw")].concat(),
        [chunk(0, b"ab"), result(b"raw"), vec![0]].concat(),
        result(b"no artifact"),
    ] {
        let mut child = b"DIYBLD01".to_vec();
        child.extend(frames);
        let (mut source, _, _) = sources(CancellationToken::default());
        let mut capture = ArtifactCapture::new(limits()).unwrap();
        assert!(
            wire::serve_build(
                Cursor::new(child),
                Vec::new(),
                b"{}",
                &mut source.ranges,
                &mut capture
            )
            .is_err()
        );
    }
}
#[test]
fn source_fingerprint_is_owned_and_execution_mode_rejects_builder_frames() {
    let (source, _, _) = sources(CancellationToken::default());
    let expected = source.source_sha256;
    assert_eq!(
        expected,
        sources(CancellationToken::default()).0.source_sha256
    );
    assert_ne!(
        expected,
        source_bytes(CancellationToken::default(), Arc::from(&b"abcdefg\xff"[..]))
            .0
            .source_sha256
    );
    let mut child = b"DIYBLD01".to_vec();
    child.extend(chunk(0, b"ab"));
    child.extend(result(b"raw"));
    let (mut source, _, _) = sources(CancellationToken::default());
    assert!(wire::serve(Cursor::new(child), Vec::new(), b"{}", &mut source.ranges).is_err());
}

fn fixture(body: &str, reply: &[u8]) -> Vec<u8> {
    let escaped: String = reply.iter().map(|b| format!("\\{b:02x}")).collect();
    format!(
        r#"(module
      (import "diyime_resource_v1" "read" (func $read (param i32 i64 i32 i32) (result i32)))
      (import "diyime_prepared_v1" "emit" (func $emit (param i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 128) "{escaped}")
      (data (i32.const 512) "abcdefghijklmnop")
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
const BUILD_BODY: &str = r#"
 i32.const 1 i64.const 0 i32.const 2 i32.const 1024 call $read drop
 i32.const 1024 i32.const 2 call $emit drop
 i32.const 1 i64.const 2 i32.const 2 i32.const 1026 call $read drop
 i32.const 1026 i32.const 2 call $emit drop
"#;

#[test]
#[ignore = "requires explicit real native Worker/pinned DLL/crash fixture paths and AppContainer"]
fn real_appcontainer_preparation() {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicBool, Ordering},
    };
    let worker =
        PathBuf::from(std::env::var_os("DIY_RANGE_WORKER_BINARY").expect("Worker path required"));
    let dll =
        PathBuf::from(std::env::var_os("DIY_RANGE_RUNTIME_DLL").expect("pinned DLL path required"));
    let crash = PathBuf::from(
        std::env::var_os("DIY_PREPARATION_CRASH_FIXTURE").expect("crash fixture path required"),
    );
    let names = [
        "normal",
        "wrong-length",
        "unprepared",
        "bad-format",
        "bad-json",
        "memory-end",
        "negative-length",
        "oversized",
        "over-budget",
        "chunk-budget",
        "cancel-after-chunk",
        "revoke-after-chunk",
        "protected-after-chunk",
        "crash-after-reply",
        "read-only",
        "no-artifact",
        "deadline",
    ];
    let mut pending = 0;
    for name in names {
        let profile = Profile::new().unwrap();
        let job = Job::new().unwrap();
        let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
        let mut reply = serde_json::json!({"apiVersion":1,"ok":true,"data":{"valid":true,"prepared":true,"artifactFormat":"fixture","artifactVersion":1,"artifactBytes":4}});
        match name {
            "wrong-length" => reply["data"]["artifactBytes"] = 3.into(),
            "unprepared" => reply["data"]["prepared"] = false.into(),
            "bad-format" => reply["data"]["artifactVersion"] = 0.into(),
            _ => (),
        }
        let reply = if name == "bad-json" {
            br#"{"apiVersion":1,"apiVersion":1,"ok":true,"data":{}}"#.to_vec()
        } else {
            serde_json::to_vec(&reply).unwrap()
        };
        let body = match name {
            "memory-end" => "i32.const 65535 i32.const 2 call $emit drop",
            "negative-length" => "i32.const 1024 i32.const -1 call $emit drop",
            "oversized" => "i32.const 0 i32.const 65537 call $emit drop",
            "over-budget" => "i32.const 512 i32.const 16 call $emit drop",
            "no-artifact" => "",
            "deadline" => "i32.const 512 i32.const 2 call $emit drop (loop $busy br $busy)",
            _ => BUILD_BODY,
        };
        let module = fixture(body, &reply);
        let module_path = cache.write("fixture.wat", &module).unwrap();
        let program = cache
            .copy(
                if name == "crash-after-reply" {
                    &crash
                } else {
                    &worker
                },
                "worker.exe",
            )
            .unwrap();
        let runtime = cache.copy(&dll, "wasmtime.dll").unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let identity = PreparationIdentity {
            producer_sha256: if name == "crash-after-reply" {
                sha256(&std::fs::read(&crash).unwrap())
            } else {
                sha256(&module)
            }
            .unwrap(),
            definition_sha256: sha256(b"fixed-fixture-definition").unwrap(),
            config_sha256: sha256(b"{}").unwrap(),
        };
        let control = RunControl {
            termination_grace: Duration::ZERO,
            walltime: Some(Duration::from_secs(if name == "deadline" { 2 } else { 30 })),
            ..RunControl::default()
        };
        let original: Arc<[u8]> = Arc::from(&b"abcdefgh"[..]);
        let weak = Arc::downgrade(&original);
        let (mut source, gate, target) = source_bytes(control.cancel.clone(), original);
        let source_hash = source.source_sha256.clone();
        let hook_called = Arc::new(AtomicBool::new(false));
        if matches!(
            name,
            "cancel-after-chunk" | "revoke-after-chunk" | "protected-after-chunk"
        ) {
            let flag = hook_called.clone();
            let cancel = control.cancel.clone();
            let mut reads = 0;
            source.ranges.after_read = Some(Box::new(move || {
                reads += 1;
                if reads == 2 {
                    flag.store(true, Ordering::Release);
                    match name {
                        "cancel-after-chunk" => cancel.cancel(),
                        "revoke-after-chunk" => gate.revoke(&target),
                        _ => gate.set_protection(true, false),
                    }
                }
            }));
        }
        let args: Vec<std::ffi::OsString> = if name == "crash-after-reply" {
            vec!["--preparation-crash".into()]
        } else {
            vec![
                if name == "read-only" {
                    "--resource-ranges-v1"
                } else {
                    "--build-resources-v1"
                }
                .into(),
                runtime.into_os_string(),
                module_path.into_os_string(),
                module.len().to_string().into(),
                if name == "deadline" {
                    "none"
                } else {
                    "50000000"
                }
                .into(),
                "134217728".into(),
            ]
        };
        if name == "read-only" {
            let outcome = execute_resources(
                &profile,
                &job,
                &cache,
                &program,
                &args,
                ResourceWorkerRequest {
                    input: b"{}".to_vec(),
                    resources: source.ranges,
                },
                &control,
            )
            .unwrap();
            let receipt = match outcome {
                Outcome::Exited(r) => r,
                Outcome::Unconfirmed(r) => match r.retry(Duration::from_secs(10)) {
                    Outcome::Exited(r) => r,
                    _ => panic!("read-only exit unconfirmed"),
                },
                _ => panic!("not started"),
            };
            assert!(
                receipt.output().is_err(),
                "builder import admitted in read-only mode"
            );
            cache.revoke_read_execute(&profile).unwrap();
            continue;
        }
        let mut output_limits = limits();
        if name == "chunk-budget" {
            output_limits.max_chunks = 1;
        }
        let outcome = execute_preparation(
            &profile,
            &job,
            &cache,
            &program,
            &args,
            PreparationWorkerRequest {
                input: b"{}".to_vec(),
                sources: source,
                identity: identity.clone(),
                limits: output_limits,
            },
            &control,
        )
        .unwrap();
        let exited = match outcome {
            PreparationOutcome::Exited(r) => r,
            PreparationOutcome::Unconfirmed(r) => {
                pending += 1;
                assert!(
                    weak.upgrade().is_some(),
                    "{name}: originals released before exit"
                );
                match r.retry(Duration::from_secs(10)) {
                    PreparationOutcome::Exited(r) => r,
                    _ => panic!("{name}: exit unconfirmed"),
                }
            }
            _ => panic!("{name}: not started"),
        };
        assert!(
            weak.upgrade().is_none(),
            "{name}: original not reclaimed after exit/IO join"
        );
        if name == "normal" {
            assert_eq!(exited.receipt().exit_code(), Some(0));
            let artifact = exited.into_artifact().unwrap();
            assert_eq!(artifact.bytes().as_ref(), b"abcd");
            assert_eq!(artifact.artifact_sha256(), sha256(b"abcd").unwrap());
            assert_eq!(artifact.source_sha256(), source_hash);
            assert_eq!(artifact.identity(), &identity);
            assert_eq!(artifact.format(), "fixture");
            assert_eq!(artifact.format_version(), 1);
        } else {
            assert!(
                exited.artifact().is_err(),
                "{name}: invalid construction accepted"
            );
            if name == "crash-after-reply" {
                assert_eq!(exited.receipt().exit_code(), Some(73));
                assert!(exited.receipt().output().is_ok());
            }
            if name == "deadline" {
                assert_eq!(exited.receipt().reason(), EndReason::Deadline);
            }
            if matches!(
                name,
                "cancel-after-chunk" | "revoke-after-chunk" | "protected-after-chunk"
            ) {
                assert!(
                    hook_called.load(Ordering::Acquire),
                    "{name}: partial-output withdrawal not exercised"
                );
            }
        }
        cache.revoke_read_execute(&profile).unwrap();
    }
    eprintln!(
        "preparation Worker: 17 cases passed; {pending} original process owners retained through confirmed exit"
    );
}
