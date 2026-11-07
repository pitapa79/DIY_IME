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
impl PreparedArtifact {
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
