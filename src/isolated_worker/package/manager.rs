//! Framed stdin package manager. CODE uploads only; no graph/IME input API.
use super::{Catalog, ImportConfirmation, ImportPlan, Package, PackageLimits, PackageVerifier};
use crate::{
    execution_control::CancellationToken,
    isolated_worker::adapter::{
        DictionaryBindingConfig, DictionaryBindings, ExecutionGate, WasmArtifact, WorkerPolicy,
    },
    node::CoreError,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagerConfig {
    #[serde(default)]
    official_root: Option<PathBuf>,
    data_root: PathBuf,
    workspace: PathBuf,
    worker: PathBuf,
    runtime_dll: PathBuf,
    policy: Policy,
    #[serde(default)]
    limits: PackageLimits,
    #[serde(default)]
    dictionary_bindings: BTreeMap<String, DictionaryBindingConfig>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Policy {
    #[serde(deserialize_with = "required_option")]
    process_memory_bytes: Option<usize>,
    #[serde(deserialize_with = "required_option")]
    wasm_memory_bytes: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    fuel_per_call: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    walltime_ms: Option<u64>,
    termination_grace_ms: u64,
}
fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}
impl ManagerConfig {
    pub(super) fn official_review_assets(&self) -> Option<(PathBuf, PackageLimits)> {
        self.official_root
            .clone()
            .map(|root| (root, self.limits.clone()))
    }
    pub(super) fn prepare_assets(&self) -> Result<(Catalog, PackageVerifier), CoreError> {
        for path in [&self.data_root, &self.workspace] {
            fs::create_dir_all(path)
                .map_err(|_| error("manager-config", "Host storage unavailable"))?;
        }
        let policy = WorkerPolicy {
            process_memory_bytes: self.policy.process_memory_bytes,
            wasm_memory_bytes: self.policy.wasm_memory_bytes,
            fuel_per_call: self.policy.fuel_per_call,
            walltime: self.policy.walltime_ms.map(Duration::from_millis),
            termination_grace: Duration::from_millis(self.policy.termination_grace_ms),
        };
        let runtime = WasmArtifact::capture(
            &self.workspace,
            &self.worker,
            &self.runtime_dll,
            b"\0asm\x01\0\0\0".to_vec(),
            policy.clone(),
        )?;
        let runtime = if self.official_root.is_some() {
            runtime.enable_official_compilation()
        } else {
            runtime
        };
        Ok((
            Catalog::open(&self.data_root, self.limits.clone())?,
            PackageVerifier::new(runtime, policy).with_dictionary_bindings(
                DictionaryBindings::open(self.dictionary_bindings.clone())?,
            ),
        ))
    }
    pub fn read(path: &std::path::Path) -> Result<Self, CoreError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| error("manager-config", "Host configuration unavailable"))?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| error("manager-config", "Host configuration read failed"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(error("manager-config", "Host configuration exceeds limit"));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| error("manager-config", "invalid Host configuration"))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    List {},
    Bootstrap {},
    Review {},
    Commit {
        review_id: String,
        allow_downgrade: bool,
        allow_same_version_rebuild: bool,
    },
    Discard {
        review_id: String,
    },
    Cancel {
        request_id: String,
    },
    Close {},
}
struct Work {
    request: Request,
    payload: Vec<u8>,
    cancel: CancellationToken,
}
#[derive(Default)]
struct Activity {
    active: Option<(String, CancellationToken)>,
}
type Output = Arc<Mutex<io::Stdout>>;
fn error(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
fn reply(output: &Output, id: &str, result: Result<Value, CoreError>) {
    let value = match result {
        Ok(data) => json!({"id":id,"ok":true,"data":data}),
        Err(e) => json!({"id":id,"ok":false,"error":{"code":e.code,"message":e.message}}),
    };
    let mut output = output.lock().unwrap_or_else(|p| p.into_inner());
    // A broken parent still triggers cancellation through stdin EOF.
    let _ = serde_json::to_writer(&mut *output, &value);
    let _ = output.write_all(b"\n");
    let _ = output.flush();
}
fn cancel_active(activity: &Mutex<Activity>) {
    if let Some((_, token)) = &activity.lock().unwrap_or_else(|p| p.into_inner()).active {
        token.cancel();
    }
}
fn request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn read_requests(
    output: Output,
    activity: Arc<Mutex<Activity>>,
    sender: mpsc::SyncSender<Work>,
    payload_limit: usize,
) {
    let mut input = io::stdin().lock();
    loop {
        let mut header = [0u8; 12];
        if input.read_exact(&mut header).is_err() {
            break;
        }
        let json_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let payload_len = u64::from_le_bytes(header[4..].try_into().unwrap());
        if json_len == 0 || json_len > 64 * 1024 || payload_len > payload_limit as u64 {
            reply(
                &output,
                "",
                Err(error(
                    "manager-frame",
                    "request frame exceeds selected limit",
                )),
            );
            break;
        }
        let mut json = vec![0; json_len];
        if input.read_exact(&mut json).is_err() {
            break;
        }
        let request: Request = match serde_json::from_slice(&json) {
            Ok(request) => request,
            Err(_) => {
                reply(
                    &output,
                    "",
                    Err(error("manager-request", "invalid request fields")),
                );
                break;
            }
        };
        if !request_id(&request.id) {
            reply(
                &output,
                "",
                Err(error("manager-request", "invalid request ID")),
            );
            break;
        }
        if !matches!(request.operation, Operation::Review {}) && payload_len != 0 {
            reply(
                &output,
                &request.id,
                Err(error("manager-frame", "unexpected request payload")),
            );
            break;
        }
        match &request.operation {
            Operation::Cancel { request_id } => {
                let state = activity.lock().unwrap_or_else(|p| p.into_inner());
                if let Some((active, token)) = &state.active
                    && active == request_id
                {
                    token.cancel();
                }
                drop(state);
                reply(&output, &request.id, Ok(json!({"cancelRequested":true})));
                continue;
            }
            Operation::Close {} => {
                cancel_active(&activity);
                let _ = sender.send(Work {
                    request,
                    payload: vec![],
                    cancel: CancellationToken::default(),
                });
                return;
            }
            _ => {}
        }
        let mut state = activity.lock().unwrap_or_else(|p| p.into_inner());
        if state.active.is_some() {
            drop(state);
            // Drain without allocating a second batch while an operation owns the slot.
            if io::copy(&mut input.by_ref().take(payload_len), &mut io::sink()).ok()
                != Some(payload_len)
            {
                break;
            }
            reply(
                &output,
                &request.id,
                Err(error(
                    "manager-busy",
                    "another package operation is still settling",
                )),
            );
            continue;
        }
        let cancel = CancellationToken::default();
        state.active = Some((request.id.clone(), cancel.clone()));
        drop(state);
        let mut payload = vec![0; payload_len as usize];
        if input.read_exact(&mut payload).is_err() {
            break;
        }
        if sender
            .send(Work {
                request,
                payload,
                cancel,
            })
            .is_err()
        {
            return;
        }
    }
    cancel_active(&activity);
    // EOF has no externally supplied identity and never starts another guest.
    let _ = sender.send(Work {
        request: Request {
            id: String::new(),
            operation: Operation::Close {},
        },
        payload: vec![],
        cancel: CancellationToken::default(),
    });
}
fn decode_packages(payload: Vec<u8>, limits: &PackageLimits) -> Result<Vec<Package>, CoreError> {
    // HTTP/IPC batch: u32 count, then each u64 length and exact file bytes.
    let mut cursor = io::Cursor::new(payload);
    let mut count = [0u8; 4];
    cursor
        .read_exact(&mut count)
        .map_err(|_| error("manager-frame", "truncated package count"))?;
    let count = u32::from_le_bytes(count) as usize;
    if count == 0 || count > (cursor.get_ref().len() - 4) / 8 {
        return Err(error("manager-frame", "invalid package count"));
    }
    let mut packages = Vec::new();
    let mut total = 0usize;
    for _ in 0..count {
        let mut length = [0u8; 8];
        cursor
            .read_exact(&mut length)
            .map_err(|_| error("manager-frame", "truncated package length"))?;
        let length = usize::try_from(u64::from_le_bytes(length))
            .map_err(|_| error("manager-frame", "package length overflow"))?;
        total = total
            .checked_add(length)
            .ok_or_else(|| error("manager-frame", "batch length overflow"))?;
        if length > limits.package_bytes
            || total > limits.batch_bytes
            || length
                > cursor
                    .get_ref()
                    .len()
                    .saturating_sub(cursor.position() as usize)
        {
            return Err(error("manager-frame", "package exceeds selected bounds"));
        }
        let mut bytes = vec![0; length];
        cursor
            .read_exact(&mut bytes)
            .map_err(|_| error("manager-frame", "truncated package"))?;
        packages.push(Package::parse(bytes, limits)?);
    }
    if cursor.position() != cursor.get_ref().len() as u64 {
        return Err(error("manager-frame", "trailing package batch data"));
    }
    Ok(packages)
}
pub(super) fn nonce() -> Result<String, CoreError> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let mut bytes = [0u8; 16];
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err(error("manager-identity", "review ID unavailable"));
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
/// Blocks on this Host process only. EOF/close cooperatively cancels the real
/// Worker and waits for its actual process/IO termination before returning.
pub fn run(config: ManagerConfig) -> Result<(), CoreError> {
    let (catalog, verifier) = config.prepare_assets()?;
    let output = Arc::new(Mutex::new(io::stdout()));
    let activity = Arc::new(Mutex::new(Activity::default()));
    let (sender, receiver) = mpsc::sync_channel(1);
    // At least eight bytes of each framed package are in the bounded batch;
    // framing overhead therefore fits within a second batch-sized allowance.
    let payload_limit = config
        .limits
        .batch_bytes
        .checked_mul(2)
        .and_then(|n| n.checked_add(4))
        .ok_or_else(|| error("manager-config", "batch framing limit overflow"))?;
    let reader = {
        let output = output.clone();
        let activity = activity.clone();
        std::thread::spawn(move || read_requests(output, activity, sender, payload_limit))
    };
    let gate = ExecutionGate::default();
    let mut reviewed: Option<(String, ImportPlan)> = None;
    while let Ok(work) = receiver.recv() {
        let id = &work.request.id;
        if matches!(work.request.operation, Operation::Close {}) {
            reviewed.take();
            if !id.is_empty() {
                reply(&output, id, Ok(json!({"closed":true})));
            }
            break;
        }
        let result = (|| {
            if work.cancel.is_cancelled() {
                return Err(error("manager-cancelled", "package operation cancelled"));
            }
            match work.request.operation {
                Operation::List {} => {
                    Ok(json!({"catalog":catalog.snapshot()?,"executionPermissionGranted":false}))
                }
                Operation::Bootstrap {} => {
                    if !work.payload.is_empty() {
                        return Err(error(
                            "manager-frame",
                            "official setup has no upload payload",
                        ));
                    }
                    let directory = config.official_root.as_deref().ok_or_else(|| {
                        error(
                            "official-unavailable",
                            "official distribution is not configured",
                        )
                    })?;
                    let report = super::official::bootstrap_batch(
                        directory,
                        &catalog,
                        &verifier,
                        &config.limits,
                        gate.clone(),
                        work.cancel.clone(),
                    )?;
                    serde_json::to_value(report)
                        .map_err(|_| error("official-distribution", "setup report encoding failed"))
                }
                Operation::Review {} => {
                    reviewed.take();
                    let packages = decode_packages(work.payload, &config.limits)?;
                    let plan =
                        catalog.review(packages, &verifier, gate.clone(), work.cancel.clone())?;
                    if work.cancel.is_cancelled() {
                        return Err(error("manager-cancelled", "package review cancelled"));
                    }
                    let review_id = nonce()?;
                    let value = json!({"reviewId":review_id,"review":plan.review(),"executionPermissionGranted":false});
                    reviewed = Some((review_id, plan));
                    Ok(value)
                }
                Operation::Commit {
                    review_id,
                    allow_downgrade,
                    allow_same_version_rebuild,
                } => {
                    if reviewed.as_ref().is_none_or(|(key, _)| key != &review_id) {
                        return Err(error("manager-review", "review is missing or expired"));
                    }
                    let (_, plan) = reviewed.take().unwrap();
                    Ok(
                        json!({"catalog":catalog.commit(plan,ImportConfirmation{allow_downgrade,allow_same_version_rebuild})?,"executionPermissionGranted":false}),
                    )
                }
                Operation::Discard { review_id } => {
                    if reviewed.as_ref().is_some_and(|(key, _)| key == &review_id) {
                        reviewed.take();
                    }
                    Ok(json!({"discarded":true}))
                }
                _ => Err(error("manager-request", "unexpected operation")),
            }
        })();
        activity.lock().unwrap_or_else(|p| p.into_inner()).active = None;
        reply(&output, id, result);
    }
    drop(receiver);
    // Reader is already returning on close/EOF; it never has to await a guest.
    let _ = reader.join();
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_control_fields_cannot_supply_paths_or_authority_and_policy_requires_explicit_nulls() {
        for operation in [
            json!({"method":"list","trusted":true}),
            json!({"method":"review","path":"private"}),
            json!({"method":"commit","reviewId":"fake","allowDowngrade":true,"allowSameVersionRebuild":false,"root":"elsewhere"}),
        ] {
            assert!(
                serde_json::from_value::<Request>(json!({"id":"1","operation":operation})).is_err()
            );
        }
        assert!(serde_json::from_value::<Policy>(json!({"terminationGraceMs":1})).is_err());
        assert!(serde_json::from_value::<Policy>(json!({"processMemoryBytes":null,"wasmMemoryBytes":null,"fuelPerCall":null,"walltimeMs":null,"terminationGraceMs":1})).is_ok());
    }
    #[test]
    fn binary_batch_rejects_zero_count_overflow_truncation_and_trailing_bytes() {
        let limits = PackageLimits::default();
        for payload in [
            vec![],
            vec![0; 4],
            u32::MAX.to_le_bytes().to_vec(),
            [1, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255].to_vec(),
            [1, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0].to_vec(),
        ] {
            assert!(decode_packages(payload, &limits).is_err());
        }
    }
}
