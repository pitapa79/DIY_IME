//! Host-owned, run-scoped reads from already validated immutable resources.
use super::gate::{ExecutionGate, GrantTarget, Lease};
use super::{dictionary, fail};
use crate::{
    execution_control::{CancellationToken, DecimalId, RunKey},
    node::CoreError,
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

/// Trusted Host policy. These limits are not supplied by the guest.
#[derive(Clone, Debug)]
pub struct ResourceRangeLimits {
    pub max_resources: u32,
    pub max_snapshot_bytes: usize,
    pub max_read_bytes: usize,
    pub max_total_read_bytes: usize,
    pub max_reads: u32,
    pub budget: Duration,
}

impl ResourceRangeLimits {
    fn deadline(&self) -> Result<Instant, CoreError> {
        if self.max_resources == 0
            || self.max_snapshot_bytes == 0
            || self.max_read_bytes == 0
            || self.max_total_read_bytes == 0
            || self.max_reads == 0
            || self.budget.is_zero()
        {
            return Err(fail("resource-range-config", "invalid fixed range policy"));
        }
        Instant::now()
            .checked_add(self.budget)
            .ok_or_else(|| fail("resource-range-config", "invalid range deadline"))
    }
}

/// An ID is meaningful only on the connection owned by this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceRangeDescription {
    pub id: u32,
    pub reference: String,
    pub length: usize,
}

/// Native transport must obtain the session from its Host owner, not this request.
#[derive(Clone, Debug)]
pub struct ResourceRangeRequest {
    pub request_id: DecimalId,
    pub resource_id: u32,
    pub offset: u64,
    pub length: u32,
}

/// Keeps the selected immutable bytes and the actual authorization lease alive.
/// A returned buffer is not proof of Worker ending or permission to publish it.
pub struct ResourceRangeSession {
    #[cfg(test)]
    pub(super) after_read: Option<Box<dyn FnMut() + Send>>,
    gate: ExecutionGate,
    lease: Arc<Lease>,
    key: RunKey,
    cancel: CancellationToken,
    resources: Vec<(String, Arc<[u8]>)>,
    limits: ResourceRangeLimits,
    deadline: Instant,
    next_request: u64,
    reads: u32,
    bytes: usize,
    closed: bool,
}

impl ResourceRangeSession {
    /// Host composition supplies only the resources of this exact validated
    /// configuration after complete successful preparation. This constructor
    /// enforces authorization and budgets; it does not validate dictionary data.
    pub fn open(
        gate: ExecutionGate,
        target: GrantTarget,
        key: RunKey,
        cancel: CancellationToken,
        resources: BTreeMap<String, Arc<[u8]>>,
        limits: ResourceRangeLimits,
    ) -> Result<Self, CoreError> {
        Self::open_captured(gate, target, key, cancel, resources, limits)
    }

    // Construction-only caller supplies authorized immutable originals, not
    // yet semantically validated dictionary artifacts. Never an execution opt-in.
    pub(super) fn open_captured(
        gate: ExecutionGate,
        target: GrantTarget,
        key: RunKey,
        cancel: CancellationToken,
        resources: BTreeMap<String, Arc<[u8]>>,
        limits: ResourceRangeLimits,
    ) -> Result<Self, CoreError> {
        let deadline = limits.deadline()?;
        if resources.is_empty()
            || resources.len() > limits.max_resources as usize
            || resources
                .keys()
                .any(|reference| !dictionary::reference(reference))
        {
            return Err(fail(
                "resource-range-config",
                "invalid prepared resource set",
            ));
        }
        let total = resources
            .values()
            .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()));
        if total.is_none_or(|total| total > limits.max_snapshot_bytes) {
            return Err(fail(
                "resource-range-limit",
                "prepared resources exceed policy",
            ));
        }
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "resource run is cancelled"));
        }
        let lease = gate.begin(target, &key, cancel.clone())?;
        let mut session = Self {
            #[cfg(test)]
            after_read: None,
            gate,
            lease,
            key,
            cancel,
            resources: resources.into_iter().collect(),
            limits,
            deadline,
            next_request: 1,
            reads: 0,
            bytes: 0,
            closed: false,
        };
        session.check_current()?;
        Ok(session)
    }

    /// Recheck this actual lease before the native owner commits a delivery.
    pub fn check_current(&mut self) -> Result<(), CoreError> {
        self.authorized(|_| Ok(()))
    }

    pub(in crate::isolated_worker) fn cancellation_matches(
        &self,
        other: &CancellationToken,
    ) -> bool {
        self.cancel.same_source(other)
    }

    pub(in crate::isolated_worker) fn withdraw(&mut self) {
        self.closed = true;
        self.cancel.cancel();
    }

    pub(in crate::isolated_worker) fn remaining_budget(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
    pub(in crate::isolated_worker) fn absolute_deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn snapshot_identity(&mut self) -> Result<String, CoreError> {
        self.check_current()?;
        // Hash outside the gate mutex; immutable originals remain owned here.
        let descriptions = self.resources.iter().map(|(name, bytes)| {
            Ok(serde_json::json!({"reference":name,"length":bytes.len(),"sha256":super::sha256(bytes)?}))
        }).collect::<Result<Vec<_>, CoreError>>()?;
        let encoded = serde_json::to_vec(&descriptions)
            .map_err(|_| fail("preparation-identity", "source identity unavailable"))?;
        let identity = super::sha256(&encoded)?;
        self.check_current()?;
        Ok(identity)
    }

    pub fn descriptions(&mut self) -> Result<Vec<ResourceRangeDescription>, CoreError> {
        self.authorized(|session| {
            Ok(session
                .resources
                .iter()
                .enumerate()
                .map(|(index, (reference, bytes))| ResourceRangeDescription {
                    id: index as u32 + 1,
                    reference: reference.clone(),
                    length: bytes.len(),
                })
                .collect())
        })
    }

    /// Commits an ordered, budgeted response in memory under the gate lock.
    /// The transport must recheck withdrawal before delivery and retain its
    /// original process/IO owner through actual termination.
    pub fn read(&mut self, request: &ResourceRangeRequest) -> Result<Vec<u8>, CoreError> {
        let result = self.authorized(|session| session.read_current(request));
        #[cfg(test)]
        if result.is_ok()
            && let Some(hook) = &mut self.after_read
        {
            hook();
        }
        result
    }

    fn read_current(&mut self, request: &ResourceRangeRequest) -> Result<Vec<u8>, CoreError> {
        if request.request_id.get() != self.next_request || request.length == 0 {
            return Err(fail(
                "resource-range-request",
                "invalid ordered range request",
            ));
        }
        let resource = request
            .resource_id
            .checked_sub(1)
            .and_then(|index| self.resources.get(index as usize))
            .ok_or_else(|| fail("resource-range-request", "resource is outside this session"))?;
        let end = request
            .offset
            .checked_add(u64::from(request.length))
            .and_then(|end| usize::try_from(end).ok())
            .ok_or_else(|| fail("resource-range-request", "range exceeds the resource"))?;
        if end > resource.1.len() {
            return Err(fail("resource-range-request", "range exceeds the resource"));
        }
        let length = request.length as usize;
        let total = self
            .bytes
            .checked_add(length)
            .filter(|total| *total <= self.limits.max_total_read_bytes)
            .ok_or_else(|| fail("resource-range-limit", "range read budget exhausted"))?;
        if length > self.limits.max_read_bytes || self.reads >= self.limits.max_reads {
            return Err(fail("resource-range-limit", "range read budget exhausted"));
        }
        self.bytes = total;
        self.reads += 1;
        self.next_request += 1;
        // end is checked against the captured resource above, so this subtraction
        // also proves that the start fits usize and stays within the same bytes.
        Ok(resource.1[end - length..end].to_vec())
    }

    fn current(&self) -> Result<(), CoreError> {
        if self.closed || self.cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "resource run is cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(fail(
                "resource-range-deadline",
                "range read deadline exceeded",
            ));
        }
        Ok(())
    }

    fn authorized<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let gate = self.gate.clone();
        let lease = self.lease.clone();
        let key = self.key.clone();
        let result = self.current().and_then(|()| {
            gate.publish(&lease, &key, || {
                self.current()?;
                let response = operation(self)?;
                self.current()?;
                Ok(response)
            })?
        });
        if result.is_err() {
            self.closed = true;
            self.cancel.cancel();
        }
        result
    }
}

#[cfg(test)]
#[path = "range_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn limits() -> ResourceRangeLimits {
        ResourceRangeLimits {
            max_resources: 2,
            max_snapshot_bytes: 16,
            max_read_bytes: 4,
            max_total_read_bytes: 6,
            max_reads: 2,
            budget: Duration::from_secs(30),
        }
    }

    pub(super) fn values() -> (ExecutionGate, GrantTarget, RunKey) {
        let gate = ExecutionGate::default();
        let key: RunKey = serde_json::from_value(serde_json::json!({
            "sessionEpoch":"11111111111111111111111111111111",
            "planVersion":"1", "runId":"1", "deliveryGeneration":"1"
        }))
        .unwrap();
        let target = GrantTarget::new(
            "verified-code".into(),
            &key.session_epoch,
            key.plan_version,
            "dictionary".into(),
        );
        gate.approve(target.clone());
        (gate, target, key)
    }

    pub(super) fn resources() -> BTreeMap<String, Arc<[u8]>> {
        BTreeMap::from([
            ("a.bin".into(), Arc::from(&b"abcdefgh"[..])),
            ("b.bin".into(), Arc::from(&b"12345678"[..])),
        ])
    }

    fn open(policy: ResourceRangeLimits) -> ResourceRangeSession {
        let (gate, target, key) = values();
        ResourceRangeSession::open(
            gate,
            target,
            key,
            CancellationToken::default(),
            resources(),
            policy,
        )
        .unwrap()
    }

    fn request(id: u64, resource: u32, offset: u64, length: u32) -> ResourceRangeRequest {
        ResourceRangeRequest {
            request_id: DecimalId::new(id).unwrap(),
            resource_id: resource,
            offset,
            length,
        }
    }

    #[test]
    fn exact_snapshot_and_resource_identity_survive_caller_mutation() {
        let mut originals = resources();
        let (gate, target, key) = values();
        let mut session = ResourceRangeSession::open(
            gate,
            target,
            key,
            CancellationToken::default(),
            originals.clone(),
            limits(),
        )
        .unwrap();
        Arc::make_mut(originals.get_mut("a.bin").unwrap())[0] = b'X';
        let descriptions = session.descriptions().unwrap();
        assert_eq!(
            descriptions,
            vec![
                ResourceRangeDescription {
                    id: 1,
                    reference: "a.bin".into(),
                    length: 8
                },
                ResourceRangeDescription {
                    id: 2,
                    reference: "b.bin".into(),
                    length: 8
                },
            ]
        );
        let mut first = session.read(&request(1, 1, 0, 2)).unwrap();
        assert_eq!(first, b"ab");
        first[0] = b'Y';
        assert_eq!(session.read(&request(2, 2, 6, 2)).unwrap(), b"78");
        assert_eq!(session.resources[0].1.as_ref(), b"abcdefgh");
    }

    #[test]
    fn resource_access_requires_the_exact_granted_target_and_run_scope() {
        let (gate, target, key) = values();
        for other in [
            GrantTarget {
                artifact: "other".into(),
                ..target.clone()
            },
            GrantTarget {
                session: "22222222222222222222222222222222".into(),
                ..target.clone()
            },
            GrantTarget {
                plan: 2,
                ..target.clone()
            },
            GrantTarget {
                node: "other".into(),
                ..target.clone()
            },
        ] {
            assert!(
                ResourceRangeSession::open(
                    gate.clone(),
                    other,
                    key.clone(),
                    CancellationToken::default(),
                    resources(),
                    limits()
                )
                .is_err()
            );
        }
        gate.revoke(&target);
        assert!(
            ResourceRangeSession::open(
                gate,
                target,
                key,
                CancellationToken::default(),
                resources(),
                limits()
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_ranges_close_the_session_and_cancel_its_owner() {
        for invalid in [
            request(1, 0, 0, 1),
            request(1, 3, 0, 1),
            request(1, 1, u64::MAX, 1),
            request(1, 1, 7, 2),
            request(1, 1, 0, 0),
            request(2, 1, 0, 1),
        ] {
            let mut session = open(limits());
            assert!(session.read(&invalid).is_err());
            assert!(session.cancel.is_cancelled());
            assert!(session.read(&request(1, 1, 0, 1)).is_err());
            assert!(session.descriptions().is_err());
        }
    }

    #[test]
    fn a_request_cannot_be_replayed_or_reordered() {
        let mut session = open(limits());
        let first = request(1, 1, 0, 1);
        assert_eq!(session.read(&first).unwrap(), b"a");
        assert!(session.read(&first).is_err());
        assert!(session.read(&request(2, 1, 1, 1)).is_err());
    }

    #[test]
    fn read_and_aggregate_byte_limits_are_checked_before_copying() {
        let mut session = open(limits());
        assert!(session.read(&request(1, 1, 0, 5)).is_err());

        let mut session = open(limits());
        assert_eq!(session.read(&request(1, 1, 0, 4)).unwrap(), b"abcd");
        assert!(session.read(&request(2, 1, 4, 3)).is_err());
        assert_eq!(session.bytes, 4);

        let mut session = open(limits());
        assert_eq!(session.read(&request(1, 1, 0, 4)).unwrap(), b"abcd");
        assert_eq!(session.read(&request(2, 1, 4, 2)).unwrap(), b"ef");
        assert_eq!(session.bytes, 6);
    }

    #[test]
    fn read_count_applies_to_small_reads_too() {
        let mut session = open(ResourceRangeLimits {
            max_reads: 1,
            ..limits()
        });
        assert_eq!(session.read(&request(1, 1, 0, 1)).unwrap(), b"a");
        assert!(session.read(&request(2, 1, 1, 1)).is_err());
        assert_eq!(session.reads, 1);
    }

    #[test]
    fn revocation_and_reapproval_do_not_restore_old_snapshots() {
        let mut session = open(limits());
        let (_, target, _) = values();
        session.gate.revoke(&target);
        session.gate.approve(target);
        assert!(session.read(&request(1, 1, 0, 1)).is_err());
        assert!(session.cancel.is_cancelled());
    }

    #[test]
    fn either_protection_mode_invalidates_the_old_session_after_release() {
        for (manual, automatic) in [(true, false), (false, true), (true, true)] {
            let mut session = open(limits());
            session.gate.set_protection(manual, automatic);
            session.gate.set_protection(false, false);
            assert!(session.read(&request(1, 1, 0, 1)).is_err());
        }
    }

    #[test]
    fn cancellation_and_deadline_are_checked_again_before_response_commit() {
        let mut session = open(limits());
        let cancel = session.cancel.clone();
        let result = session.authorized(|_| {
            cancel.cancel();
            Ok(vec![42])
        });
        assert!(result.is_err());

        let mut session = open(limits());
        session.deadline = Instant::now();
        let error = session.read(&request(1, 1, 0, 1)).unwrap_err();
        assert_eq!(error.code, "resource-range-deadline");
        assert_eq!(session.bytes, 0);
    }

    #[test]
    fn invalid_host_policy_and_unbounded_snapshot_sets_are_refused() {
        let (gate, target, key) = values();
        for policy in [
            ResourceRangeLimits {
                max_resources: 0,
                ..limits()
            },
            ResourceRangeLimits {
                max_snapshot_bytes: 15,
                ..limits()
            },
            ResourceRangeLimits {
                max_resources: 1,
                ..limits()
            },
            ResourceRangeLimits {
                max_read_bytes: 0,
                ..limits()
            },
            ResourceRangeLimits {
                max_total_read_bytes: 0,
                ..limits()
            },
            ResourceRangeLimits {
                max_reads: 0,
                ..limits()
            },
            ResourceRangeLimits {
                budget: Duration::ZERO,
                ..limits()
            },
            ResourceRangeLimits {
                budget: Duration::MAX,
                ..limits()
            },
        ] {
            assert!(
                ResourceRangeSession::open(
                    gate.clone(),
                    target.clone(),
                    key.clone(),
                    CancellationToken::default(),
                    resources(),
                    policy
                )
                .is_err()
            );
        }
        let unsafe_resource = BTreeMap::from([("../other".into(), Arc::from(&b"x"[..]))]);
        assert!(
            ResourceRangeSession::open(
                gate,
                target,
                key,
                CancellationToken::default(),
                unsafe_resource,
                limits()
            )
            .is_err()
        );
    }

    #[test]
    fn cancelled_execution_cannot_open_a_range_session() {
        let (gate, target, key) = values();
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert!(
            ResourceRangeSession::open(gate, target, key, cancel, resources(), limits()).is_err()
        );
    }
}
