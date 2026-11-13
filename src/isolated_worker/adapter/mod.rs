//! Host-owned validated Wasm composition for v3. No default registry/GUI activation.
mod dictionary;
mod gate;
#[cfg(all(feature = "resource-integration-probe", target_arch = "x86_64"))]
pub mod integration_probe;
mod network;
mod profile_pool;
mod retained;
pub(in crate::isolated_worker) use retained::Owner as RetainedOwner;
mod prepared;
mod ranges;
#[cfg(test)]
mod resource_sdk_tests;
#[cfg(test)]
mod retained_driver_tests;
#[cfg(test)]
mod retained_tests;
pub(in crate::isolated_worker) use network::NetworkPlans;
pub use network::{NetworkConfiguration, NetworkLimits, NetworkProvider, NetworkUseDescription};
pub(in crate::isolated_worker) use prepared::ArtifactCapture;
pub(in crate::isolated_worker) use prepared::DictionaryCompanions;
pub use prepared::{PreparationIdentity, PreparationLimits, PreparationSources, PreparedArtifact};
pub use ranges::{
    ResourceRangeDescription, ResourceRangeLimits, ResourceRangeRequest, ResourceRangeSession,
};
mod wire;
pub(in crate::isolated_worker) fn retained_reply(
    bytes: &[u8],
) -> Result<Result<Value, crate::node::CoreError>, crate::node::CoreError> {
    match wire::reply(bytes) {
        Ok(data) => Ok(Ok(data)),
        Err(error) if wire::is_error_reply(bytes) => Ok(Err(error)),
        Err(error) => Err(error),
    }
}
use super::{Cache, EndReason, ExitReceipt, Job, Outcome, Profile, RunControl, pipe::JitDelivery};
use crate::{
    execution_control::{
        CancellationToken, CompletionPermit, DecimalId, DeferredNode, ExecutionHost,
        HostTracePhase, OwnedInvocation, ResourceLease, ResourceUsage, SessionEpoch,
    },
    node::{CoreError, NodeDefinition, Registry},
};
pub(in crate::isolated_worker) use dictionary::{DictionaryBatch, DictionaryPlans};
pub use dictionary::{DictionaryBindingConfig, DictionaryBindings, ResourceDescription};
pub use gate::{ExecutionGate, GrantTarget, GuardedController};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const DLL_HASH: &str = "fa7fdafc1dcb98ff20acd6ea480f305b90b7a8f518cb35110904ef345517e4ef";

#[derive(Clone, Default, PartialEq, Eq)]
pub struct WorkerPolicy {
    pub process_memory_bytes: Option<usize>,
    pub wasm_memory_bytes: Option<u64>,
    pub fuel_per_call: Option<u64>,
    pub walltime: Option<Duration>,
    pub termination_grace: Duration,
}
impl WorkerPolicy {
    fn supports_range_pipeline(&self) -> bool {
        self.fuel_per_call
            .is_some_and(|v| (1..=5_000_000_000).contains(&v))
            && self
                .wasm_memory_bytes
                .is_some_and(|v| (1..=128 * 1024 * 1024).contains(&v))
            && self
                .walltime
                .is_some_and(|v| !v.is_zero() && v <= Duration::from_secs(30))
            && self.termination_grace <= Duration::from_secs(5)
    }

    pub fn bounded_probe() -> Self {
        Self {
            process_memory_bytes: Some(512 * 1024 * 1024),
            wasm_memory_bytes: Some(128 * 1024 * 1024),
            fuel_per_call: Some(50_000_000),
            walltime: Some(Duration::from_secs(30)),
            termination_grace: Duration::from_secs(5),
        }
    }
}
/// Host-only aggregate timing diagnostics. No input, paths, Node identities or run keys.
#[derive(Default)]
struct TimingCounters {
    calls: AtomicU64,
    staging: AtomicU64,
    process: AtomicU64,
    cleanup: AtomicU64,
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTimings {
    pub settled_calls: u64,
    pub staging_micros: u64,
    pub process_micros: u64,
    pub cleanup_micros: u64,
}
fn elapsed_micros(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}
struct Image {
    profile_pool: Option<Arc<profile_pool::ProfilePool>>,
    workspace: PathBuf,
    worker: Arc<[u8]>,
    dll: Arc<[u8]>,
    module: Arc<[u8]>,
    identity: String,
    policy: WorkerPolicy,
    requests: AtomicU64,
    activity: Arc<AtomicU64>,
    timings: Arc<TimingCounters>,
    runtime_files: Arc<std::sync::Mutex<Option<Arc<CodeFiles>>>>,
    module_files: std::sync::Mutex<Option<Arc<CodeFiles>>>,
    code_budget: Arc<CodeBudget>,
    compiled: Option<Arc<std::sync::Mutex<CompiledState>>>,
    jit_compiled: std::sync::Mutex<CompiledState>,
}
struct CompiledAsset {
    files: Arc<CodeFiles>,
    len: usize,
    digest: String,
}
enum CompiledState {
    Pending,
    Ready(CompiledAsset),
    Unavailable,
}
enum WorkerImage {
    Jit,
    Compile,
    CompileJit,
    PreparedCompiled(Arc<CodeFiles>, usize, String),
    Reuse(Arc<CodeFiles>, usize, String),
}
enum WorkerRequest {
    Immediate(Vec<u8>),
    Jit {
        request: Box<dyn FnOnce() -> Result<JitDelivery, String> + Send>,
        trace: Option<Arc<std::sync::OnceLock<OwnedInvocation>>>,
    },
}
/// Paths are chosen by trusted Host installation code, never package/Node fields.
/// All executable bytes are captured before validation, preventing later path replacement.
#[derive(Clone)]
pub struct WasmArtifact(Arc<Image>);
/// Observation only. Does not retain code/resources or establish OS exit.
pub(in crate::isolated_worker) struct ProcessActivity(std::sync::Weak<Image>);
impl ProcessActivity {
    pub(in crate::isolated_worker) fn is_live(&self) -> bool {
        self.0.strong_count() != 0
    }
    pub(in crate::isolated_worker) fn count(&self) -> u64 {
        self.0
            .upgrade()
            .map_or(0, |image| image.activity.load(Ordering::Acquire))
    }
}
fn fail(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
pub(super) fn sha256(bytes: &[u8]) -> Result<String, CoreError> {
    use windows_sys::Win32::Security::Cryptography::*;
    struct Algorithm(BCRYPT_ALG_HANDLE);
    impl Drop for Algorithm {
        fn drop(&mut self) {
            unsafe {
                BCryptCloseAlgorithmProvider(self.0, 0);
            }
        }
    }
    let mut algorithm = std::ptr::null_mut();
    let name = super::wide(std::ffi::OsStr::new("SHA256"));
    if unsafe { BCryptOpenAlgorithmProvider(&mut algorithm, name.as_ptr(), std::ptr::null(), 0) }
        < 0
    {
        return Err(fail("worker-identity", "SHA256 provider unavailable"));
    }
    let algorithm = Algorithm(algorithm);
    let mut output = [0u8; 32];
    let length = u32::try_from(bytes.len())
        .map_err(|_| fail("worker-identity", "asset exceeds digest input limit"))?;
    if unsafe {
        BCryptHash(
            algorithm.0,
            std::ptr::null(),
            0,
            bytes.as_ptr(),
            length,
            output.as_mut_ptr(),
            32,
        )
    } < 0
    {
        return Err(fail("worker-identity", "asset digest failed"));
    }
    Ok(output.iter().map(|b| format!("{b:02x}")).collect())
}
fn asset(path: &Path, limit: usize) -> Result<Arc<[u8]>, CoreError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| fail("worker-asset", "Host asset unavailable"))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("worker-asset", "Host asset read failed"))?;
    if bytes.len() > limit {
        return Err(fail("worker-asset", "Host asset exceeds load limit"));
    }
    Ok(Arc::from(bytes))
}
fn settle(mut outcome: Outcome<'_>) -> Option<ExitReceipt> {
    loop {
        match outcome {
            Outcome::CancelledBeforeStart => return None,
            Outcome::Exited(receipt) => return Some(receipt),
            Outcome::Unconfirmed(owner) => {
                outcome = owner.retry(Duration::from_millis(100));
            }
        }
    }
}
struct ScopedCache(Cache);
impl std::ops::Deref for ScopedCache {
    type Target = Cache;
    fn deref(&self) -> &Cache {
        &self.0
    }
}
impl Drop for ScopedCache {
    fn drop(&mut self) {
        let _ = cleanup(&self.0);
    }
}
/// Host-factory-wide bound for retained immutable code files, including all package images.
struct CodeBudget {
    used: AtomicU64,
    limit: u64,
}
impl Default for CodeBudget {
    fn default() -> Self {
        Self {
            used: AtomicU64::new(0),
            limit: 512 * 1024 * 1024,
        }
    }
}
struct CodeReservation {
    budget: Arc<CodeBudget>,
    bytes: u64,
}
impl CodeBudget {
    fn reserve(self: &Arc<Self>, bytes: u64) -> Result<CodeReservation, CoreError> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(bytes).filter(|new| *new <= self.limit)
            })
            .map_err(|_| fail("worker-asset", "immutable code storage limit exceeded"))?;
        Ok(CodeReservation {
            budget: self.clone(),
            bytes,
        })
    }
}
impl Drop for CodeReservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
/// Captured immutable code only; every invocation still has a fresh process and profile.
struct CodeFiles {
    verified_runtime: std::sync::OnceLock<Result<super::runtime::VerifiedRuntimeFile, String>>,
    reservation: Option<CodeReservation>,
    locks: Vec<std::fs::File>,
    cache: ScopedCache,
    healthy: std::sync::Mutex<bool>,
    inherited_acl: bool,
}
impl CodeFiles {
    fn new(
        workspace: &Path,
        assets: &[(&str, &[u8])],
        budget: &Arc<CodeBudget>,
    ) -> Result<Self, CoreError> {
        let bytes = assets
            .iter()
            .try_fold(0u64, |total, (_, bytes)| {
                total.checked_add(bytes.len() as u64)
            })
            .ok_or_else(|| fail("worker-asset", "code size overflow"))?;
        let reservation = budget.reserve(bytes)?;
        // Own the reservation together with the directory before the first write,
        // so partial construction has the same cleanup/accounting as a live cache.
        let mut files = Self {
            verified_runtime: std::sync::OnceLock::new(),
            reservation: Some(reservation),
            locks: Vec::new(),
            cache: ScopedCache(
                Cache::new_code_snapshot(workspace)
                    .map_err(|_| fail("worker-asset", "runtime cache unavailable"))?,
            ),
            healthy: std::sync::Mutex::new(true),
            inherited_acl: std::env::var_os("DIY_EXPERIMENT_INHERITED_CODE_ACL").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        };
        for (name, bytes) in assets {
            files
                .cache
                .write(name, bytes)
                .map_err(|_| fail("worker-asset", "code snapshot unavailable"))?;
        }
        // Deny write/delete sharing until every actual invocation has ended.
        use std::os::windows::fs::OpenOptionsExt;
        for (name, _) in assets {
            files.locks.push(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
                    .open(files.cache.path().join(name))
                    .map_err(|_| fail("worker-asset", "code snapshot lock unavailable"))?,
            );
        }
        Ok(files)
    }
    fn change_access(&self, profile: &Profile, allow: bool) -> Result<(), String> {
        if self.inherited_acl {
            self.cache
                .change_inherited_code_access(profile, self.locks.len(), allow)
        } else if allow {
            self.cache.allow_read_execute(profile)
        } else {
            self.cache.revoke_read_execute(profile)
        }
    }
    fn check_healthy(&self) -> Result<(), CoreError> {
        let healthy = self
            .healthy
            .lock()
            .map_err(|_| fail("worker-asset", "code permission state unavailable"))?;
        if !*healthy {
            return Err(fail(
                "worker-asset",
                "code snapshot permissions unavailable",
            ));
        }
        Ok(())
    }
    fn lease<'a>(&'a self, profile: &'a Profile) -> Result<CodeLease<'a>, CoreError> {
        let mut healthy = self
            .healthy
            .lock()
            .map_err(|_| fail("worker-asset", "code permission state unavailable"))?;
        if !*healthy {
            return Err(fail(
                "worker-asset",
                "runtime snapshot permissions unavailable",
            ));
        }
        if self.change_access(profile, true).is_err() {
            *healthy = false;
            return Err(fail(
                "worker-isolation",
                "runtime read permission unavailable",
            ));
        }
        Ok(CodeLease {
            files: self,
            profile,
            active: true,
        })
    }
}
impl Drop for CodeFiles {
    fn drop(&mut self) {
        drop(self.verified_runtime.take());
        self.locks.clear();
        if cleanup(&self.cache).is_err() {
            // Keep failed cleanup charged for the rest of this Host lifetime.
            // Files are immutable code only; never recursively remove unknown entries.
            if let Some(reservation) = self.reservation.take() {
                std::mem::forget(reservation);
            }
        }
    }
}
struct CodeLease<'a> {
    files: &'a CodeFiles,
    profile: &'a Profile,
    active: bool,
}
impl CodeLease<'_> {
    fn release(&mut self) -> Result<(), CoreError> {
        self.release_measured(None)
    }

    fn release_measured(&mut self, lock_wait_us: Option<&mut u64>) -> Result<(), CoreError> {
        if !self.active {
            return Ok(());
        }
        let waiting = lock_wait_us.as_ref().map(|_| std::time::Instant::now());
        let mut healthy = self
            .files
            .healthy
            .lock()
            .map_err(|_| fail("worker-cleanup", "code permission state unavailable"))?;
        if let Some((waiting, total)) = waiting.zip(lock_wait_us) {
            *total = total.saturating_add(elapsed_micros(waiting));
        }
        if self.files.change_access(self.profile, false).is_err() {
            *healthy = false;
            return Err(fail(
                "worker-cleanup",
                "runtime read permission cleanup failed",
            ));
        }
        self.active = false;
        Ok(())
    }
}
impl Drop for CodeLease<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
pub(super) fn remove_asset(path: &Path) -> Result<(), CoreError> {
    // A signalled process handle does not prevent another Windows service
    // temporarily opening the exited image without FILE_SHARE_DELETE.
    for attempt in 0..31 {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt < 30 && matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(fail(
                    "worker-cleanup",
                    &format!(
                        "asset cleanup failed (OS {})",
                        error.raw_os_error().unwrap_or(-1)
                    ),
                ));
            }
        }
    }
    unreachable!()
}
fn cleanup(cache: &Cache) -> Result<(), CoreError> {
    use std::os::windows::fs::MetadataExt;
    // This newly created directory is already canonicalized within the chosen Host workspace.
    // Delete fixed regular assets only; never recursive-delete or follow a new link.
    let root = cache.path();
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(fail("worker-cleanup", "cache metadata unavailable")),
    };
    if metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
        || metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !root
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("DIYIME2.Worker."))
    {
        return Err(fail("worker-cleanup", "unexpected cache identity"));
    }
    let entries: Vec<_> = std::fs::read_dir(root)
        .map_err(|_| fail("worker-cleanup", "cache unavailable"))?
        .collect::<Result<_, _>>()
        .map_err(|_| fail("worker-cleanup", "cache enumeration failed"))?;
    for entry in &entries {
        if !["worker.exe", "wasmtime.dll", "node.wasm", "node.cwasm"]
            .iter()
            .any(|n| entry.file_name() == *n)
            || !entry.metadata().is_ok_and(|m| {
                m.is_file()
                    && m.file_attributes()
                        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                        == 0
            })
            || !entry
                .file_type()
                .is_ok_and(|m| m.is_file() && !m.is_symlink())
        {
            return Err(fail("worker-cleanup", "unexpected cache asset type/name"));
        }
    }
    for entry in entries {
        remove_asset(&entry.path())?;
    }
    std::fs::remove_dir(root).map_err(|_| fail("worker-cleanup", "cache cleanup failed"))
}
impl WasmArtifact {
    pub fn capture(
        workspace: &Path,
        worker: &Path,
        runtime_dll: &Path,
        module: Vec<u8>,
        policy: WorkerPolicy,
    ) -> Result<Self, CoreError> {
        Self::capture_assets(
            workspace,
            // Trusted Host executables can include debug symbols during verification.
            || asset(worker, 128 * 1024 * 1024),
            || asset(runtime_dll, 256 * 1024 * 1024),
            module,
            policy,
        )
    }

    fn capture_assets(
        workspace: &Path,
        worker: impl FnOnce() -> Result<Arc<[u8]>, CoreError>,
        dll: impl FnOnce() -> Result<Arc<[u8]>, CoreError>,
        module: Vec<u8>,
        policy: WorkerPolicy,
    ) -> Result<Self, CoreError> {
        if !module.starts_with(b"\0asm\x01\0\0\0") {
            return Err(fail("worker-module", "binary Wasm v1 required"));
        }
        if policy.fuel_per_call == Some(0)
            || policy
                .wasm_memory_bytes
                .is_some_and(|n| n == 0 || n > i64::MAX as u64)
        {
            return Err(fail("worker-policy", "invalid runtime resource policy"));
        }
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|_| fail("worker-asset", "Host cache workspace unavailable"))?;
        let worker = worker()?;
        let dll = dll()?;
        let dll_hash = sha256(&dll)?;
        if dll_hash != DLL_HASH {
            return Err(fail(
                "worker-identity",
                "runtime does not match the pinned official DLL",
            ));
        }
        let identity = format!("{}:{}:{}", sha256(&worker)?, dll_hash, sha256(&module)?);
        Ok(Self(Arc::new(Image {
            profile_pool: None,
            workspace,
            worker,
            dll,
            module: Arc::from(module),
            identity,
            policy,
            requests: AtomicU64::new(0),
            activity: Arc::new(AtomicU64::new(0)),
            timings: Arc::new(TimingCounters::default()),
            code_budget: Arc::new(CodeBudget::default()),
            runtime_files: Arc::new(std::sync::Mutex::new(None)),
            module_files: std::sync::Mutex::new(None),
            compiled: None,
            jit_compiled: std::sync::Mutex::new(CompiledState::Pending),
        })))
    }
    pub(super) fn enable_profile_preparation(mut self, physical_slots: u64) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("new Host runtime")
            .profile_pool = profile_pool::ProfilePool::start(physical_slots).map(Arc::new);
        self
    }
    pub(super) fn enable_official_compilation(mut self) -> Self {
        if !cfg!(feature = "compiled-module-jit-baseline") {
            Arc::get_mut(&mut self.0)
                .expect("new Host runtime")
                .compiled = Some(Arc::new(std::sync::Mutex::new(CompiledState::Pending)));
        }
        self
    }
    /// Reuse the captured Host Worker/DLL bytes for another package module.
    pub fn with_module(&self, module: Vec<u8>, policy: WorkerPolicy) -> Result<Self, CoreError> {
        if !module.starts_with(b"\0asm\x01\0\0\0") {
            return Err(fail("worker-module", "binary Wasm v1 required"));
        }
        if policy.process_memory_bytes == Some(0)
            || policy.fuel_per_call == Some(0)
            || policy
                .wasm_memory_bytes
                .is_some_and(|n| n == 0 || n > i64::MAX as u64)
        {
            return Err(fail("worker-policy", "invalid runtime resource policy"));
        }
        let worker_hash = self
            .0
            .identity
            .split(':')
            .next()
            .expect("captured identity");
        let identity = format!("{worker_hash}:{DLL_HASH}:{}", sha256(&module)?);
        Ok(Self(Arc::new(Image {
            profile_pool: self.0.profile_pool.clone(),
            workspace: self.0.workspace.clone(),
            worker: self.0.worker.clone(),
            dll: self.0.dll.clone(),
            module: Arc::from(module),
            identity,
            policy,
            requests: AtomicU64::new(0),
            activity: Arc::new(AtomicU64::new(0)),
            timings: Arc::new(TimingCounters::default()),
            code_budget: self.0.code_budget.clone(),
            runtime_files: self.0.runtime_files.clone(),
            module_files: std::sync::Mutex::new(None),
            compiled: self.0.compiled.clone(),
            // A new captured module/policy never inherits another source's code.
            jit_compiled: std::sync::Mutex::new(CompiledState::Pending),
        })))
    }
    pub fn identity(&self) -> &str {
        &self.0.identity
    }
    pub fn active_processes(&self) -> u64 {
        self.0.activity.load(Ordering::Acquire)
    }
    /// Diagnostics only: these counters never release a slot or grant authority.
    pub fn timings(&self) -> WorkerTimings {
        WorkerTimings {
            settled_calls: self.0.timings.calls.load(Ordering::Acquire),
            staging_micros: self.0.timings.staging.load(Ordering::Acquire),
            process_micros: self.0.timings.process.load(Ordering::Acquire),
            cleanup_micros: self.0.timings.cleanup.load(Ordering::Acquire),
        }
    }
    pub fn request_count(&self) -> u64 {
        self.0.requests.load(Ordering::Acquire)
    }
    fn call(&self, request: &Value, cancel: CancellationToken) -> Result<Vec<u8>, CoreError> {
        self.call_traced(request, cancel, None, 0, None)
    }

    fn runtime_code_files(&self) -> Result<Arc<CodeFiles>, CoreError> {
        let mut captured = self
            .0
            .runtime_files
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if captured.is_none() {
            *captured = Some(Arc::new(CodeFiles::new(
                &self.0.workspace,
                &[
                    ("worker.exe", &self.0.worker),
                    ("wasmtime.dll", &self.0.dll),
                ],
                &self.0.code_budget,
            )?));
        }
        Ok(captured.as_ref().expect("captured runtime").clone())
    }

    fn module_code_files(&self) -> Result<Arc<CodeFiles>, CoreError> {
        let mut captured = self
            .0
            .module_files
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if captured.is_none() {
            *captured = Some(Arc::new(CodeFiles::new(
                &self.0.workspace,
                &[("node.wasm", &self.0.module)],
                &self.0.code_budget,
            )?));
        }
        Ok(captured.as_ref().expect("captured module").clone())
    }
    fn call_traced(
        &self,
        request: &Value,
        cancel: CancellationToken,
        invocation: Option<&OwnedInvocation>,
        instance: u32,
        delivery: Option<super::pipe::JitDeliveryCheck>,
    ) -> Result<Vec<u8>, CoreError> {
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "Worker was not started"));
        }
        let bytes = serde_json::to_vec(request)
            .map_err(|_| fail("worker-request", "request serialization failed"))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(fail("worker-request", "request exceeds guest wire limit"));
        }
        self.0.requests.fetch_add(1, Ordering::AcqRel);
        let image = self.worker_image(&cancel);
        let request = if matches!(image, WorkerImage::Jit) {
            WorkerRequest::Jit {
                request: Box::new(move || {
                    Ok(JitDelivery {
                        bytes,
                        check: delivery,
                        run_cancel: None,
                        detailed: false,
                    })
                }),
                trace: None,
            }
        } else {
            WorkerRequest::Immediate(bytes)
        };
        self.call_worker(request, cancel, invocation, instance, image)
    }

    fn worker_image(&self, cancel: &CancellationToken) -> WorkerImage {
        let Some(compiled) = &self.0.compiled else {
            return WorkerImage::Jit;
        };
        if self.0.identity.rsplit(':').next() != Some(super::runtime::OFFICIAL_WASM_HASH)
            || self.0.policy.fuel_per_call != Some(50_000_000)
            || self.0.policy.wasm_memory_bytes != Some(128 * 1024 * 1024)
            || cancel.is_cancelled()
        {
            return WorkerImage::Jit;
        }
        // A different call preparing the asset does not hold up this call.
        let Ok(mut captured) = compiled.try_lock() else {
            return WorkerImage::Jit;
        };
        if matches!(*captured, CompiledState::Pending) {
            let prepared = self.compile_official_asset(cancel);
            if cancel.is_cancelled() {
                // Withdrawal must not disable compilation for a later caller.
                return WorkerImage::Jit;
            }
            *captured = prepared.map_or(CompiledState::Unavailable, CompiledState::Ready);
        }
        if let CompiledState::Ready(artifact) = &*captured {
            WorkerImage::Reuse(
                artifact.files.clone(),
                artifact.len,
                artifact.digest.clone(),
            )
        } else {
            WorkerImage::Jit
        }
    }

    fn prepared_jit_image(&self, cancel: &CancellationToken) -> WorkerImage {
        if std::env::var_os("DIY_EXPERIMENT_COMPILED_JIT").as_deref()
            != Some(std::ffi::OsStr::new("1"))
            || cancel.is_cancelled()
        {
            return WorkerImage::Jit;
        }
        // One compile-only process per captured source/policy. Other owners
        // retain ordinary JIT while compilation is underway; no busy wait.
        let Ok(mut state) = self.0.jit_compiled.try_lock() else {
            return WorkerImage::Jit;
        };
        if matches!(*state, CompiledState::Pending) {
            let produced: Result<CompiledAsset, CoreError> = (|| {
                let bytes = self.call_worker(
                    WorkerRequest::Immediate(Vec::new()),
                    cancel.clone(),
                    None,
                    0,
                    WorkerImage::CompileJit,
                )?;
                // Only accept the captured compiler's output after original
                // process/IO/ACL/profile cleanup, never a package artifact.
                super::runtime::compiled_jit::validate_produced(
                    &bytes,
                    self.0.identity.rsplit(':').next().unwrap(),
                    self.0.policy.fuel_per_call,
                    self.0.policy.wasm_memory_bytes,
                )
                .map_err(|_| fail("worker-identity", "compiled JIT binding unavailable"))?;
                let digest = sha256(&bytes)?;
                let files = Arc::new(CodeFiles::new(
                    &self.0.workspace,
                    &[("node.cwasm", &bytes)],
                    &self.0.code_budget,
                )?);
                Ok(CompiledAsset {
                    files,
                    len: bytes.len(),
                    digest,
                })
            })();
            if cancel.is_cancelled() {
                return WorkerImage::Jit;
            }
            *state = produced.map_or(CompiledState::Unavailable, CompiledState::Ready);
        }
        if let CompiledState::Ready(asset) = &*state {
            WorkerImage::PreparedCompiled(asset.files.clone(), asset.len, asset.digest.clone())
        } else {
            WorkerImage::Jit
        }
    }

    fn compile_official_asset(
        &self,
        cancel: &CancellationToken,
    ) -> Result<CompiledAsset, CoreError> {
        // This process is compile-only and receives no guest input. A failed
        // preparation may fall back to JIT; guest execution is never retried.
        let bytes = self.call_worker(
            WorkerRequest::Immediate(Vec::new()),
            cancel.clone(),
            None,
            0,
            WorkerImage::Compile,
        )?;
        let digest = sha256(&bytes)?;
        let files = Arc::new(CodeFiles::new(
            &self.0.workspace,
            &[("node.cwasm", &bytes)],
            &self.0.code_budget,
        )?);
        Ok(CompiledAsset {
            files,
            len: bytes.len(),
            digest,
        })
    }

    fn call_worker(
        &self,
        request: WorkerRequest,
        cancel: CancellationToken,
        invocation: Option<&OwnedInvocation>,
        instance: u32,
        image: WorkerImage,
    ) -> Result<Vec<u8>, CoreError> {
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "Worker was not started"));
        }
        let staging = invocation.map(|i| i.trace_host_phase(HostTracePhase::WorkerStaging));
        let started = std::time::Instant::now();
        let mut profile = self
            .0
            .profile_pool
            .as_ref()
            .and_then(|pool| pool.take())
            .map(Ok)
            .unwrap_or_else(Profile::new)
            .map_err(|_| {
                fail(
                    "worker-isolation",
                    "AppContainer unavailable; refusing execution",
                )
            })?;
        let runtime_files = self.runtime_code_files()?;
        let mut runtime_lease = runtime_files.lease(&profile)?;
        let exe = runtime_files.cache.path().join("worker.exe");
        let dll = runtime_files.cache.path().join("wasmtime.dll");
        let verified_runtime =
            if matches!(image, WorkerImage::Jit | WorkerImage::PreparedCompiled(..))
                && std::env::var_os("DIY_EXPERIMENT_VERIFIED_RUNTIME_FILE").as_deref()
                    == Some(std::ffi::OsStr::new("1"))
            {
                Some(
                    runtime_files
                        .verified_runtime
                        .get_or_init(|| super::runtime::VerifiedRuntimeFile::open(&dll))
                        .as_ref()
                        .map_err(|_| fail("worker-identity", "runtime file proof unavailable"))?,
                )
            } else {
                None
            };
        if matches!(image, WorkerImage::Jit | WorkerImage::PreparedCompiled(..))
            != matches!(request, WorkerRequest::Jit { .. })
        {
            return Err(fail(
                "worker-request",
                "Worker image and request transport differ",
            ));
        }
        let stream_enabled = std::env::var_os("DIY_EXPERIMENT_JIT_STREAM_MODULE").as_deref()
            == Some(std::ffi::OsStr::new("1"));
        let stream_module = matches!(image, WorkerImage::Jit)
            && (1..=super::runtime::MAX_STREAMED_MODULE_BYTES).contains(&self.0.module.len())
            && stream_enabled;
        let mut streamed_bytes = stream_module.then(|| self.0.module.clone());
        let mut stream_charge = None;
        let mut streamed_files = None;
        let mut late_module_digest = None;
        let (module_files, module_name, module_len, flag): (_, _, _, Option<String>) = match image {
            WorkerImage::Jit if stream_module => (
                None,
                "-",
                self.0.module.len(),
                Some("--prepare-jit-pipe-v1".into()),
            ),
            WorkerImage::Jit => (
                Some(self.module_code_files()?),
                "node.wasm",
                self.0.module.len(),
                Some("--prepare-jit-v1".into()),
            ),
            WorkerImage::Compile => (
                Some(self.module_code_files()?),
                "node.wasm",
                self.0.module.len(),
                Some("--compile-official".into()),
            ),
            WorkerImage::CompileJit => (
                Some(self.module_code_files()?),
                "node.wasm",
                self.0.module.len(),
                Some("--compile-jit-v1".into()),
            ),
            WorkerImage::PreparedCompiled(files, len, digest) => {
                let runtime_spare = std::env::var_os("DIY_EXPERIMENT_RUNTIME_ONLY_SPARE")
                    .as_deref()
                    == Some(std::ffi::OsStr::new("1"));
                // Charge both Vec and Arc conversion storage conservatively.
                // If the existing code budget cannot cover it, retain the
                // file-based path rather than widening a resource limit.
                let charge = (stream_enabled || runtime_spare)
                    .then(|| self.0.code_budget.reserve(2 * (len as u64 + 1)))
                    .and_then(Result::ok);
                if let Some(charge) = charge {
                    files.check_healthy()?;
                    let mut bytes = Vec::with_capacity(len + 1);
                    std::fs::File::open(files.cache.path().join("node.cwasm"))
                        .and_then(|file| file.take(len as u64 + 1).read_to_end(&mut bytes))
                        .map_err(|_| fail("worker-asset", "compiled stream unavailable"))?;
                    if bytes.len() != len {
                        return Err(fail("worker-identity", "compiled stream length changed"));
                    }
                    stream_charge = Some(charge);
                    streamed_files = Some(files);
                    streamed_bytes = Some(Arc::from(bytes));
                    if runtime_spare {
                        late_module_digest = Some(digest.clone());
                    }
                    (
                        None,
                        "-",
                        if runtime_spare { 0 } else { len },
                        Some(if runtime_spare {
                            "--prepare-runtime-jit-v1".into()
                        } else {
                            format!("--prepare-compiled-jit-pipe-v1={digest}")
                        }),
                    )
                } else {
                    (
                        Some(files),
                        "node.cwasm",
                        len,
                        Some(format!("--prepare-compiled-jit-v1={digest}")),
                    )
                }
            }
            WorkerImage::Reuse(files, len, digest) => (
                Some(files),
                "node.cwasm",
                len,
                Some(format!("--compiled-official={digest}")),
            ),
        };
        let mut module_lease = module_files
            .as_ref()
            .map(|files| files.lease(&profile))
            .transpose()?;
        let module = module_files.as_ref().map_or_else(
            || OsString::from(module_name),
            |files| files.cache.path().join(module_name).into_os_string(),
        );
        let job = Job::with_memory_limit(self.0.policy.process_memory_bytes)
            .map_err(|_| fail("worker-isolation", "Worker Job unavailable"))?;
        let option =
            |n: Option<u64>| OsString::from(n.map_or_else(|| "none".to_owned(), |n| n.to_string()));
        let mut args: Vec<_> = flag
            .into_iter()
            .map(OsString::from)
            .chain([
                dll.into_os_string(),
                module,
                OsString::from(module_len.to_string()),
                option(self.0.policy.fuel_per_call),
                option(self.0.policy.wasm_memory_bytes),
            ])
            .collect();
        let control = RunControl {
            cancel,
            walltime: self.0.policy.walltime,
            termination_grace: self.0.policy.termination_grace,
        };
        let staged = elapsed_micros(started);
        drop(staging);
        let process = invocation.map(|i| i.trace_host_phase(HostTracePhase::WorkerProcess));
        let process_started = std::time::Instant::now();
        // settle retains any unconfirmed owner on this same execution thread.
        let deferred_trace = match &request {
            WorkerRequest::Jit { trace, .. } => trace.clone(),
            WorkerRequest::Immediate(_) => None,
        };
        let legacy_prepared_diagnostics = deferred_trace.is_some()
            && std::env::var_os("DIY_EXPERIMENT_LEGACY_PREPARED_DIAGNOSTICS").as_deref()
                == Some(std::ffi::OsStr::new("1"));
        let detailed = legacy_prepared_diagnostics
            || invocation.is_some_and(OwnedInvocation::host_trace_is_detailed);
        let deferred_diagnostics = deferred_trace.is_some() && !legacy_prepared_diagnostics;
        if deferred_diagnostics {
            args.push("--deferred-diagnostics-v1".into());
        }
        let scope = super::pipe::WorkerScope::observed(
            &profile,
            &job,
            &runtime_files.cache,
            self.0.activity.clone(),
        )
        .with_verified_runtime(verified_runtime);
        let (scope, bytes) = match request {
            WorkerRequest::Jit { request, .. } => (
                scope.with_jit_delivery(
                    streamed_bytes,
                    request,
                    control.cancel.clone(),
                    deferred_diagnostics,
                    late_module_digest,
                ),
                Vec::new(),
            ),
            WorkerRequest::Immediate(bytes) => (scope, bytes),
        };
        let receipt = settle(
            super::pipe::execute_with_diagnostics(scope, &exe, &args, bytes, &control, detailed)
                .map_err(|_| fail("worker-start", "isolated Worker could not start"))?,
        );
        let process_time = elapsed_micros(process_started);
        // Original process and all transport owners have settled. No temporary
        // streamed-code allocation can remain in a running IO closure now.
        drop(stream_charge);
        drop(process);
        let trace_invocation =
            invocation.or_else(|| deferred_trace.as_deref().and_then(std::sync::OnceLock::get));
        let _cleanup = trace_invocation.map(|i| i.trace_host_phase(HostTracePhase::WorkerCleanup));
        if let (Some(invocation), Some(receipt)) = (
            trace_invocation.filter(|i| i.host_trace_is_detailed()),
            &receipt,
        ) {
            match receipt
                .diagnostics()
                .and_then(|r| r.as_ref().ok())
                .and_then(|b| super::worker_trace::decode(b).ok())
            {
                Some(intervals) => {
                    if std::env::var_os("DIY_EXPERIMENT_DIRECT_WORKER_TRACE").as_deref()
                        == Some(std::ffi::OsStr::new("1"))
                    {
                        // Matched diagnostic control, with the same binaries,
                        // Native evidence, limits and execution route.
                        for interval in intervals {
                            invocation.trace_worker_interval(
                                instance,
                                interval.phase,
                                Duration::from_micros(interval.start),
                                interval.end.map(Duration::from_micros),
                            );
                        }
                    } else {
                        invocation.trace_worker_intervals(
                            instance,
                            intervals.into_iter().map(|interval| {
                                (
                                    interval.phase,
                                    Duration::from_micros(interval.start),
                                    interval.end.map(Duration::from_micros),
                                )
                            }),
                        );
                    }
                }
                None => invocation.trace_worker_diagnostics_lost(),
            }
        }
        let measure_cleanup = deferred_trace.is_some()
            && trace_invocation.is_some_and(OwnedInvocation::host_trace_is_detailed);
        let mut lock_wait_us = 0;
        let cleanup_started = std::time::Instant::now();
        drop(job);
        let job_close_us = measure_cleanup.then(|| elapsed_micros(cleanup_started));
        let mut release_acls = || -> Result<(), CoreError> {
            if let Some(mut lease) = module_lease.take() {
                lease.release_measured(measure_cleanup.then_some(&mut lock_wait_us))?;
            }
            runtime_lease.release_measured(measure_cleanup.then_some(&mut lock_wait_us))?;
            if let Some(files) = &streamed_files {
                files.check_healthy()?;
            }
            Ok(())
        };
        let overlap_cleanup = deferred_trace.is_some()
            && std::env::var_os("DIY_EXPERIMENT_OVERLAP_WORKER_CLEANUP").as_deref()
                == Some(std::ffi::OsStr::new("1"));
        let (acl_us, profile_delete_us) = if overlap_cleanup {
            let name = &profile.name;
            // The original process and IO owners have settled and the Job is closed.
            // The code ACL uses the retained SID and only touches the code cache.
            let (acl_result, acl_us, deletion) = std::thread::scope(|scope| {
                let remove_profile = || {
                    let started = std::time::Instant::now();
                    (Profile::delete_named(name), elapsed_micros(started))
                };
                let deletion = std::thread::Builder::new().spawn_scoped(scope, remove_profile);
                let acl_result = release_acls();
                let acl_us = elapsed_micros(cleanup_started);
                let deletion = deletion.map_or_else(|_| Ok(remove_profile()), |task| task.join());
                (acl_result, acl_us, deletion)
            });
            drop(module_lease);
            drop(runtime_lease);
            let (result, duration) = deletion
                .map_err(|_| fail("worker-cleanup", "Worker profile cleanup thread failed"))?;
            profile.deleted = result.is_ok();
            acl_result?;
            result.map_err(|_| fail("worker-cleanup", "Worker profile cleanup failed"))?;
            (acl_us, duration)
        } else {
            release_acls()?;
            drop(module_lease);
            drop(runtime_lease);
            let acl_us = elapsed_micros(cleanup_started);
            profile
                .delete()
                .map_err(|_| fail("worker-cleanup", "Worker profile cleanup failed"))?;
            (
                acl_us,
                elapsed_micros(cleanup_started).saturating_sub(acl_us),
            )
        };
        let cleanup_us = elapsed_micros(cleanup_started);
        if let Some(job_close_us) = job_close_us
            && cleanup_us >= 20_000
        {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "prepared JIT cleanup: job_close_us={job_close_us} acl_lock_wait_us={lock_wait_us} job_and_acl_us={acl_us} profile_delete_us={profile_delete_us} overlap={overlap_cleanup}"
            );
        }
        self.0.timings.staging.fetch_add(staged, Ordering::AcqRel);
        self.0
            .timings
            .process
            .fetch_add(process_time, Ordering::AcqRel);
        self.0
            .timings
            .cleanup
            .fetch_add(cleanup_us, Ordering::AcqRel);
        self.0.timings.calls.fetch_add(1, Ordering::AcqRel);
        let receipt = receipt.ok_or_else(|| fail("worker-cancelled", "Worker was not started"))?;
        if receipt.reason() != EndReason::Exited
            || receipt.exit_code() != Some(0)
            || !receipt.input_delivered()
        {
            #[cfg(test)]
            eprintln!(
                "failed Worker: exit={:?} reason={:?} input_delivered={} trace={:?}",
                receipt.exit_code(),
                receipt.reason(),
                receipt.input_delivered(),
                receipt
                    .diagnostics()
                    .map(|r| r
                        .as_ref()
                        .map(|b| super::worker_trace::decode(b).map(|intervals| intervals
                            .into_iter()
                            .map(|i| (i.phase, i.start, i.end))
                            .collect::<Vec<_>>())))
            );
            return Err(fail(
                "worker-failed",
                "isolated Worker did not complete successfully",
            ));
        }
        receipt
            .output()
            .map(<[u8]>::to_vec)
            .map_err(|_| fail("worker-reply", "Worker output unavailable or oversized"))
    }
    /// Blocking Host preparation: run on an owned preparation thread, before Core prepare.
    pub fn prepare(
        &self,
        kind: &str,
        version: u32,
        configs: Vec<Value>,
        gate: ExecutionGate,
        cancel: CancellationToken,
    ) -> Result<WasmNode, CoreError> {
        if matches!(
            kind,
            "input.text" | "demo.packet" | "e2e.input" | "e2e.output"
        ) {
            return Err(fail(
                "worker-kind",
                "system Nodes cannot be registered from guest",
            ));
        }
        let described = wire::reply(&self.call(
            &wire::request(kind, version, json!({"method":"describe"})),
            cancel.clone(),
        )?)?;
        let definition: NodeDefinition = serde_json::from_value(described)
            .map_err(|_| fail("worker-definition", "invalid guest definition"))?;
        if definition.kind != kind || definition.version != version {
            return Err(fail("worker-definition", "guest identity mismatch"));
        }
        let mut approved = vec![definition.default_config.clone()];
        for config in configs {
            if !approved.contains(&config) {
                approved.push(config);
            }
        }
        for config in &approved {
            let validated = wire::reply(&self.call(
                &wire::request(kind, version, json!({"method":"validate","config":config})),
                cancel.clone(),
            )?)?;
            if validated != json!({"valid":true}) {
                return Err(fail(
                    "worker-definition",
                    "guest did not validate the exact config",
                ));
            }
        }
        let node = WasmNode {
            artifact: self.clone(),
            definition: Arc::new(definition),
            configs: Arc::new(approved),
            gate,
            package_identity: None,
            dictionary: None,
            prepared_dictionary: Arc::default(),
            network: None,
            retained: retained::NodeLink::default(),
            prepared_jit: Arc::default(),
        };
        // Reuse Core's exact port/type/default validation, without invoking guest code here.
        let mut registry = Registry::new();
        registry.register_deferred(node.clone())?;
        Ok(node)
    }
}
#[derive(Clone)]
pub struct WasmNode {
    artifact: WasmArtifact,
    definition: Arc<NodeDefinition>,
    configs: Arc<Vec<Value>>,
    gate: ExecutionGate,
    package_identity: Option<String>,
    dictionary: Option<DictionaryPlans>,
    prepared_dictionary: Arc<std::sync::OnceLock<prepared::PreparedDictionary>>,
    network: Option<NetworkPlans>,
    retained: retained::NodeLink,
    prepared_jit: Arc<std::sync::Mutex<Vec<std::sync::Weak<PreparedJitSlot>>>>,
}
impl WasmNode {
    pub(super) fn bind_network(mut self, plans: Option<NetworkPlans>) -> Self {
        self.network = plans;
        self
    }
    pub(super) fn network_descriptions(&self) -> Vec<NetworkUseDescription> {
        self.network
            .as_ref()
            .map_or_else(Vec::new, NetworkPlans::descriptions)
    }
    pub(super) fn fresh_preparation(mut self) -> Self {
        self.dictionary = self.dictionary.as_ref().map(DictionaryPlans::fresh);
        self.prepared_dictionary = Arc::default();
        self.retained = retained::NodeLink::default();
        self.prepared_jit = Arc::default();
        self
    }
    pub(super) fn bind_dictionary(mut self, plans: Option<DictionaryPlans>) -> Self {
        self.dictionary = plans;
        self
    }
    pub(super) fn resource_descriptions(
        &self,
        config: &Value,
    ) -> Result<Vec<ResourceDescription>, CoreError> {
        self.dictionary
            .as_ref()
            .map_or_else(|| Ok(vec![]), |p| p.descriptions(config))
    }
    pub(super) fn dictionary_bindings(&self) -> Option<Arc<DictionaryBindings>> {
        self.dictionary.as_ref().map(|p| p.bindings.clone())
    }
    pub(super) fn capture_resources(
        &self,
        config: &Value,
        batch: &mut DictionaryBatch<'_>,
        cancel: &CancellationToken,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if let Some(plans) = &self.dictionary {
            plans.capture(config, batch, cancel, check)?;
        }
        Ok(())
    }
    pub(super) fn initialize_resources(
        &self,
        config: &Value,
        batch: &mut DictionaryBatch<'_>,
        cancel: CancellationToken,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
        prepared: Option<(
            &DictionaryCompanions,
            &crate::execution_control::RunKey,
            &str,
        )>,
    ) -> Result<(), CoreError> {
        let prepared = prepared.filter(|_| self.artifact.0.policy.supports_range_pipeline());
        if let Some(plans) = &self.dictionary {
            plans.initialize(config, batch, cancel.clone(), check, |snapshots| {
                if let Some((modules, key, id)) = prepared {
                    return self.initialize_prepared_dictionary(
                        modules, key, id, config, snapshots, cancel,
                    );
                }
                let payload: std::collections::BTreeMap<_, _> = snapshots
                    .iter()
                    .map(|(id, bytes)| (id, bytes.as_ref()))
                    .collect();
                let valid = wire::reply(&self.artifact.call(
                    &wire::request(
                        &self.definition.kind,
                        self.definition.version,
                        json!({"method":"prepare","config":config,"resources":payload}),
                    ),
                    cancel,
                )?)?;
                if valid.as_object().is_none_or(|v| v.len() != 2)
                    || valid["valid"] != true
                    || !valid["prepared"].is_boolean()
                {
                    return Err(fail("worker-reply", "invalid resource preparation reply"));
                }
                Ok(())
            })?;
        } else if prepared.is_some() {
            return Err(fail(
                "dictionary-not-prepared",
                "approved dictionary binding missing",
            ));
        }
        Ok(())
    }
    pub fn active_processes(&self) -> u64 {
        self.artifact.active_processes()
    }
    pub(in crate::isolated_worker) fn process_activity(&self) -> ProcessActivity {
        ProcessActivity(Arc::downgrade(&self.artifact.0))
    }
    pub(super) fn permission_identity(&self) -> Result<String, CoreError> {
        let package = self
            .package_identity
            .as_ref()
            .ok_or_else(|| fail("permission-identity", "validated package identity required"))?;
        let mut identity = json!({"permissionFormat":1,"runtime":self.artifact.identity(),"package":package,"definition":*self.definition});
        if let Some(plans) = &self.dictionary {
            identity["resourcePolicy"] = json!(plans.identity);
        }
        if let Some(plans) = &self.network {
            identity["networkPolicy"] = json!(plans.identity);
        }
        sha256(
            &serde_json::to_vec(&identity)
                .map_err(|_| fail("permission-identity", "identity encoding failed"))?,
        )
    }
    pub(super) fn merge_validated_configs(&mut self, other: &Self) -> Result<(), CoreError> {
        if self.permission_identity()? != other.permission_identity()?
            || !self.gate.same_authority(&other.gate)
        {
            return Err(fail(
                "permission-identity",
                "one kind cannot mix implementation/authority proofs",
            ));
        }
        match (&self.network, &other.network) {
            (Some(a), Some(b)) if a.same_proof(b) => (),
            (None, None) => (),
            _ => return Err(fail("permission-identity", "network proofs differ")),
        }
        match (&mut self.dictionary, &other.dictionary) {
            (Some(a), Some(b)) => a.merge(b)?,
            (None, None) => (),
            _ => return Err(fail("permission-identity", "resource proofs differ")),
        }
        let mut configs = (*self.configs).clone();
        for config in other.configs.iter() {
            if !configs.contains(config) {
                configs.push(config.clone());
            }
        }
        self.configs = Arc::new(configs);
        Ok(())
    }
    pub(super) fn permission_gate(&self) -> ExecutionGate {
        self.gate.clone()
    }
    pub(super) fn bind_package(mut self, identity: String) -> Self {
        self.package_identity = Some(identity);
        self
    }
    pub fn grant_target(
        &self,
        session: &SessionEpoch,
        plan: DecimalId,
        node_id: impl Into<String>,
    ) -> GrantTarget {
        GrantTarget::new(
            format!(
                "{}:{}@{}:{}",
                self.artifact.identity(),
                self.definition.kind,
                self.definition.version,
                self.package_identity.as_deref().unwrap_or("raw")
            ),
            session,
            plan,
            node_id.into(),
        )
    }
}
impl DeferredNode for WasmNode {
    fn definition(&self) -> NodeDefinition {
        (*self.definition).clone()
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if self.configs.contains(config) {
            Ok(())
        } else {
            Err(fail(
                "wasm-config-not-prepared",
                "exact configuration has not been validated in an isolated Worker",
            ))
        }
    }
    fn start(
        &self,
        invocation: OwnedInvocation,
        completion: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        match self.retained.start(self, (invocation, completion, cancel)) {
            Ok(()) => Ok(()),
            Err(arguments) => {
                let (invocation, completion, cancel) = *arguments;
                self.start_legacy(invocation, completion, cancel)
            }
        }
    }
}
impl WasmNode {
    pub(in crate::isolated_worker) fn shares_jit_preparation(&self, other: &Self) -> bool {
        // This identity is fresh for each prepared plan. Equal module bytes or
        // a package's claimed kind are insufficient to share a waiting owner.
        Arc::ptr_eq(&self.prepared_jit, &other.prepared_jit)
    }

    pub(in crate::isolated_worker) fn can_prepare_jit(&self) -> bool {
        self.dictionary.is_none() && self.network.is_none() && self.artifact.0.compiled.is_none()
    }

    pub(in crate::isolated_worker) fn prepare_jit_slot(
        &self,
        host: &ExecutionHost,
        ids: Vec<String>,
        limit: usize,
        refill_controller: Option<GuardedController>,
        scope_cancel: CancellationToken,
    ) -> Option<Arc<PreparedJitSlot>> {
        if !self.can_prepare_jit() || host.policy().max_physical_jobs < 2 {
            return None;
        }
        let slot = Arc::new(PreparedJitSlot {
            ids,
            owners: std::sync::Mutex::new(Vec::new()),
            capacity: limit.min(host.policy().max_physical_jobs.saturating_sub(1) as usize),
            host: host.clone(),
            refill_controller,
            scope_cancel,
        });
        slot.refill(&self.artifact);
        self.prepared_jit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::downgrade(&slot));
        Some(slot)
    }

    fn take_prepared_jit(&self, id: &str) -> Option<PreparedJitOwner> {
        let slot = self.prepared_jit_slot(id)?;
        let mut owners = slot.owners.lock().unwrap_or_else(|p| p.into_inner());
        let ready = owners
            .iter_mut()
            .position(|worker| worker.observe_ready(Duration::ZERO));
        if let Some(index) = ready {
            return Some(owners.remove(index));
        }
        if std::env::var_os("DIY_EXPERIMENT_PREPARED_JIT_WAIT").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return None;
        }
        // A short grace period can avoid a full cold launch when a replacement
        // is already compiling. An unfinished owner remains available on timeout.
        let index = owners.iter().position(PreparedJitOwner::is_running)?;
        if !owners[index].observe_ready(Duration::from_millis(3)) {
            return None;
        }
        Some(owners.remove(index))
    }

    fn prepared_jit_slot(&self, id: &str) -> Option<Arc<PreparedJitSlot>> {
        self.prepared_jit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .find(|slot| slot.ids.iter().any(|candidate| candidate == id))
    }

    fn start_legacy(
        &self,
        invocation: OwnedInvocation,
        completion: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let node = self.clone();
        std::thread::Builder::new()
            .name("diyime2-wasm-invocation".into())
            .spawn(move || {
                let mut deferred_refill = None;
                let run = || -> Result<(), CoreError> {
                    let request_phase = invocation.trace_host_phase(HostTracePhase::Request);
                    node.validate_config(invocation.config())?;
                    let key = invocation.key();
                    let target = node.grant_target(
                        &key.session_epoch,
                        key.plan_version,
                        invocation.context().node_id,
                    );
                    let lease = node.gate.begin(target, key, cancel.clone())?;
                    if let Some(prepared) = node.prepared_dictionary.get() {
                        let mut scope = prepared.scope(
                            &node, invocation.context().node_id, key, cancel.clone(),
                            node.artifact.0.policy.walltime.unwrap_or(Duration::from_secs(30)),
                        )?;
                        let request = prepared.request(&invocation, &node.definition, &mut scope)?;
                        let bytes = serde_json::to_vec(&request)
                            .map_err(|_| fail("worker-request", "request serialization failed"))?;
                        drop(request_phase);
                        let reply = prepared.execute(bytes, scope, cancel.clone())?;
                        let wire::ExecutionStep::Outputs(result) = wire::execution_step(&reply, &node.definition)? else {
                            return Err(fail("package-capability", "prepared dictionary cannot request network"));
                        };
                        node.gate.publish(&lease, key, || {
                            if cancel.is_cancelled() {
                                return Err(fail("worker-cancelled", "invocation cancelled"));
                            }
                            completion.complete(Ok(result));
                            Ok(())
                        })??;
                        return Ok(());
                    }
                    let mut request = wire::execution(&invocation, &node.definition)?;
                    if let Some(plans) = &node.dictionary {
                        request["operation"]["resources"] = plans.payload(invocation.config())?;
                    }
                    let mut network = node.network.as_ref().map(NetworkPlans::execution).transpose()?;
                    let mut check = || {
                        if cancel.is_cancelled() { return Err(fail("worker-cancelled", "invocation cancelled")); }
                        node.gate.publish(&lease, key, || ())
                    };
                    drop(request_phase);
                    let mut worker_instance = 0u32;
                    let result = loop {
                        check()?;
                        if let Some(scope) = &network { scope.check(&cancel)?; }
                        // call returns only after this Worker, pipes and handles end.
                        worker_instance = worker_instance.saturating_add(1);
                        let gate = node.gate.clone();
                        let delivery_lease = lease.clone();
                        let delivery_key = key.clone();
                        let delivery = Box::new(move || {
                            gate.publish(&delivery_lease, &delivery_key, || ())
                                .map_err(|error| error.to_string())
                        });
                        let response = if worker_instance == 1
                            && let Some(prepared) = node.take_prepared_jit(invocation.context().node_id)
                        {
                            let bytes = serde_json::to_vec(&request)
                                .map_err(|_| fail("worker-request", "request serialization failed"))?;
                            match prepared.try_dispatch(
                                JitDelivery { bytes, check: Some(delivery), run_cancel: Some(cancel.clone()), detailed: false },
                                Some((&invocation, &completion)),
                            ) {
                                PreparedDispatch::Settled(result) => {
                                    node.artifact.0.requests.fetch_add(1, Ordering::AcqRel);
                                    result
                                }
                                PreparedDispatch::Undelivered(delivery) => {
                                    node.artifact.call_traced(&request, cancel.clone(), Some(&invocation), worker_instance, delivery.check)
                                }
                            }
                        } else {
                            node.artifact.call_traced(&request, cancel.clone(), Some(&invocation), worker_instance, Some(delivery))
                        };
                        // An input replacement cancels this invocation without withdrawing
                        // the plan grant. Refill only after its Worker has settled.
                        if std::env::var_os("DIY_EXPERIMENT_NO_JIT_REFILL").as_deref()
                            != Some(std::ffi::OsStr::new("1"))
                            && (response.is_ok() || cancel.is_cancelled())
                            && let Some(slot) = node.prepared_jit_slot(invocation.context().node_id)
                            && node.gate.publish(&lease, key, || ()).is_ok()
                        {
                            if let Some(controller) = &slot.refill_controller {
                                deferred_refill = Some((Arc::downgrade(&slot), controller.clone(), lease.clone()));
                            } else {
                                slot.refill(&node.artifact);
                            }
                        }
                        let bytes = response?;
                        check()?;
                        if let Some(scope) = &network { scope.check(&cancel)?; }
                        match wire::execution_step(&bytes, &node.definition)? {
                            wire::ExecutionStep::Outputs(result) => break result,
                            wire::ExecutionStep::Network(call) => {
                                let scope = network.as_mut().ok_or_else(|| fail("package-capability", "Node has no approved network proof"))?;
                                let exchange = invocation.trace_host_phase(HostTracePhase::BrokerExchange);
                                let (continuation, response) = scope.exchange(call, &cancel, &mut check)?;
                                drop(exchange);
                                check()?;
                                request = wire::request(&node.definition.kind, node.definition.version, json!({"method":"resume", "config":invocation.config(), "continuation":continuation, "response":response}));
                                if let Some(plans) = &node.dictionary {
                                    request["operation"]["resources"] = plans.payload(invocation.config())?;
                                }
                            }
                        }
                    };
                    // A second policy check is linearized with completion enqueue below.
                    node.gate.publish(&lease, key, || {
                        if cancel.is_cancelled() {
                            return Err(fail("worker-cancelled", "invocation cancelled"));
                        }
                        if let Some(scope) = &network {
                            scope.check(&cancel)?;
                        }
                        completion.complete(Ok(result));
                        Ok(())
                    })??;
                    Ok(())
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                    .unwrap_or_else(|_| {
                        Err(fail(
                            "worker-host-fault",
                            "Host invocation failed after physical ownership cleanup",
                        ))
                    });
                match result {
                    Ok(_) => (),
                    Err(error) => {
                        // Both failure and cancellation paths reach here only after no process
                        // was created or the original process/IO threads actually ended.
                        if !cancel.is_cancelled() || !completion.finish_cancelled() {
                            completion.complete(Err(error));
                        }
                    }
                }
                if let Some((slot, controller, lease)) = deferred_refill {
                    let key = invocation.key();
                    if PreparedJitSlot::wait_for_graph_cleanup(&controller, key)
                        && node.gate.publish(&lease, key, || ()).is_ok()
                        && let Some(slot) = slot.upgrade()
                    {
                        slot.refill(&node.artifact);
                    }
                }
            })
            .map_err(|_| fail("worker-thread", "Host execution thread unavailable"))?;
        Ok(())
    }
}
pub(in crate::isolated_worker) struct PreparedJitSlot {
    ids: Vec<String>,
    owners: std::sync::Mutex<Vec<PreparedJitOwner>>,
    capacity: usize,
    host: ExecutionHost,
    refill_controller: Option<GuardedController>,
    scope_cancel: CancellationToken,
}

impl PreparedJitSlot {
    pub(in crate::isolated_worker) fn wait_ready(
        &self,
        cancel: &CancellationToken,
        deadline: std::time::Instant,
    ) -> Result<bool, CoreError> {
        loop {
            if cancel.is_cancelled() {
                return Err(fail("worker-cancelled", "JIT preparation cancelled"));
            }
            let ready = {
                let mut owners = self.owners.lock().unwrap_or_else(|p| p.into_inner());
                // Missing reservations cannot become ready by waiting. Leave
                // the ordinary cold path available under resource contention.
                if owners.len() != self.capacity {
                    return Ok(false);
                }
                owners
                    .iter_mut()
                    .all(|owner| owner.observe_ready(Duration::ZERO))
            };
            if ready {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_graph_cleanup(
        controller: &GuardedController,
        key: &crate::execution_control::RunKey,
    ) -> bool {
        use crate::execution_control::{PhysicalCounts, RunPhase};
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(5)
            && let Ok(snapshot) = controller.wait_run(key)
        {
            if snapshot.state == RunPhase::Terminal
                && snapshot.physical == PhysicalCounts::default()
            {
                return true;
            }
            // A superseded run can return immediately while physical owners drain.
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    fn refill(&self, artifact: &WasmArtifact) {
        if self.scope_cancel.is_cancelled() {
            return;
        }
        let mut owners = self.owners.lock().unwrap_or_else(|p| p.into_inner());
        owners.retain(PreparedJitOwner::is_running);
        while owners.len() < self.capacity {
            let Ok(owner) =
                PreparedJitOwner::start_scoped(&self.host, artifact.clone(), &self.scope_cancel)
            else {
                break;
            };
            owners.push(owner);
        }
    }
}

struct PreparedJitOwner {
    input: Option<std::sync::mpsc::SyncSender<JitDelivery>>,
    ready: std::sync::mpsc::Receiver<()>,
    ready_seen: bool,
    cancel: CancellationToken,
    worker: Option<std::thread::JoinHandle<Result<Vec<u8>, CoreError>>>,
    trace: Arc<std::sync::OnceLock<OwnedInvocation>>,
    reservation: std::sync::Weak<std::sync::Mutex<ResourceLease>>,
    host: ExecutionHost,
}

enum PreparedDispatch {
    Settled(Result<Vec<u8>, CoreError>),
    Undelivered(JitDelivery),
}

impl PreparedJitOwner {
    fn observe_ready(&mut self, timeout: Duration) -> bool {
        if !self.is_running() {
            return false;
        }
        if !self.ready_seen {
            self.ready_seen = if timeout.is_zero() {
                self.ready.try_recv().is_ok()
            } else {
                self.ready.recv_timeout(timeout).is_ok()
            };
        }
        self.ready_seen && self.is_running()
    }

    fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
    #[cfg(test)]
    fn start(host: &ExecutionHost, artifact: WasmArtifact) -> Result<Self, CoreError> {
        Self::start_scoped(host, artifact, &CancellationToken::default())
    }

    fn start_scoped(
        host: &ExecutionHost,
        artifact: WasmArtifact,
        scope: &CancellationToken,
    ) -> Result<Self, CoreError> {
        use std::sync::mpsc;
        if scope.is_cancelled() {
            return Err(fail("worker-cancelled", "JIT plan scope withdrawn"));
        }
        let slot = host.try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..ResourceUsage::default()
        })?;
        let reservation = Arc::new(std::sync::Mutex::new(slot));
        let worker_reservation = reservation.clone();
        let (notify, ready) = mpsc::sync_channel(1);
        let (input, receive) = mpsc::sync_channel(1);
        let cancel = scope.scoped_child();
        let stop = cancel.clone();
        let trace = Arc::new(std::sync::OnceLock::new());
        let worker_trace = trace.clone();
        let worker = std::thread::Builder::new()
            .name("diyime-prepared-jit".into())
            .spawn(move || {
                let _slot = worker_reservation;
                let delivery_cancel = stop.clone();
                // The charged physical slot covers compiler then execution
                // sequentially; the compiler settles before this image exists.
                let image = artifact.prepared_jit_image(&stop);
                artifact.call_worker(
                    WorkerRequest::Jit {
                        request: Box::new(move || {
                            notify
                                .send(())
                                .map_err(|_| "prepared Worker owner closed")?;
                            let deadline = std::time::Instant::now() + Duration::from_secs(5);
                            loop {
                                if delivery_cancel.is_cancelled() {
                                    return Err("prepared JIT delivery cancelled".into());
                                }
                                let remaining =
                                    deadline.saturating_duration_since(std::time::Instant::now());
                                if remaining.is_zero() {
                                    return Err("prepared JIT delivery expired".into());
                                }
                                match receive.recv_timeout(remaining.min(Duration::from_millis(10)))
                                {
                                    Ok(delivery) => return Ok(delivery),
                                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                                        return Err("prepared Worker owner closed".into());
                                    }
                                }
                            }
                        }),
                        trace: Some(worker_trace),
                    },
                    stop,
                    None,
                    1,
                    image,
                )
            })
            .map_err(|_| fail("worker-thread", "prepared JIT owner unavailable"))?;
        Ok(Self {
            input: Some(input),
            ready,
            ready_seen: false,
            cancel,
            worker: Some(worker),
            trace,
            reservation: Arc::downgrade(&reservation),
            host: host.clone(),
        })
    }

    fn try_dispatch(
        mut self,
        mut delivery: JitDelivery,
        execution: Option<(&OwnedInvocation, &CompletionPermit)>,
    ) -> PreparedDispatch {
        if let Some((invocation, completion)) = execution {
            if completion.key() != invocation.key() {
                return PreparedDispatch::Settled(Err(fail(
                    "invalid-reservation",
                    "prepared Worker and dispatch differ",
                )));
            }
            let slot = match completion.pin_physical_reservation(&self.host) {
                Ok(slot) => slot,
                Err(error) => return PreparedDispatch::Settled(Err(error)),
            };
            // Both charges exist until admission. Thereafter the original
            // process and the dispatcher own the same reservation.
            let Some(reservation) = self.reservation.upgrade() else {
                return PreparedDispatch::Undelivered(delivery);
            };
            *reservation.lock().unwrap_or_else(|p| p.into_inner()) = slot;
            if invocation.host_trace_is_detailed() {
                let _ = self.trace.set(invocation.clone());
            }
            delivery.detailed = invocation.host_trace_is_detailed();
        }
        if let Err(error) = self.input.take().unwrap().send(delivery) {
            return PreparedDispatch::Undelivered(error.0);
        }
        PreparedDispatch::Settled(
            self.worker
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Err(fail("worker-thread", "prepared JIT owner failed"))),
        )
    }

    #[cfg(test)]
    fn dispatch(
        self,
        delivery: JitDelivery,
        execution: Option<(&OwnedInvocation, &CompletionPermit)>,
    ) -> Result<Vec<u8>, CoreError> {
        match self.try_dispatch(delivery, execution) {
            PreparedDispatch::Settled(result) => result,
            PreparedDispatch::Undelivered(_) => Err(fail(
                "worker-request",
                "prepared Worker stopped before delivery",
            )),
        }
    }
}

impl Drop for PreparedJitOwner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.input.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn generated_compiled_code_releases_its_snapshot_and_budget() {
        let budget = Arc::new(CodeBudget::default());
        let files = CodeFiles::new(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &[("node.cwasm", b"generated immutable code")],
            &budget,
        )
        .unwrap();
        let path = files.cache.path().join("node.cwasm");
        assert!(path.exists());
        assert!(std::fs::File::options().write(true).open(&path).is_err());
        drop(files);
        assert!(!path.exists());
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }

    #[test]
    #[ignore = "requires current production Worker, pinned DLL and real AppContainer"]
    fn real_compiled_jit_cache_preserves_source_policy_single_use_and_exit() {
        use crate::execution_control::ResourcePolicy;
        assert_eq!(
            std::env::var("DIY_EXPERIMENT_COMPILED_JIT").as_deref(),
            Ok("1")
        );
        let contract: Value = serde_json::from_str(include_str!(
            "../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let mut resources =
            ResourcePolicy::from_value(contract["resourceProfiles"]["bounded"].clone()).unwrap();
        resources.max_physical_jobs = 1;
        let host = ExecutionHost::new("compiled-jit-lifecycle", resources).unwrap();
        let worker = std::env::var_os("DIY_RANGE_WORKER_BINARY").unwrap();
        let dll = std::env::var_os("DIY_RANGE_RUNTIME_DLL").unwrap();
        let fixture =
            |mode| super::super::runtime::compiled_jit_probe::worker_fixture(Path::new(&dll), mode);
        let source = fixture("state");
        let policy = WorkerPolicy {
            fuel_per_call: Some(10_000),
            wasm_memory_bytes: Some(1024 * 1024),
            ..WorkerPolicy::bounded_probe()
        };
        let artifact = WasmArtifact::capture(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            Path::new(&worker),
            Path::new(&dll),
            source.clone(),
            policy.clone(),
        )
        .unwrap();
        let readiness_attempt = std::cell::Cell::new(0);
        let ready = |artifact: &WasmArtifact| {
            let attempt = readiness_attempt.get() + 1;
            readiness_attempt.set(attempt);
            let mut owner = PreparedJitOwner::start(&host, artifact.clone()).unwrap();
            if let Err(error) = owner.ready.recv_timeout(Duration::from_secs(5)) {
                let processes = artifact.active_processes();
                let settled = artifact.timings().settled_calls;
                owner.cancel.cancel();
                let result = owner.worker.take().unwrap().join();
                panic!(
                    "compiled JIT readiness failed: attempt={attempt} active={processes} settled={settled} {error:?}; {result:?}"
                );
            }
            assert_eq!(artifact.active_processes(), 1);
            assert_eq!(host.usage().physical_jobs, 1);
            assert!(matches!(
                *artifact.0.jit_compiled.lock().unwrap(),
                CompiledState::Ready(_)
            ));
            owner
        };
        let delivery = |check| JitDelivery {
            bytes: b"{}".to_vec(),
            check,
            run_cancel: None,
            detailed: false,
        };
        for expected_settled in [2, 3] {
            let before = artifact.0.code_budget.used.load(Ordering::Acquire);
            let owner = ready(&artifact);
            // Exactly one compiler exited before the first spare became ready;
            // the next spare loads that code and owns a new Store/process.
            assert_eq!(artifact.timings().settled_calls, expected_settled - 1);
            if expected_settled == 3 {
                let state = artifact.0.jit_compiled.lock().unwrap();
                let CompiledState::Ready(asset) = &*state else {
                    unreachable!()
                };
                let temporary = if [
                    "DIY_EXPERIMENT_JIT_STREAM_MODULE",
                    "DIY_EXPERIMENT_RUNTIME_ONLY_SPARE",
                ]
                .iter()
                .any(|name| std::env::var_os(name).as_deref() == Some(std::ffi::OsStr::new("1")))
                {
                    2 * (asset.len as u64 + 1)
                } else {
                    0
                };
                assert_eq!(
                    artifact.0.code_budget.used.load(Ordering::Acquire),
                    before + temporary
                );
            }
            let mut input = delivery(None);
            input.detailed = expected_settled == 3;
            let bytes = owner.dispatch(input, None).unwrap();
            assert_eq!(wire::reply(&bytes).unwrap()["value"], 1);
            assert_eq!(artifact.timings().settled_calls, expected_settled);
            assert_eq!(artifact.active_processes(), 0);
            assert_eq!(host.usage().physical_jobs, 0);
            if expected_settled == 3 {
                assert_eq!(artifact.0.code_budget.used.load(Ordering::Acquire), before);
            }
        }
        let owner = ready(&artifact);
        assert!(
            owner
                .dispatch(
                    delivery(Some(Box::new(|| Err("withdrawn before delivery".into())))),
                    None
                )
                .is_err()
        );
        assert_eq!(artifact.timings().settled_calls, 4);
        assert_eq!(host.usage().physical_jobs, 0);
        let owner = ready(&artifact);
        assert_eq!(
            wire::reply(&owner.dispatch(delivery(None), None).unwrap()).unwrap()["value"],
            1
        );
        assert_eq!(artifact.timings().settled_calls, 5);

        let baseline = artifact.0.code_budget.used.load(Ordering::Acquire);
        let full = artifact
            .0
            .code_budget
            .reserve(artifact.0.code_budget.limit - baseline)
            .unwrap();
        let owner = ready(&artifact); // no buffer headroom: preserve file fallback
        assert_eq!(
            wire::reply(&owner.dispatch(delivery(None), None).unwrap()).unwrap()["value"],
            1
        );
        assert_eq!(
            artifact.0.code_budget.used.load(Ordering::Acquire),
            artifact.0.code_budget.limit
        );
        drop(full);
        assert_eq!(
            artifact.0.code_budget.used.load(Ordering::Acquire),
            baseline
        );

        let preparing = artifact
            .with_module(
                fixture("state"),
                WorkerPolicy {
                    fuel_per_call: Some(11_000),
                    ..policy.clone()
                },
            )
            .unwrap();
        let scope = CancellationToken::default();
        let held = PreparedJitOwner::start_scoped(&host, preparing.clone(), &scope).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while preparing.active_processes() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "no preparation process observed"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // Cancel while preparation/ready ownership is retained and before any
        // input delivery; no status query or slot Drop may be needed to stop it.
        scope.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while preparing.active_processes() != 0 || host.usage().physical_jobs != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "cancelled compiler/spare remained live"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(held);
        let before_retry = preparing.timings().settled_calls;
        let compiled_before_retry = matches!(
            *preparing.0.jit_compiled.lock().unwrap(),
            CompiledState::Ready(_)
        );
        let owner = ready(&preparing);
        assert_eq!(
            preparing.timings().settled_calls,
            before_retry + u64::from(!compiled_before_retry)
        );
        assert_eq!(
            wire::reply(&owner.dispatch(delivery(None), None).unwrap()).unwrap()["value"],
            1
        );
        assert_eq!(preparing.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);

        let different = artifact
            .with_module(fixture("different"), policy.clone())
            .unwrap();
        let changed_policy = artifact
            .with_module(
                source,
                WorkerPolicy {
                    fuel_per_call: None,
                    ..policy.clone()
                },
            )
            .unwrap();
        let standard = artifact
            .with_module(fixture("standard-state"), policy.clone())
            .unwrap();
        let memory = artifact
            .with_module(fixture("memory"), policy.clone())
            .unwrap();
        for (other, expected) in [
            (&different, 2),
            (&changed_policy, 1),
            (&standard, 1),
            (&memory, 1),
        ] {
            assert!(matches!(
                *other.0.jit_compiled.lock().unwrap(),
                CompiledState::Pending
            ));
            let owner = ready(other);
            assert_eq!(other.timings().settled_calls, 1);
            assert_eq!(
                wire::reply(&owner.dispatch(delivery(None), None).unwrap()).unwrap()["value"],
                expected
            );
            assert_eq!(other.timings().settled_calls, 2);
            assert_eq!(other.active_processes(), 0);
            assert_eq!(host.usage().physical_jobs, 0);
        }
        let trap = artifact
            .with_module(fixture("trap"), policy.clone())
            .unwrap();
        let owner = ready(&trap); // start must not execute during compilation/load
        assert!(owner.dispatch(delivery(None), None).is_err());
        assert_eq!(
            trap.timings().settled_calls,
            2,
            "no retry after guest start traps"
        );
        assert_eq!(trap.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);

        let fuel = artifact
            .with_module(fixture("fuel"), policy.clone())
            .unwrap();
        let owner = ready(&fuel);
        assert!(owner.dispatch(delivery(None), None).is_err());
        assert_eq!(
            fuel.timings().settled_calls,
            2,
            "fuel failure is not retried"
        );
        assert_eq!(fuel.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);

        let tail = artifact
            .with_module(fixture("tail"), policy.clone())
            .unwrap();
        let owner = ready(&tail);
        let code_path = {
            let state = tail.0.jit_compiled.lock().unwrap();
            let CompiledState::Ready(asset) = &*state else {
                panic!("compiler did not produce code")
            };
            let path = asset.files.cache.path().join("node.cwasm");
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(
                bytes[8], 1,
                "Winch-incompatible source must use standard compiler"
            );
            assert_eq!(sha256(&bytes).unwrap(), asset.digest);
            assert!(std::fs::File::options().write(true).open(&path).is_err());
            path
        };
        drop(owner); // no input, still requires actual process/IO cleanup
        assert_eq!(tail.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);
        assert_eq!(tail.timings().settled_calls, 2);
        drop(tail);
        assert!(
            !code_path.exists(),
            "compiled snapshot retained after all owners closed"
        );
        let poisoned = artifact.with_module(fixture("state"), policy).unwrap();
        drop(ready(&poisoned));
        {
            let state = poisoned.0.jit_compiled.lock().unwrap();
            let CompiledState::Ready(asset) = &*state else {
                unreachable!()
            };
            *asset.files.healthy.lock().unwrap() = false;
        }
        let before = poisoned.0.code_budget.used.load(Ordering::Acquire);
        let mut refused = PreparedJitOwner::start(&host, poisoned.clone()).unwrap();
        assert!(refused.ready.recv_timeout(Duration::from_secs(5)).is_err());
        let error = refused.worker.take().unwrap().join().unwrap().unwrap_err();
        assert_eq!(error.code, "worker-asset");
        assert_eq!(poisoned.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);
        assert_eq!(poisoned.0.code_budget.used.load(Ordering::Acquire), before);
    }

    #[test]
    #[ignore = "requires the production Worker, pinned DLL and a real AppContainer"]
    fn real_prepared_jit_uses_production_worker_lifecycle() {
        use crate::execution_control::{ResourcePolicy, RunKey};
        let contract: Value = serde_json::from_str(include_str!(
            "../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let mut policy =
            ResourcePolicy::from_value(contract["resourceProfiles"]["bounded"].clone()).unwrap();
        policy.max_physical_jobs = 1;
        let host = ExecutionHost::new("prepared-jit-lifecycle", policy).unwrap();
        let worker = std::env::var_os("DIY_RANGE_WORKER_BINARY").unwrap();
        let dll = std::env::var_os("DIY_RANGE_RUNTIME_DLL").unwrap();
        let fixture = std::env::var_os("DIY_RETAINED_HOST_FIXTURE_WASM").unwrap();
        let artifact = WasmArtifact::capture(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            Path::new(&worker),
            Path::new(&dll),
            std::fs::read(fixture).unwrap(),
            WorkerPolicy {
                fuel_per_call: None,
                ..WorkerPolicy::bounded_probe()
            },
        )
        .unwrap();
        let mut owner = PreparedJitOwner::start(&host, artifact.clone()).unwrap();
        if let Err(error) = owner.ready.recv_timeout(Duration::from_secs(5)) {
            owner.cancel.cancel();
            let result = owner.worker.take().unwrap().join();
            panic!("prepared Worker readiness failed: {error:?}; worker: {result:?}");
        }
        assert_eq!(artifact.active_processes(), 1);
        assert_eq!(host.usage().physical_jobs, 1);
        let second = match PreparedJitOwner::start(&host, artifact.clone()) {
            Ok(_) => panic!("a second Worker bypassed the only physical slot"),
            Err(error) => error,
        };
        assert_eq!(second.code, "busy");
        let output = owner
            .dispatch(
                JitDelivery {
                    bytes: serde_json::to_vec(&wire::request(
                        "fixture.wasm",
                        1,
                        json!({"method":"describe"}),
                    ))
                    .unwrap(),
                    check: Some(Box::new(|| Ok(()))),
                    run_cancel: None,
                    detailed: false,
                },
                None,
            )
            .unwrap();
        assert_eq!(wire::reply(&output).unwrap()["kind"], "fixture.wasm");
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(artifact.timings().settled_calls, 1);
        assert_eq!(host.usage().physical_jobs, 0);

        let gate = ExecutionGate::default();
        let key: RunKey = serde_json::from_value(json!({
            "sessionEpoch":"11111111111111111111111111111111",
            "planVersion":"1","runId":"1","deliveryGeneration":"1"
        }))
        .unwrap();
        let target = GrantTarget::new(
            artifact.identity().into(),
            &key.session_epoch,
            key.plan_version,
            "node".into(),
        );
        gate.approve(target.clone());
        let lease = gate
            .begin(target.clone(), &key, CancellationToken::default())
            .unwrap();
        let owner = PreparedJitOwner::start(&host, artifact.clone()).unwrap();
        owner.ready.recv_timeout(Duration::from_secs(5)).unwrap();
        gate.revoke(&target);
        gate.approve(target);
        let stale = owner.dispatch(
            JitDelivery {
                bytes: serde_json::to_vec(&wire::request(
                    "fixture.wasm",
                    1,
                    json!({"method":"describe"}),
                ))
                .unwrap(),
                check: Some(Box::new(move || {
                    gate.publish(&lease, &key, || ())
                        .map_err(|error| error.to_string())
                })),
                run_cancel: None,
                detailed: false,
            },
            None,
        );
        assert_eq!(stale.unwrap_err().code, "worker-failed");
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);

        let owner = PreparedJitOwner::start(&host, artifact.clone()).unwrap();
        owner.ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(artifact.active_processes(), 1);
        assert_eq!(host.usage().physical_jobs, 1);
        drop(owner);
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(artifact.timings().settled_calls, 3);
        assert_eq!(host.usage().physical_jobs, 0);

        let mut owner = PreparedJitOwner::start(&host, artifact.clone()).unwrap();
        owner.ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(host.usage().physical_jobs, 1);
        // Retain the sender to prove cancellation alone wakes the blocked reader.
        owner.cancel.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !owner.worker.as_ref().unwrap().is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "cancelled Worker did not settle"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(owner.worker.take().unwrap().join().unwrap().is_err());
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);
        let unsent = owner.try_dispatch(
            JitDelivery {
                bytes: b"not dispatched".to_vec(),
                check: None,
                run_cancel: None,
                detailed: false,
            },
            None,
        );
        let PreparedDispatch::Undelivered(unsent) = unsent else {
            panic!("an ended Worker cannot accept input");
        };
        assert_eq!(unsent.bytes, b"not dispatched");

        let mut owner = PreparedJitOwner::start(&host, artifact.clone()).unwrap();
        owner.ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let run_cancel = CancellationToken::default();
        let (notify, checked) = mpsc::sync_channel(1);
        owner
            .input
            .take()
            .unwrap()
            .send(JitDelivery {
                bytes: serde_json::to_vec(&wire::request(
                    "fixture.wasm.hang",
                    1,
                    json!({"method":"describe"}),
                ))
                .unwrap(),
                check: Some(Box::new(move || {
                    notify.send(()).map_err(|error| error.to_string())
                })),
                run_cancel: Some(run_cancel.clone()),
                detailed: false,
            })
            .map_err(|_| "prepared Worker stopped before delivery")
            .unwrap();
        checked.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!owner.worker.as_ref().unwrap().is_finished());
        assert_eq!(host.usage().physical_jobs, 1);
        run_cancel.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !owner.worker.as_ref().unwrap().is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "run cancellation did not stop the original Worker"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(owner.worker.take().unwrap().join().unwrap().is_err());
        assert!(!owner.cancel.is_cancelled());
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);

        // Input-free owners can serve either instance of the exact prepared
        // implementation. A new plan and an unrelated graph ID cannot take one.
        let node = artifact
            .prepare(
                "fixture.wasm",
                1,
                vec![json!({"mode":"echo"})],
                ExecutionGate::default(),
                CancellationToken::default(),
            )
            .unwrap();
        assert!(node.shares_jit_preparation(&node.clone()));
        let other_plan = node.clone().fresh_preparation();
        assert!(!node.shares_jit_preparation(&other_plan));
        let mut pool_policy =
            ResourcePolicy::from_value(contract["resourceProfiles"]["bounded"].clone()).unwrap();
        pool_policy.max_physical_jobs = 2;
        let pool_host = ExecutionHost::new("prepared-jit-shared-instances", pool_policy).unwrap();
        let slot = node
            .prepare_jit_slot(
                &pool_host,
                vec!["first".into(), "second".into()],
                1,
                None,
                CancellationToken::default(),
            )
            .unwrap();
        let cancelled_wait = CancellationToken::default();
        cancelled_wait.cancel();
        assert_eq!(
            slot.wait_ready(&cancelled_wait, std::time::Instant::now())
                .unwrap_err()
                .code,
            "worker-cancelled"
        );
        assert!(
            slot.wait_ready(
                &CancellationToken::default(),
                std::time::Instant::now() + Duration::from_secs(5)
            )
            .unwrap()
        );
        assert!(
            slot.wait_ready(&CancellationToken::default(), std::time::Instant::now())
                .unwrap()
        );
        assert!(other_plan.prepared_jit_slot("second").is_none());
        assert!(node.take_prepared_jit("unrelated").is_none());
        for id in ["second", "first"] {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut owner = loop {
                if let Some(owner) = node.take_prepared_jit(id) {
                    break owner;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            };
            // Activation may have observed the one-shot acknowledgement.
            // Repeated observation must not consume the available Worker.
            assert!(owner.observe_ready(Duration::ZERO));
            assert!(owner.observe_ready(Duration::ZERO));
            assert_eq!(pool_host.usage().physical_jobs, 1);
            assert!(node.take_prepared_jit("first").is_none());
            assert!(node.take_prepared_jit("second").is_none());
            let output = owner
                .dispatch(
                    JitDelivery {
                        bytes: serde_json::to_vec(&wire::request(
                            "fixture.wasm",
                            1,
                            json!({"method":"describe"}),
                        ))
                        .unwrap(),
                        check: Some(Box::new(|| Ok(()))),
                        run_cancel: None,
                        detailed: false,
                    },
                    None,
                )
                .unwrap();
            assert_eq!(wire::reply(&output).unwrap()["kind"], "fixture.wasm");
            assert_eq!(artifact.active_processes(), 0);
            assert_eq!(pool_host.usage().physical_jobs, 0);
            if id == "second" {
                slot.refill(&artifact);
            }
        }
        drop(slot);
        assert!(node.prepared_jit_slot("first").is_none());

        let waiting = node
            .prepare_jit_slot(
                &pool_host,
                vec!["waiting".into()],
                1,
                None,
                CancellationToken::default(),
            )
            .unwrap();
        // Delay the parent's notification after the actual isolated process
        // has reached its input-free ready point. No guest input is supplied.
        let (_delayed_notice, delayed) = mpsc::sync_channel(1);
        {
            let mut owners = waiting.owners.lock().unwrap();
            owners[0]
                .ready
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            owners[0].ready = delayed;
        }
        assert!(
            !waiting
                .wait_ready(&CancellationToken::default(), std::time::Instant::now())
                .unwrap()
        );
        let cancel_wait = CancellationToken::default();
        let stop_wait = cancel_wait.clone();
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            stop_wait.cancel();
        });
        assert_eq!(
            waiting
                .wait_ready(
                    &cancel_wait,
                    std::time::Instant::now() + Duration::from_secs(5)
                )
                .unwrap_err()
                .code,
            "worker-cancelled"
        );
        cancel_thread.join().unwrap();
        // A readiness timeout/cancel is not an original-process exit receipt.
        assert_eq!(artifact.active_processes(), 1);
        assert_eq!(pool_host.usage().physical_jobs, 1);
        drop(waiting);
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(pool_host.usage().physical_jobs, 0);

        let scope = CancellationToken::default();
        let scoped = node
            .prepare_jit_slot(&pool_host, vec!["scoped".into()], 1, None, scope.clone())
            .unwrap();
        assert!(
            scoped
                .wait_ready(
                    &CancellationToken::default(),
                    std::time::Instant::now() + Duration::from_secs(5)
                )
                .unwrap()
        );
        scope.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while artifact.active_processes() != 0 || pool_host.usage().physical_jobs != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "withdrawn plan retained an input-free process or reservation"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // Stop does not depend on taking the slot, polling Host status or
        // dropping the plan's inventory. Withdrawal also prevents refilling.
        scoped.refill(&artifact);
        assert_eq!(artifact.active_processes(), 0);
        assert_eq!(pool_host.usage().physical_jobs, 0);
        drop(scoped);
    }

    #[test]
    fn cng_hashes_the_actual_captured_bytes() {
        assert_eq!(
            sha256(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn exited_asset_cleanup_waits_for_temporary_sharing_locks_and_reports_persistent_locks() {
        use std::os::windows::fs::OpenOptionsExt;
        let root =
            std::env::temp_dir().join(format!("diyime-cleanup-{}-{}", std::process::id(), {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                NEXT.fetch_add(1, Ordering::Relaxed)
            }));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("worker.exe");
        std::fs::write(&path, b"synthetic executable bytes").unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(std::fs::remove_file(&path).is_err());
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            drop(locked)
        });
        remove_asset(&path).unwrap();
        release.join().unwrap();
        assert!(!path.exists());
        std::fs::write(&path, b"synthetic executable bytes").unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let failure = remove_asset(&path).unwrap_err();
        assert_eq!(failure.code, "worker-cleanup");
        assert!(failure.message.contains("OS "));
        assert!(path.is_file());
        drop(locked);
        remove_asset(&path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn code_cache_child() {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            Security::{GetTokenInformation, TOKEN_QUERY, TokenIsAppContainer},
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };
        let mut token = std::ptr::null_mut();
        let mut isolated = 0u32;
        let mut size = 0u32;
        unsafe {
            assert_ne!(
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token),
                0
            );
            let ok = GetTokenInformation(
                token,
                TokenIsAppContainer,
                (&mut isolated as *mut u32).cast(),
                4,
                &mut size,
            );
            CloseHandle(token);
            assert_ne!(ok, 0);
        }
        if isolated == 0 {
            return;
        }
        let checks: Vec<(PathBuf, bool)> = serde_json::from_reader(std::io::stdin()).unwrap();
        for (path, expected) in checks {
            assert_eq!(std::fs::read(&path).is_ok(), expected);
            if expected {
                assert!(
                    std::fs::OpenOptions::new().write(true).open(&path).is_err(),
                    "code must remain read-only in the guest"
                );
                assert!(
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path.with_file_name("guest-created.txt"))
                        .is_err(),
                    "guest cannot add files to code storage"
                );
            }
        }
    }
    #[test]
    #[ignore = "local Windows cleanup timing probe"]
    fn measure_code_acl_and_profile_cleanup() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let budget = Arc::new(CodeBudget::default());
        let runtime = CodeFiles::new(
            root,
            &[("worker.exe", b"worker"), ("wasmtime.dll", b"runtime")],
            &budget,
        )
        .unwrap();
        let module = CodeFiles::new(root, &[("node.wasm", b"module")], &budget).unwrap();
        for parallel in [false, true] {
            let mut samples = Vec::new();
            for _ in 0..101 {
                let mut profile = Profile::new().unwrap();
                let mut module_lease = module.lease(&profile).unwrap();
                let mut runtime_lease = runtime.lease(&profile).unwrap();
                let name = profile.name.clone();
                let start = std::time::Instant::now();
                let remove_profile = || {
                    let started = std::time::Instant::now();
                    // This probe creates the profile but never starts a process.
                    Profile::delete_named(&name).unwrap();
                    started.elapsed().as_micros()
                };
                let mut remove_acls = || {
                    module_lease.release().unwrap();
                    runtime_lease.release().unwrap();
                    start.elapsed().as_micros()
                };
                let (acl_us, profile_us) = if parallel {
                    std::thread::scope(|scope| {
                        let deletion = scope.spawn(remove_profile);
                        (remove_acls(), deletion.join().unwrap())
                    })
                } else {
                    (remove_acls(), remove_profile())
                };
                samples.push([acl_us, profile_us, start.elapsed().as_micros()]);
                drop(module_lease);
                drop(runtime_lease);
                profile.deleted = true;
            }
            for (index, phase) in ["code-acls", "profile-delete", "combined"]
                .into_iter()
                .enumerate()
            {
                let mut times: Vec<_> = samples[1..].iter().map(|sample| sample[index]).collect();
                times.sort_unstable();
                println!(
                    "parallel={parallel} {phase}: p50={}us p95={}us p99={}us n={}",
                    times[49],
                    times[94],
                    times[98],
                    times.len()
                );
            }
        }
    }

    #[test]
    fn code_grants_are_specific_to_each_fresh_profile_and_revocation_preserves_the_other() {
        use std::ffi::OsString;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let executable = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let runtime = CodeFiles::new(
            root,
            &[("worker.exe", &executable)],
            &Arc::new(CodeBudget::default()),
        )
        .unwrap();
        let first_module = CodeFiles::new(
            root,
            &[("node.wasm", b"public synthetic code A")],
            &Arc::new(CodeBudget::default()),
        )
        .unwrap();
        let second_module = CodeFiles::new(
            root,
            &[("node.wasm", b"public synthetic code B")],
            &Arc::new(CodeBudget::default()),
        )
        .unwrap();
        let mut first = Profile::new().unwrap();
        let mut second = Profile::new().unwrap();
        assert_ne!(first.name(), second.name());
        let mut first_runtime = runtime.lease(&first).unwrap();
        let mut second_runtime = runtime.lease(&second).unwrap();
        let mut first_code = first_module.lease(&first).unwrap();
        let mut second_code = second_module.lease(&second).unwrap();
        let invoke = |profile: &Profile, a: bool, b: bool| {
            let request = serde_json::to_vec(&[
                (first_module.cache.path().join("node.wasm"), a),
                (second_module.cache.path().join("node.wasm"), b),
            ])
            .unwrap();
            let job = Job::new().unwrap();
            let control = RunControl {
                cancel: CancellationToken::default(),
                walltime: Some(Duration::from_secs(30)),
                termination_grace: Duration::from_millis(5),
            };
            let args = [
                "--exact",
                "isolated_worker::adapter::tests::code_cache_child",
                "--nocapture",
            ]
            .map(OsString::from);
            let receipt = settle(
                super::super::pipe::execute_observed(
                    super::super::pipe::WorkerScope::observed(
                        profile,
                        &job,
                        &runtime.cache,
                        Arc::new(AtomicU64::new(0)),
                    ),
                    &runtime.cache.path().join("worker.exe"),
                    &args,
                    request,
                    &control,
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(receipt.reason(), EndReason::Exited);
            assert_eq!(receipt.exit_code(), Some(0), "{:?}", receipt);
        };
        invoke(&first, true, false);
        invoke(&second, false, true);
        first_code.release().unwrap();
        // A still has permission to execute the shared Host image, but no module grant.
        invoke(&first, false, false);
        invoke(&second, false, true);
        first_runtime.release().unwrap();
        invoke(&second, false, true);
        second_code.release().unwrap();
        invoke(&second, false, false);
        second_runtime.release().unwrap();
        drop((first_code, first_runtime, second_code, second_runtime));
        first.delete().unwrap();
        second.delete().unwrap();
    }
    #[test]
    fn an_acl_failure_poisoned_code_snapshot_cannot_be_reused() {
        let code = CodeFiles::new(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &[("node.wasm", b"synthetic code")],
            &Arc::new(CodeBudget::default()),
        )
        .unwrap();
        let path = code.cache.path().join("unexpected");
        let mut profile = Profile::new().unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(code.lease(&profile).is_err());
        std::fs::remove_dir(&path).unwrap();
        assert!(code.lease(&profile).is_err());
        profile.delete().unwrap();
    }
    #[test]
    fn inherited_code_cache_refuses_an_extra_file() {
        let mut code = CodeFiles::new(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &[("node.wasm", b"synthetic code")],
            &Arc::new(CodeBudget::default()),
        )
        .unwrap();
        code.inherited_acl = true;
        let mut profile = Profile::new().unwrap();
        let mut lease = code.lease(&profile).unwrap();
        lease.release().unwrap();
        drop(lease);

        let extra = code.cache.write("worker.exe", b"uncaptured file").unwrap();
        let rejected = code.lease(&profile).is_err();
        std::fs::remove_file(extra).unwrap();
        assert!(rejected);
        assert!(code.lease(&profile).is_err());
        profile.delete().unwrap();
    }
    #[test]
    fn retained_code_storage_is_bounded_across_images_and_released_after_cleanup() {
        let budget = Arc::new(CodeBudget {
            used: AtomicU64::new(0),
            limit: 8,
        });
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let first = CodeFiles::new(root, &[("node.wasm", b"12345")], &budget).unwrap();
        assert!(CodeFiles::new(root, &[("node.wasm", b"67890")], &budget).is_err());
        let path = first.cache.path().to_path_buf();
        drop(first);
        assert!(!path.exists());
        let second = CodeFiles::new(root, &[("node.wasm", b"67890")], &budget).unwrap();
        drop(second);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }
    #[test]
    fn live_code_snapshots_cannot_be_rewritten_replaced_or_deleted() {
        let code = CodeFiles::new(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &[("node.wasm", b"exact captured code")],
            &Arc::new(CodeBudget::default()),
        )
        .unwrap();
        let path = code.cache.path().join("node.wasm");
        assert!(std::fs::write(&path, b"changed code").is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert!(std::fs::rename(&path, path.with_file_name("replacement.wasm")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"exact captured code");
        drop(code);
        assert!(!path.exists());
    }
    #[test]
    fn partial_code_construction_cleans_files_before_returning_its_reservation() {
        let budget = Arc::new(CodeBudget {
            used: AtomicU64::new(0),
            limit: 16,
        });
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(
            CodeFiles::new(
                root,
                &[("node.wasm", b"first"), ("node.wasm", b"duplicate")],
                &budget
            )
            .is_err()
        );
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
        let code = CodeFiles::new(root, &[("node.wasm", b"retry")], &budget).unwrap();
        drop(code);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }
}
