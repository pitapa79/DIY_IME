//! Non-default development feature. Grants are synthetic, never product evidence.
//! Uses the production broker/Job/receipt path, with no Node processing here.
use super::*;
use crate::{
    execution_control::RunKey,
    isolated_worker::{PreparationWorkerRequest, ResourceWorkerRequest, execute_resources},
};
use diy_ime2_wasm_resource_sdk::{
    ArtifactDescriptor, Mode, NodeIdentity, Operation, Request, ResourceDescriptor, ResourceMap,
};
use std::{collections::BTreeMap, fs};

const SOURCE_LIMIT: usize = 1024 * 1024;
const MODULE_LIMIT: usize = 32 * 1024 * 1024;
const ARTIFACT_LIMIT: usize = 8 * 1024 * 1024;
const MAX_PROBE_FUEL: u64 = 5_000_000_000;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProbeSpec {
    pub workspace: PathBuf,
    pub worker: PathBuf,
    pub runtime: PathBuf,
    pub producer: PathBuf,
    #[serde(default)]
    pub producer_fuel: Option<u64>,
    pub consumer: PathBuf,
    pub source: PathBuf,
    pub source_reference: String,
    pub node: NodeIdentity,
    pub config: Value,
    pub cases: Vec<ProbeCase>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProbeCase {
    pub inputs: Value,
    #[serde(default)]
    pub mode: Option<Mode>,
    #[serde(default)]
    pub cancel_before_start: bool,
}
pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|_| "probe asset open failed")?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "probe asset read failed")?;
    if bytes.len() > limit {
        return Err("probe asset exceeds finite limit".into());
    }
    Ok(bytes)
}
fn scope(run: usize) -> (ExecutionGate, GrantTarget, RunKey) {
    let key: RunKey = serde_json::from_value(json!({"sessionEpoch":"11111111111111111111111111111111","planVersion":"1","runId":run.to_string(),"deliveryGeneration":"1"})).unwrap();
    let target = GrantTarget::new(
        "synthetic-integration-fixture".into(),
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
        max_total_read_bytes: 32 * 1024 * 1024,
        max_reads: 4096,
        budget: Duration::from_secs(30),
    }
}
fn args(runtime: &Path, module: &Path, length: usize) -> Vec<OsString> {
    vec![
        "--resource-ranges-v1".into(),
        runtime.into(),
        module.into(),
        length.to_string().into(),
        "5000000000".into(),
        "134217728".into(),
    ]
}
fn new_control() -> RunControl {
    RunControl {
        walltime: Some(Duration::from_secs(30)),
        termination_grace: Duration::ZERO,
        ..RunControl::default()
    }
}
fn receipt(value: &ExitReceipt) -> Value {
    json!({"processId":value.process_id(),"exitCode":value.exit_code(),"reason":format!("{:?}",value.reason()),"inputDelivered":value.input_delivered(),"actualExitConfirmed":true})
}
fn core(error: CoreError) -> String {
    error.code
}
fn encode(request: Request<Value, Value>) -> Result<Vec<u8>, String> {
    request
        .validate()
        .map_err(|_| "invalid synthetic request descriptor")?;
    let bytes = serde_json::to_vec(&request).map_err(|_| "probe request encoding failed")?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("probe request exceeds finite limit".into());
    }
    Ok(bytes)
}

pub fn run(spec: ProbeSpec) -> Result<Value, String> {
    if spec.cases.is_empty() || spec.cases.len() > 64 {
        return Err("probe needs 1..64 cases".into());
    }
    let producer_fuel = spec.producer_fuel.unwrap_or(MAX_PROBE_FUEL);
    if !(1..=MAX_PROBE_FUEL).contains(&producer_fuel) {
        return Err("invalid probe producer fuel".into());
    }
    let original: Arc<[u8]> = read_bounded(&spec.source, SOURCE_LIMIT)?.into();
    let producer = read_bounded(&spec.producer, MODULE_LIMIT)?;
    let consumer = read_bounded(&spec.consumer, MODULE_LIMIT)?;
    // Runtime identity is a production pin, not a manifest self-claim.
    let dll: Arc<[u8]> = read_bounded(&spec.runtime, 128 * 1024 * 1024)?.into();
    if sha256(&dll).map_err(core)? != DLL_HASH {
        return Err("pinned runtime hash mismatch".into());
    }
    // CI retains native debug symbols; these are not Wasm input or guest memory.
    let worker: Arc<[u8]> = read_bounded(&spec.worker, 256 * 1024 * 1024)?.into();
    let producer_sha256 = sha256(&producer).map_err(core)?;
    let hashes = json!({"worker":sha256(&worker).map_err(core)?,"runtime":DLL_HASH,"producer":producer_sha256,"consumer":sha256(&consumer).map_err(core)?});
    let producer = WasmArtifact::capture_assets(
        &spec.workspace,
        || Ok(worker.clone()),
        || Ok(dll.clone()),
        producer,
        WorkerPolicy {
            fuel_per_call: Some(producer_fuel),
            termination_grace: Duration::ZERO,
            ..WorkerPolicy::bounded_probe()
        },
    )
    .map_err(core)?;
    let mut profile = Profile::new()?;
    let job = Job::with_memory_limit(Some(512 * 1024 * 1024))?;
    let cache = Cache::new(&spec.workspace, &profile)?;
    let program = cache.write("worker.exe", &worker)?;
    let runtime = cache.write("wasmtime.dll", &dll)?;
    let consumer_path = cache.write("consumer.wasm", &consumer)?;
    cache.allow_read_execute(&profile)?;
    let result = (|| {
        let control = new_control();
        let (gate, target, key) = scope(1);
        let length = original.len();
        let sources = PreparationSources::open(
            gate,
            target,
            key,
            control.cancel.clone(),
            BTreeMap::from([(spec.source_reference.clone(), original)]),
            limits(length),
        )
        .map_err(core)?;
        let identity = PreparationIdentity {
            producer_sha256,
            definition_sha256: sha256(&serde_json::to_vec(&spec.node).unwrap()).map_err(core)?,
            config_sha256: sha256(&serde_json::to_vec(&spec.config).unwrap()).map_err(core)?,
        };
        let input = encode(Request {
            api_version: 1,
            node: spec.node.clone(),
            operation: Operation::PrepareArtifact {
                config: spec.config.clone(),
                resources: ResourceMap(BTreeMap::from([(
                    spec.source_reference.clone(),
                    ResourceDescriptor {
                        id: 1,
                        length: length as u64,
                    },
                )])),
            },
        })?;
        let exited = producer
            .prepare_artifact(
                PreparationWorkerRequest {
                    input,
                    sources,
                    identity,
                    limits: PreparationLimits {
                        max_artifact_bytes: ARTIFACT_LIMIT,
                        max_chunks: 4096,
                        max_resident_bytes: 2 * ARTIFACT_LIMIT + 65536,
                    },
                },
                control.cancel,
            )
            .map_err(core)?;
        let prepare_receipt = receipt(exited.receipt());
        let artifact = match exited.into_artifact() {
            Ok(value) => value,
            Err(error) => {
                return Ok(
                    json!({"syntheticScope":true,"hashes":hashes,"producerFuel":producer_fuel,"preparation":prepare_receipt,"preparationError":error.code,"cases":[]}),
                );
            }
        };
        let descriptor = ArtifactDescriptor {
            resource: ResourceDescriptor {
                id: 1,
                length: artifact.bytes().len() as u64,
            },
            format: artifact.format().into(),
            version: artifact.format_version(),
        };
        let mut results = Vec::new();
        for (index, case) in spec.cases.into_iter().enumerate() {
            let control = new_control();
            let (gate, target, key) = scope(index + 2);
            let resources = ResourceRangeSession::open(
                gate,
                target,
                key,
                control.cancel.clone(),
                BTreeMap::from([("prepared.bin".into(), artifact.bytes().clone())]),
                limits(artifact.bytes().len()),
            )
            .map_err(core)?;
            let input = encode(Request {
                api_version: 1,
                node: spec.node.clone(),
                operation: Operation::ExecutePrepared {
                    config: spec.config.clone(),
                    artifact: descriptor.clone(),
                    inputs: case.inputs,
                    mode: case.mode,
                },
            })?;
            if case.cancel_before_start {
                control.cancel.cancel();
            }
            let attempt = execute_resources(
                &profile,
                &job,
                &cache,
                &program,
                &args(&runtime, &consumer_path, consumer.len()),
                ResourceWorkerRequest { input, resources },
                &control,
            );
            // The production range API checks the cancelled lease before launch;
            // unlike the ordinary pipe API it returns this explicit rejection.
            let mut outcome = match attempt {
                Ok(outcome) => outcome,
                Err(error)
                    if case.cancel_before_start && error == "resource invocation unavailable" =>
                {
                    results.push(json!({"cancelledBeforeStart":true,"processStarted":false,"nativeRejection":error}));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let value = loop {
                match outcome {
                    Outcome::CancelledBeforeStart => {
                        break json!({"cancelledBeforeStart":true,"processStarted":false});
                    }
                    Outcome::Unconfirmed(owner) => outcome = owner.retry(Duration::from_secs(1)),
                    Outcome::Exited(exit) => {
                        let output = if exit.reason() == EndReason::Exited
                            && exit.exit_code() == Some(0)
                            && exit.input_delivered()
                        {
                            match exit.output() {
                                Ok(bytes) => wire::reply(bytes).map_err(core),
                                Err(_) => Err("probe-transport".into()),
                            }
                        } else {
                            Err("probe-worker-exit".into())
                        };
                        break match output {
                            Ok(data) => json!({"receipt":receipt(&exit),"data":data}),
                            Err(error) => json!({"receipt":receipt(&exit),"error":error}),
                        };
                    }
                }
            };
            results.push(value);
        }
        Ok(
            json!({"syntheticScope":true,"hashes":hashes,"producerFuel":producer_fuel,"preparation":prepare_receipt,"artifact":{"bytes":artifact.bytes().len(),"sha256":artifact.artifact_sha256(),"format":artifact.format(),"version":artifact.format_version()},"cases":results}),
        )
    })();
    // Every live outcome was consumed, or dropped (blocking actual stop/join),
    // before this point. Never remove staging while a Worker can still use it.
    cache.revoke_read_execute(&profile)?;
    profile.delete()?;
    let parent = fs::canonicalize(spec.workspace.join("target/appcontainer-probes"))
        .map_err(|_| "probe cache parent unavailable")?;
    if cache.path().parent() != Some(parent.as_path()) {
        return Err("unexpected probe cache location".into());
    }
    for name in ["worker.exe", "wasmtime.dll", "consumer.wasm"] {
        remove_asset(&cache.path().join(name)).map_err(core)?;
    }
    fs::remove_dir(cache.path())
        .map_err(|error| format!("probe staging cleanup failed: {error}"))?;
    result
}
