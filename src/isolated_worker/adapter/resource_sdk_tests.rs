//! Actual compiled SDK guest fixtures; no processing Node is registered here.
use super::prepared::{
    PreparationIdentity, PreparationLimits, PreparationSources, PreparedArtifact,
};
use super::*;
use crate::{
    execution_control::RunKey,
    isolated_worker::{
        PreparationOutcome, PreparationWorkerRequest, ResourceWorkerRequest, execute_preparation,
        execute_resources,
    },
};
use diy_ime2_wasm_resource_sdk::{
    ArtifactDescriptor, Mode, NodeIdentity, Operation, Request, ResourceDescriptor, ResourceMap,
};
use std::{collections::BTreeMap, fs};

fn scope() -> (ExecutionGate, GrantTarget, RunKey) {
    let key:RunKey=serde_json::from_value(json!({"sessionEpoch":"11111111111111111111111111111111","planVersion":"1","runId":"1","deliveryGeneration":"1"})).unwrap();
    let target = GrantTarget::new(
        "verified-sdk-fixture".into(),
        &key.session_epoch,
        key.plan_version,
        "fixture.resource".into(),
    );
    let gate = ExecutionGate::default();
    gate.approve(target.clone());
    (gate, target, key)
}
fn limits(length: usize) -> ResourceRangeLimits {
    ResourceRangeLimits {
        max_resources: 1,
        max_snapshot_bytes: length.max(1),
        max_read_bytes: 65536,
        max_total_read_bytes: length.max(1),
        max_reads: 8,
        budget: Duration::from_secs(30),
    }
}
fn producer_request(length: u64, id: u32) -> Value {
    serde_json::to_value(Request::<Value, Value> {
        api_version: 1,
        node: NodeIdentity {
            kind: "fixture.resource".into(),
            version: 1,
        },
        operation: Operation::PrepareArtifact {
            config: json!({}),
            resources: ResourceMap(BTreeMap::from([(
                "source.bin".into(),
                ResourceDescriptor { id, length },
            )])),
        },
    })
    .unwrap()
}
fn consumer_request(length: u64, id: u32) -> Value {
    serde_json::to_value(Request::<Value, Value> {
        api_version: 1,
        node: NodeIdentity {
            kind: "fixture.resource".into(),
            version: 1,
        },
        operation: Operation::ExecutePrepared {
            config: json!({}),
            inputs: json!({}),
            artifact: ArtifactDescriptor {
                resource: ResourceDescriptor { id, length },
                format: "fixture.sdk-bytes".into(),
                version: 1,
            },
            mode: Some(Mode::Finalize),
        },
    })
    .unwrap()
}
fn args(runtime: PathBuf, module: PathBuf, length: usize, building: bool) -> Vec<OsString> {
    vec![
        if building {
            "--build-resources-v1"
        } else {
            "--resource-ranges-v1"
        }
        .into(),
        runtime.into_os_string(),
        module.into_os_string(),
        length.to_string().into(),
        "50000000".into(),
        "134217728".into(),
    ]
}
struct Assets {
    worker: PathBuf,
    dll: PathBuf,
    producer: PathBuf,
    consumer: PathBuf,
}
fn produce(
    assets: &Assets,
    original: Arc<[u8]>,
    input: Vec<u8>,
) -> Result<PreparedArtifact, CoreError> {
    let profile = Profile::new().unwrap();
    let job = Job::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
    let program = cache.copy(&assets.worker, "worker.exe").unwrap();
    let runtime = cache.copy(&assets.dll, "wasmtime.dll").unwrap();
    let bytes = fs::read(&assets.producer).unwrap();
    let module = cache.write("module.wasm", &bytes).unwrap();
    cache.allow_read_execute(&profile).unwrap();
    let control = RunControl {
        termination_grace: Duration::ZERO,
        ..RunControl::default()
    };
    let (gate, target, key) = scope();
    let length = original.len();
    let sources = PreparationSources::open(
        gate,
        target,
        key,
        control.cancel.clone(),
        BTreeMap::from([("source.bin".into(), original)]),
        limits(length),
    )
    .unwrap();
    let identity = PreparationIdentity {
        producer_sha256: sha256(&bytes).unwrap(),
        definition_sha256: sha256(b"sdk-fixture-definition").unwrap(),
        config_sha256: sha256(b"{}").unwrap(),
    };
    let output = PreparationLimits {
        max_artifact_bytes: length.max(1),
        max_chunks: 8,
        max_resident_bytes: length.max(1) * 2 + 65536,
    };
    let outcome = execute_preparation(
        &profile,
        &job,
        &cache,
        &program,
        &args(runtime, module, bytes.len(), true),
        PreparationWorkerRequest {
            input,
            sources,
            identity,
            limits: output,
        },
        &control,
    )
    .unwrap();
    let exited = match outcome {
        PreparationOutcome::Exited(r) => r,
        PreparationOutcome::Unconfirmed(r) => match r.retry(Duration::from_secs(10)) {
            PreparationOutcome::Exited(r) => r,
            _ => panic!("SDK producer exit unconfirmed"),
        },
        _ => panic!("SDK producer not started"),
    };
    assert_eq!(
        fs::read_dir(cache.path()).unwrap().count(),
        3,
        "request/original persisted as cache asset"
    );
    let artifact = exited.into_artifact();
    cache.revoke_read_execute(&profile).unwrap();
    artifact
}
fn consume(
    assets: &Assets,
    data: Arc<[u8]>,
    input: Value,
    producer_in_reader: bool,
) -> Result<Value, String> {
    let profile = Profile::new().unwrap();
    let job = Job::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
    let program = cache.copy(&assets.worker, "worker.exe").unwrap();
    let runtime = cache.copy(&assets.dll, "wasmtime.dll").unwrap();
    let bytes = fs::read(if producer_in_reader {
        &assets.producer
    } else {
        &assets.consumer
    })
    .unwrap();
    let module = cache.write("module.wasm", &bytes).unwrap();
    cache.allow_read_execute(&profile).unwrap();
    let control = RunControl {
        termination_grace: Duration::ZERO,
        ..RunControl::default()
    };
    let (gate, target, key) = scope();
    let length = data.len();
    let resources = ResourceRangeSession::open(
        gate,
        target,
        key,
        control.cancel.clone(),
        BTreeMap::from([("prepared.bin".into(), data)]),
        limits(length),
    )
    .unwrap();
    let outcome = execute_resources(
        &profile,
        &job,
        &cache,
        &program,
        &args(runtime, module, bytes.len(), false),
        ResourceWorkerRequest {
            input: serde_json::to_vec(&input).unwrap(),
            resources,
        },
        &control,
    )
    .unwrap();
    let receipt = match outcome {
        Outcome::Exited(r) => r,
        Outcome::Unconfirmed(r) => match r.retry(Duration::from_secs(10)) {
            Outcome::Exited(r) => r,
            _ => panic!("SDK consumer exit unconfirmed"),
        },
        _ => panic!("SDK consumer not started"),
    };
    let result = if receipt.exit_code() == Some(0) && receipt.reason() == EndReason::Exited {
        receipt
            .output()
            .map_err(str::to_owned)
            .and_then(|bytes| wire::reply(bytes).map_err(|e| e.code))
    } else {
        Err("SDK guest refused".into())
    };
    assert_eq!(
        fs::read_dir(cache.path()).unwrap().count(),
        3,
        "invocation persisted in cache"
    );
    cache.revoke_read_execute(&profile).unwrap();
    result
}

#[test]
#[ignore = "requires compiled SDK producer/consumer Wasm and real native Worker/pinned DLL"]
fn real_resource_sdk_guests() {
    let required =
        |name| PathBuf::from(std::env::var_os(name).expect("explicit SDK fixture asset required"));
    let assets = Assets {
        worker: required("DIY_RANGE_WORKER_BINARY"),
        dll: required("DIY_RANGE_RUNTIME_DLL"),
        producer: required("DIY_RESOURCE_SDK_PRODUCER_WASM"),
        consumer: required("DIY_RESOURCE_SDK_CONSUMER_WASM"),
    };
    let small: Arc<[u8]> = Arc::from(&[0xe3, 0x81, 0x8b, 0xe3, 0x81, 0xaa, 0, 0xff][..]);
    let large: Arc<[u8]> = Arc::from((0..65553).map(|i| (i % 256) as u8).collect::<Vec<_>>());
    for data in [small.clone(), large] {
        let artifact = produce(
            &assets,
            data.clone(),
            serde_json::to_vec(&producer_request(data.len() as u64, 1)).unwrap(),
        )
        .unwrap();
        assert_eq!(artifact.bytes().as_ref(), data.as_ref());
        assert_eq!(artifact.format(), "fixture.sdk-bytes");
        let copied = consume(
            &assets,
            artifact.bytes().clone(),
            consumer_request(data.len() as u64, 1),
            false,
        )
        .unwrap();
        assert_eq!(copied, json!({"copied":data.as_ref()}));
    }
    let mut wrong_api = producer_request(8, 1);
    wrong_api["apiVersion"] = 2.into();
    assert!(
        produce(
            &assets,
            small.clone(),
            serde_json::to_vec(&wrong_api).unwrap()
        )
        .is_err()
    );
    let duplicate=br#"{"apiVersion":1,"node":{"kind":"fixture.resource","version":1},"operation":{"method":"prepareArtifact","config":{},"resources":{"source.bin":{"id":1,"length":8},"source.bin":{"id":1,"length":8}}}}"#;
    assert!(produce(&assets, small.clone(), duplicate.to_vec()).is_err());
    // Structurally valid guest claims cannot authorize a different native ID
    // or a range beyond the actual pinned resource.
    assert!(consume(&assets, small.clone(), consumer_request(8, 2), false).is_err());
    assert!(consume(&assets, small.clone(), consumer_request(9, 1), false).is_err());
    let mut null_mode = consumer_request(8, 1);
    null_mode["operation"]["mode"] = Value::Null;
    assert_eq!(
        consume(&assets, small.clone(), null_mode, false).unwrap_err(),
        "fixture-resource"
    );
    assert_eq!(
        consume(&assets, small, consumer_request(8, 1), true).unwrap_err(),
        "SDK guest refused",
        "producer must fail to instantiate, rather than reach its operation dispatcher"
    );
    eprintln!(
        "SDK guests: small/binary and multi-chunk roundtrips plus six refusals passed in real AppContainer"
    );
}
