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
    files: Arc<CodeFiles>,
    bytes: Arc<[u8]>,
    digest: String,
    _charge: CodeReservation,
}
impl LateJitCode {
    pub(in crate::isolated_worker) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(in crate::isolated_worker) fn digest(&self) -> &str {
        &self.digest
    }
    pub(in crate::isolated_worker) fn check(&self) -> Result<(), String> {
        self.files.check_healthy().map_err(|e| e.to_string())
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
        let charge = self.0.code_budget.reserve(2 * (len as u64 + 1))?;
        files.check_healthy()?;
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
        Ok(Arc::new(LateJitCode {
            source: self.clone(),
            files,
            bytes: Arc::from(bytes),
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
