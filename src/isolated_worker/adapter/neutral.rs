//! Host-owned late code, constructed only from this factory's settled compiler.
use super::*;

pub(in crate::isolated_worker) fn enabled() -> bool {
    [
        "DIY_EXPERIMENT_NEUTRAL_JIT_POOL",
        "DIY_EXPERIMENT_COMPILED_JIT",
    ]
    .iter()
    .all(|key| std::env::var_os(key).as_deref() == Some(std::ffi::OsStr::new("1")))
}

pub(in crate::isolated_worker) struct LateJitCode {
    source: WasmArtifact,
    captured: Arc<CapturedCode>,
}
// No Image/WasmArtifact/slot reference: storing this owner in an Image's cache
// cannot create a strong cycle that retains code/files/budget indefinitely.
pub(super) struct CapturedCode {
    // Drop the extra mapping/file pin BEFORE CodeFiles attempts physical cache
    // deletion. Reversing these fields would strand Windows files and budget.
    mapping: Option<super::super::runtime::code_mapping::CodeMapping>,
    files: Arc<CodeFiles>,
    bytes: Arc<[u8]>,
    digest: String,
    _charge: CodeReservation,
}
pub(super) type CodeCacheSlot = std::sync::Mutex<Option<Arc<CapturedCode>>>;
pub(super) fn cache_enabled() -> bool {
    enabled()
        && std::env::var_os("DIY_EXPERIMENT_NEUTRAL_CODE_CACHE").as_deref()
            == Some(std::ffi::OsStr::new("1"))
}
pub(super) fn mapping_enabled() -> bool {
    cache_enabled()
        && std::env::var_os("DIY_EXPERIMENT_NEUTRAL_CODE_MAPPING").as_deref()
            == Some(std::ffi::OsStr::new("1"))
}
impl CodeBudget {
    pub(super) fn code_cache_slot(self: &Arc<Self>) -> Arc<CodeCacheSlot> {
        let slot = Arc::new(std::sync::Mutex::new(None));
        let mut caches = self.code_caches.lock().unwrap_or_else(|p| p.into_inner());
        caches.retain(|known| known.strong_count() != 0);
        caches.push(Arc::downgrade(&slot));
        slot
    }
    pub(super) fn evict_code_caches(&self) {
        let mut caches = self.code_caches.lock().unwrap_or_else(|p| p.into_inner());
        caches.retain(|known| {
            let Some(slot) = known.upgrade() else {
                return false;
            };
            // Never block on a caller filling another slot under this budget.
            // Active invocations retain their OWN Arc and charge until real
            // process/IO/Job/ACL/profile cleanup, even after cache eviction.
            if let Ok(mut captured) = slot.try_lock() {
                captured.take();
            }
            true
        });
    }
}
impl LateJitCode {
    pub(in crate::isolated_worker) fn mapping(
        &self,
    ) -> Option<&super::super::runtime::code_mapping::CodeMapping> {
        self.captured.mapping.as_ref()
    }
    pub(in crate::isolated_worker) fn bytes(&self) -> &[u8] {
        &self.captured.bytes
    }
    pub(in crate::isolated_worker) fn digest(&self) -> &str {
        &self.captured.digest
    }
    pub(in crate::isolated_worker) fn check(&self) -> Result<(), String> {
        if self
            .captured
            .mapping
            .as_ref()
            .is_some_and(|m| !m.matches(self.captured.bytes.len(), &self.captured.digest))
        {
            return Err("mapped code binding changed".into());
        }
        self.captured
            .files
            .check_healthy()
            .map_err(|e| e.to_string())
    }
    pub(super) fn matches(&self, factory: &WasmArtifact) -> bool {
        self.source.same_runtime_factory(factory)
    }
}
impl WasmArtifact {
    fn same_runtime_factory(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.runtime_files, &other.0.runtime_files)
            && Arc::ptr_eq(&self.0.code_budget, &other.0.code_budget)
            && self.0.policy == other.0.policy
    }

    pub(super) fn late_jit_code(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Arc<LateJitCode>, CoreError> {
        let WorkerImage::PreparedCompiled(files, len, digest) = self.prepared_jit_image(cancel)
        else {
            return Err(fail(
                "worker-asset",
                "own compiled code is not ready for late delivery",
            ));
        };
        files.check_healthy()?;
        let capture = || self.capture_late_bytes(files.clone(), len, digest.clone());
        let captured = if let Some(slot) = &self.0.late_code_cache {
            let mut cached = slot.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(code) = cached.as_ref() {
                if code.digest != digest || !Arc::ptr_eq(&code.files, &files) {
                    return Err(fail("worker-identity", "cached code binding changed"));
                }
                code.clone()
            } else {
                let code = capture()?;
                *cached = Some(code.clone());
                code
            }
        } else {
            capture()?
        };
        Ok(Arc::new(LateJitCode {
            source: self.clone(),
            captured,
        }))
    }

    fn capture_late_bytes(
        &self,
        files: Arc<CodeFiles>,
        len: usize,
        digest: String,
    ) -> Result<Arc<CapturedCode>, CoreError> {
        let mapped = mapping_enabled();
        // Include the additional read-only view's worst-case resident bytes.
        let charge = self
            .0
            .code_budget
            .reserve((if mapped { 3 } else { 2 }) * (len as u64 + 1))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len + 1)
            .map_err(|_| fail("worker-asset", "late code allocation unavailable"))?;
        std::fs::File::open(files.cache.path().join("node.cwasm"))
            .and_then(|file| file.take(len as u64 + 1).read_to_end(&mut bytes))
            .map_err(|_| fail("worker-asset", "late code capture failed"))?;
        if bytes.len() != len || sha256(&bytes)? != digest {
            return Err(fail("worker-identity", "late compiled code changed"));
        }
        files.check_healthy()?;
        let mapping = if mapped {
            if files.locks.len() != 1 {
                return Err(fail(
                    "worker-identity",
                    "compiled code file binding changed",
                ));
            }
            // SAFETY: only this source's settled CompileJit creates these files;
            // private snapshot locks deny writes/deletes, with no prior writable
            // mapping or guest access. The payload pins both files and view.
            Some(
                unsafe {
                    super::super::runtime::code_mapping::CodeMapping::capture_locked(
                        &files.locks[0],
                        len,
                        &digest,
                    )
                }
                .map_err(|_| fail("worker-identity", "immutable code mapping unavailable"))?,
            )
        } else {
            None
        };
        Ok(Arc::new(CapturedCode {
            files,
            bytes: Arc::from(bytes),
            mapping,
            digest,
            _charge: charge,
        }))
    }
}

impl WasmNode {
    pub(in crate::isolated_worker) fn neutral_jit_pool_enabled() -> bool {
        enabled()
    }
    pub(in crate::isolated_worker) fn shares_runtime_factory(&self, other: &Self) -> bool {
        self.can_prepare_jit()
            && other.can_prepare_jit()
            && self.artifact.same_runtime_factory(&other.artifact)
    }
    pub(in crate::isolated_worker) fn warm_neutral_code(&self, cancel: &CancellationToken) {
        // Caller owns a physical preparation reservation. No input or guest
        // execution is involved; failure leaves the ordinary cold path usable.
        let _ = self.artifact.prepared_jit_image(cancel);
    }
    pub(in crate::isolated_worker) fn attach_jit_slot(&self, slot: &Arc<PreparedJitSlot>) {
        let mut slots = self.prepared_jit.lock().unwrap_or_else(|p| p.into_inner());
        if !slots
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .any(|known| Arc::ptr_eq(&known, slot))
        {
            slots.push(Arc::downgrade(slot));
        }
    }
}

#[cfg(test)]
mod tests;
