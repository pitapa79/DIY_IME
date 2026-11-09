//! Construction-only originals and opaque bounded output. No dictionary parser.
use super::{
    ExecutionGate, GrantTarget, ResourceRangeLimits, ResourceRangeSession, fail, sha256, wire,
};
use crate::isolated_worker::{EndReason, ExitReceipt, resource_transport::MAX_READ};
use crate::{
    execution_control::{CancellationToken, RunKey},
    node::CoreError,
};
use diy_ime2_wasm_resource_sdk::PreparationAcknowledgement;
use std::{collections::BTreeMap, sync::Arc};

/// Trusted composition binds these fingerprints to the actual verified module,
/// Node definition and exact configuration. Guest fields are never this proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparationIdentity {
    pub producer_sha256: String,
    pub definition_sha256: String,
    pub config_sha256: String,
}
impl PreparationIdentity {
    pub(in crate::isolated_worker) fn validate(&self) -> Result<(), CoreError> {
        if [
            &self.producer_sha256,
            &self.definition_sha256,
            &self.config_sha256,
        ]
        .into_iter()
        .all(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            Ok(())
        } else {
            Err(fail(
                "preparation-identity",
                "invalid preparation fingerprint",
            ))
        }
    }
}

/// Budgets are native Host policy, never guest supplied. Resident budget covers
/// capture + contiguous finalization copy + one bounded transport chunk.
#[derive(Clone, Debug)]
pub struct PreparationLimits {
    pub max_artifact_bytes: usize,
    pub max_chunks: u32,
    pub max_resident_bytes: usize,
}

pub struct PreparationSources {
    pub(in crate::isolated_worker) ranges: ResourceRangeSession,
    pub(in crate::isolated_worker) source_sha256: String,
}
impl PreparationSources {
    /// Original bytes must already be captured under the exact configuration's
    /// resource grants. This does not mark a dictionary prepared or active.
    pub fn open(
        gate: ExecutionGate,
        target: GrantTarget,
        key: RunKey,
        cancel: CancellationToken,
        originals: BTreeMap<String, Arc<[u8]>>,
        limits: ResourceRangeLimits,
    ) -> Result<Self, CoreError> {
        let mut ranges =
            ResourceRangeSession::open_captured(gate, target, key, cancel, originals, limits)?;
        let source_sha256 = ranges.snapshot_identity()?;
        Ok(Self {
            ranges,
            source_sha256,
        })
    }
}

/// Native construction evidence, not a guarantee of dictionary semantics.
/// Only verified official composition may connect it to the execution route.
pub struct PreparedArtifact {
    bytes: Arc<[u8]>,
    identity: PreparationIdentity,
    source_sha256: String,
    artifact_sha256: String,
    format: String,
    format_version: u32,
}

/// Session-local consumer code and the immutable bytes produced for its
/// approved configuration. The original plan remains the execution grant.
pub(super) struct PreparedDictionary {
    pub(super) consumer: super::WasmArtifact,
    pub(super) index: PreparedArtifact,
}

/// Both modules were captured from the pinned official companion release.
pub(in crate::isolated_worker) struct DictionaryCompanions {
    pub(in crate::isolated_worker) producer: Vec<u8>,
    pub(in crate::isolated_worker) consumer: Vec<u8>,
}

impl super::WasmNode {
    pub(super) fn initialize_prepared_dictionary(
        &self,
        modules: &DictionaryCompanions,
        key: &RunKey,
        id: &str,
        config: &serde_json::Value,
        originals: &BTreeMap<String, Arc<[u8]>>,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        use crate::isolated_worker::PreparationWorkerRequest;
        use serde_json::json;
        use std::time::Duration;

        const ORIGINAL_LIMIT: usize = 1024 * 1024;
        const ARTIFACT_LIMIT: usize = 4 * 1024 * 1024;
        let mut entries = originals.iter();
        let (reference, original) = entries
            .next()
            .ok_or_else(|| fail("dictionary-not-prepared", "original dictionary missing"))?;
        if self.definition.kind != "text.dictionary-exact"
            || self.definition.version != 1
            || config.get("path").and_then(serde_json::Value::as_str) != Some(reference.as_str())
            || entries.next().is_some()
            || original.len() > ORIGINAL_LIMIT
        {
            return Err(fail(
                "dictionary-not-prepared",
                "invalid prepared dictionary plan",
            ));
        }
        let policy = &self.artifact.0.policy;
        let producer = self
            .artifact
            .with_module(modules.producer.clone(), policy.clone())?;
        let consumer = self
            .artifact
            .with_module(modules.consumer.clone(), policy.clone())?;
        let source = PreparationSources::open(
            self.gate.clone(),
            self.grant_target(&key.session_epoch, key.plan_version, id),
            key.clone(),
            cancel.clone(),
            originals.clone(),
            ResourceRangeLimits {
                max_resources: 1,
                max_snapshot_bytes: ORIGINAL_LIMIT,
                max_read_bytes: MAX_READ,
                max_total_read_bytes: ORIGINAL_LIMIT,
                max_reads: 4096,
                budget: policy.walltime.unwrap_or(Duration::from_secs(30)),
            },
        )?;
        let mut resources = serde_json::Map::new();
        resources.insert(reference.clone(), json!({"id":1,"length":original.len()}));
        let input = serde_json::to_vec(&wire::request(
            &self.definition.kind,
            self.definition.version,
            json!({"method":"prepareArtifact","config":config,"resources":resources}),
        ))
        .map_err(|_| fail("worker-request", "producer request serialization failed"))?;
        let fingerprint = |value: &serde_json::Value| {
            sha256(
                &serde_json::to_vec(value)
                    .map_err(|_| fail("preparation-identity", "identity serialization failed"))?,
            )
        };
        let index = producer
            .prepare_artifact(
                PreparationWorkerRequest {
                    input,
                    sources: source,
                    identity: PreparationIdentity {
                        producer_sha256: sha256(&modules.producer)?,
                        definition_sha256: fingerprint(&json!(*self.definition))?,
                        config_sha256: fingerprint(config)?,
                    },
                    limits: PreparationLimits {
                        max_artifact_bytes: ARTIFACT_LIMIT,
                        max_chunks: 4096,
                        max_resident_bytes: 2 * ARTIFACT_LIMIT + MAX_READ,
                    },
                },
                cancel,
            )?
            .into_artifact()?;
        if index.format() != "dictionary-exact-index" || index.format_version() != 1 {
            return Err(fail(
                "dictionary-not-prepared",
                "unexpected dictionary artifact format",
            ));
        }
        self.prepared_dictionary
            .set(PreparedDictionary { consumer, index })
            .map_err(|_| fail("dictionary-not-prepared", "dictionary was already prepared"))
    }
}

impl PreparedDictionary {
    pub(super) fn request(
        &self,
        invocation: &crate::execution_control::OwnedInvocation,
        definition: &crate::node::NodeDefinition,
        scope: &mut ResourceRangeSession,
    ) -> Result<serde_json::Value, CoreError> {
        let mut request = wire::execution(invocation, definition)?;
        request["operation"]["method"] = serde_json::json!("executePrepared");
        request["operation"]["artifact"] = serde_json::to_value(self.descriptor(scope)?)
            .map_err(|_| fail("worker-request", "artifact descriptor unavailable"))?;
        Ok(request)
    }

    pub(super) fn execute(
        &self,
        input: Vec<u8>,
        resources: ResourceRangeSession,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, CoreError> {
        use crate::isolated_worker::{EndReason, ResourceWorkerRequest, execute_resources};
        self.consumer.with_resource_worker(
            "--resource-ranges-v1",
            cancel,
            |profile, job, cache, program, args, control| {
                let receipt = super::settle(
                    execute_resources(
                        profile,
                        job,
                        cache,
                        program,
                        args,
                        ResourceWorkerRequest { input, resources },
                        control,
                    )
                    .map_err(|_| fail("worker-start", "dictionary lookup could not start"))?,
                )
                .ok_or_else(|| fail("worker-cancelled", "dictionary lookup was not started"))?;
                if receipt.reason() != EndReason::Exited
                    || receipt.exit_code() != Some(0)
                    || !receipt.input_delivered()
                {
                    return Err(fail("worker-failed", "isolated dictionary lookup failed"));
                }
                receipt
                    .output()
                    .map(<[u8]>::to_vec)
                    .map_err(|_| fail("worker-reply", "dictionary output unavailable"))
            },
        )
    }

    pub(super) fn scope(
        &self,
        node: &super::WasmNode,
        id: &str,
        key: &RunKey,
        cancel: CancellationToken,
        budget: std::time::Duration,
    ) -> Result<ResourceRangeSession, CoreError> {
        let bytes = self.index.bytes();
        if self.index.format() != "dictionary-exact-index"
            || self.index.format_version() != 1
            || bytes.is_empty()
            || bytes.len() > 4 * 1024 * 1024
        {
            return Err(fail(
                "dictionary-not-prepared",
                "invalid dictionary artifact",
            ));
        }
        ResourceRangeSession::open(
            node.gate.clone(),
            node.grant_target(&key.session_epoch, key.plan_version, id),
            key.clone(),
            cancel,
            BTreeMap::from([("prepared.bin".into(), bytes.clone())]),
            ResourceRangeLimits {
                max_resources: 1,
                max_snapshot_bytes: 4 * 1024 * 1024,
                max_read_bytes: MAX_READ,
                max_total_read_bytes: 8 * 1024 * 1024,
                max_reads: 4096,
                budget,
            },
        )
    }

    pub(super) fn descriptor(
        &self,
        scope: &mut ResourceRangeSession,
    ) -> Result<diy_ime2_wasm_resource_sdk::ArtifactDescriptor, CoreError> {
        use diy_ime2_wasm_resource_sdk::{ArtifactDescriptor, ResourceDescriptor};
        let resources = scope.descriptions()?;
        let [resource] = resources.as_slice() else {
            return Err(fail("dictionary-not-prepared", "missing dictionary scope"));
        };
        if resource.reference != "prepared.bin" || resource.length != self.index.bytes().len() {
            return Err(fail("dictionary-not-prepared", "dictionary scope differs"));
        }
        Ok(ArtifactDescriptor {
            resource: ResourceDescriptor {
                id: resource.id,
                length: resource.length as u64,
            },
            format: self.index.format().into(),
            version: self.index.format_version(),
        })
    }
}

impl super::WasmArtifact {
    /// Run the construction role with immutable code and keep the range
    /// connection owned until the original process and its IO are settled.
    pub(super) fn prepare_artifact(
        &self,
        request: crate::isolated_worker::PreparationWorkerRequest,
        cancel: CancellationToken,
    ) -> Result<crate::isolated_worker::PreparationExit, CoreError> {
        use crate::isolated_worker::{PreparationOutcome, execute_preparation};
        use std::time::Duration;

        if self.identity().rsplit(':').next() != Some(&request.identity.producer_sha256) {
            return Err(fail(
                "preparation-identity",
                "producer code identity differs",
            ));
        }
        self.with_resource_worker(
            "--build-resources-v1",
            cancel,
            |profile, job, cache, program, args, control| {
                let mut outcome =
                    execute_preparation(profile, job, cache, program, args, request, control)
                        .map_err(|_| fail("worker-start", "dictionary producer could not start"))?;
                loop {
                    match outcome {
                        PreparationOutcome::Exited(exit) => return Ok(exit),
                        PreparationOutcome::Unconfirmed(owner) => {
                            outcome = owner.retry(Duration::from_millis(100));
                        }
                        PreparationOutcome::CancelledBeforeStart => {
                            return Err(fail("worker-cancelled", "producer was not started"));
                        }
                    }
                }
            },
        )
    }

    fn with_resource_worker<T>(
        &self,
        role: &str,
        cancel: CancellationToken,
        run: impl FnOnce(
            &super::Profile,
            &super::Job,
            &super::Cache,
            &std::path::Path,
            &[std::ffi::OsString],
            &super::RunControl,
        ) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        use super::{Job, Profile, RunControl};
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "resource Worker was not started"));
        }
        let fuel = self.0.policy.fuel_per_call.ok_or_else(|| {
            fail(
                "worker-policy",
                "resource Worker requires a bounded fuel policy",
            )
        })?;
        let memory = self.0.policy.wasm_memory_bytes.ok_or_else(|| {
            fail(
                "worker-policy",
                "resource Worker requires a bounded Wasm memory policy",
            )
        })?;
        let mut profile =
            Profile::new().map_err(|_| fail("worker-isolation", "AppContainer unavailable"))?;
        let runtime = self.runtime_code_files()?;
        let module = self.module_code_files()?;
        let mut runtime_lease = runtime.lease(&profile)?;
        let result = (|| {
            let mut module_lease = module.lease(&profile)?;
            let result = (|| {
                let job = Job::with_memory_limit(self.0.policy.process_memory_bytes)
                    .map_err(|_| fail("worker-isolation", "Worker Job unavailable"))?;
                let control = RunControl {
                    cancel,
                    walltime: self.0.policy.walltime,
                    termination_grace: self.0.policy.termination_grace,
                };
                let args = [
                    role.into(),
                    runtime.cache.path().join("wasmtime.dll").into_os_string(),
                    module.cache.path().join("node.wasm").into_os_string(),
                    self.0.module.len().to_string().into(),
                    fuel.to_string().into(),
                    memory.to_string().into(),
                ];
                run(
                    &profile,
                    &job,
                    &runtime.cache,
                    &runtime.cache.path().join("worker.exe"),
                    &args,
                    &control,
                )
            })();
            module_lease.release()?;
            result
        })();
        runtime_lease.release()?;
        drop(runtime_lease);
        profile
            .delete()
            .map_err(|_| fail("worker-cleanup", "resource Worker profile cleanup failed"))?;
        result
    }
}
impl PreparedArtifact {
    #[cfg(test)]
    pub(super) fn retained_fixture() -> Self {
        Self {
            bytes: Arc::from(&b"fixture"[..]),
            identity: PreparationIdentity {
                producer_sha256: "0".repeat(64),
                definition_sha256: "0".repeat(64),
                config_sha256: "0".repeat(64),
            },
            source_sha256: "0".repeat(64),
            artifact_sha256: "0".repeat(64),
            format: "dictionary-exact-index".into(),
            format_version: 1,
        }
    }

    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
    pub fn identity(&self) -> &PreparationIdentity {
        &self.identity
    }
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }
    pub fn artifact_sha256(&self) -> &str {
        &self.artifact_sha256
    }
    pub fn format(&self) -> &str {
        &self.format
    }
    pub fn format_version(&self) -> u32 {
        self.format_version
    }
}

pub(in crate::isolated_worker) struct ArtifactCapture {
    bytes: Vec<u8>,
    limits: PreparationLimits,
    chunks: u32,
    pending: Option<usize>,
    completed: bool,
}
impl ArtifactCapture {
    pub(in crate::isolated_worker) fn new(limits: PreparationLimits) -> Result<Self, CoreError> {
        let resident = limits
            .max_artifact_bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(MAX_READ));
        if limits.max_artifact_bytes == 0
            || limits.max_artifact_bytes > u32::MAX as usize
            || limits.max_chunks == 0
            || resident.is_none_or(|n| n > limits.max_resident_bytes)
        {
            return Err(fail("preparation-limit", "invalid finite artifact budgets"));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(limits.max_artifact_bytes)
            .map_err(|_| fail("preparation-limit", "artifact reservation unavailable"))?;
        Ok(Self {
            bytes,
            limits,
            chunks: 0,
            pending: None,
            completed: false,
        })
    }
    pub(in crate::isolated_worker) fn reserve(
        &mut self,
        offset: u64,
        length: usize,
    ) -> Result<(), CoreError> {
        let end = self.bytes.len().checked_add(length);
        if self.completed
            || self.pending.is_some()
            || length == 0
            || length > MAX_READ
            || offset != self.bytes.len() as u64
            || self.chunks >= self.limits.max_chunks
            || end.is_none_or(|n| n > self.limits.max_artifact_bytes)
        {
            return Err(fail("preparation-chunk", "invalid ordered artifact chunk"));
        }
        self.chunks += 1;
        self.pending = Some(length);
        Ok(())
    }
    pub(in crate::isolated_worker) fn append(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        if self.pending.take() != Some(bytes.len()) {
            return Err(fail("preparation-chunk", "incomplete artifact chunk"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub(in crate::isolated_worker) fn complete(&mut self) -> Result<(), CoreError> {
        if self.pending.is_some() || self.bytes.is_empty() || self.completed {
            return Err(fail("preparation-chunk", "artifact is incomplete"));
        }
        self.completed = true;
        Ok(())
    }
    pub(in crate::isolated_worker) fn complete_reply(
        &mut self,
        reply: &[u8],
    ) -> Result<(), CoreError> {
        // A fully validated error has no artifact. Keep the reply available for
        // diagnosis only after the same true-exit/authority checks in settle.
        // Success still requires nonempty, complete captured output and an ack.
        if self.bytes.is_empty()
            && self.pending.is_none()
            && !self.completed
            && wire::is_error_reply(reply)
        {
            self.completed = true;
            Ok(())
        } else {
            self.complete()
        }
    }
    pub(in crate::isolated_worker) fn settle(
        &mut self,
        receipt: &ExitReceipt,
        ranges: &mut ResourceRangeSession,
        identity: PreparationIdentity,
        source_sha256: String,
    ) -> Result<PreparedArtifact, CoreError> {
        if !self.completed
            || receipt.reason() != EndReason::Exited
            || receipt.exit_code() != Some(0)
            || !receipt.input_delivered()
        {
            return Err(fail(
                "preparation-exit",
                "construction did not finish normally",
            ));
        }
        ranges.check_current()?;
        identity.validate()?;
        let reply = receipt
            .output()
            .map_err(|_| fail("preparation-reply", "construction reply unavailable"))?;
        let acknowledgement: PreparationAcknowledgement =
            serde_json::from_value(wire::reply(reply)?)
                .map_err(|_| fail("preparation-reply", "invalid construction acknowledgement"))?;
        if acknowledgement.validate().is_err()
            || acknowledgement.artifact_bytes != self.bytes.len() as u64
        {
            return Err(fail(
                "preparation-reply",
                "construction acknowledgement mismatch",
            ));
        }
        let artifact_sha256 = sha256(&self.bytes)?;
        ranges.check_current()?;
        let bytes = Arc::from(std::mem::take(&mut self.bytes));
        // Copy/finalization cannot publish after a concurrent withdrawal.
        ranges.check_current()?;
        Ok(PreparedArtifact {
            bytes,
            identity,
            source_sha256,
            artifact_sha256,
            format: acknowledgement.artifact_format,
            format_version: acknowledgement.artifact_version,
        })
    }
}

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
