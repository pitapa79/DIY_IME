use super::ResourcePolicy;
use crate::node::CoreError;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUsage {
    pub active_runs: u64,
    pub physical_jobs: u64,
    pub observers: u64,
    pub queued_jobs: u64,
    pub queued_input_bytes: u64,
    pub retained_runs: u64,
    pub retained_bytes: u64,
    pub reserved_result_bytes: u64,
    pub dedupe_entries: u64,
    pub tombstones: u64,
}
macro_rules! calculate {
    ($left:expr,$right:expr,$method:ident) => {{
        let a = $left;
        let b = $right;
        Some(ResourceUsage {
            active_runs: a.active_runs.$method(b.active_runs)?,
            physical_jobs: a.physical_jobs.$method(b.physical_jobs)?,
            observers: a.observers.$method(b.observers)?,
            queued_jobs: a.queued_jobs.$method(b.queued_jobs)?,
            queued_input_bytes: a.queued_input_bytes.$method(b.queued_input_bytes)?,
            retained_runs: a.retained_runs.$method(b.retained_runs)?,
            retained_bytes: a.retained_bytes.$method(b.retained_bytes)?,
            reserved_result_bytes: a.reserved_result_bytes.$method(b.reserved_result_bytes)?,
            dedupe_entries: a.dedupe_entries.$method(b.dedupe_entries)?,
            tombstones: a.tombstones.$method(b.tombstones)?,
        })
    }};
}
impl ResourceUsage {
    pub(crate) fn add(self, other: Self) -> Option<Self> {
        calculate!(self, other, checked_add)
    }
    fn subtract(self, other: Self) -> Option<Self> {
        calculate!(self, other, checked_sub)
    }
    fn fits(self, p: &ResourcePolicy) -> bool {
        self.active_runs <= p.max_active_runs
            && self.physical_jobs <= p.max_physical_jobs
            && self.observers <= p.max_observers
            && self.queued_jobs <= p.max_queued_jobs
            && self.queued_input_bytes <= p.max_queued_input_bytes
            && self.retained_runs <= p.max_retained_runs
            && self.dedupe_entries <= p.max_dedupe_entries
            && self.tombstones <= p.max_tombstones
            && self
                .retained_bytes
                .checked_add(self.reserved_result_bytes)
                .is_some_and(|bytes| bytes <= p.max_retained_bytes)
    }
}
#[derive(Debug)]
struct HostState {
    policy: ResourcePolicy,
    domain: String,
    usage: Mutex<ResourceUsage>,
    dispatcher: Mutex<Weak<()>>,
}
static DOMAINS: OnceLock<Mutex<BTreeMap<String, Weak<HostState>>>> = OnceLock::new();
#[derive(Debug, Clone)]
pub struct ExecutionHost {
    pub(crate) diagnostics: Arc<super::trace::TraceRegistry>,
    inner: Arc<HostState>,
    pub(crate) retention: Arc<Mutex<super::ledger::RetentionStore>>,
    pub(crate) service: Arc<Mutex<Weak<super::service::Shared>>>,
    pub(crate) service_clock: Arc<Mutex<Option<super::ExecutionClock>>>,
}
impl ExecutionHost {
    pub fn new(domain: impl Into<String>, policy: ResourcePolicy) -> Result<Self, CoreError> {
        policy.validate()?;
        let domain = domain.into();
        if domain.trim().is_empty() {
            return Err(CoreError::new(
                "invalid-resource-policy",
                "resource domain must be nonempty",
            ));
        }
        let mut domains = DOMAINS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        domains.retain(|_, owner| owner.strong_count() > 0);
        if domains.get(&domain).and_then(Weak::upgrade).is_some() {
            return Err(CoreError::new(
                "invalid-resource-policy",
                "resource domain is still owned; clone its ExecutionHost instead of resetting it",
            ));
        }
        let inner = Arc::new(HostState {
            policy,
            domain: domain.clone(),
            usage: Mutex::new(ResourceUsage::default()),
            dispatcher: Mutex::new(Weak::new()),
        });
        domains.insert(domain, Arc::downgrade(&inner));
        Ok(Self {
            inner,
            diagnostics: super::trace::TraceRegistry::new()?,
            retention: Arc::new(Mutex::new(super::ledger::RetentionStore::default())),
            service: Arc::new(Mutex::new(Weak::new())),
            service_clock: Arc::new(Mutex::new(None)),
        })
    }
    pub fn policy(&self) -> &ResourcePolicy {
        &self.inner.policy
    }
    pub(crate) fn same_host(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
    pub(crate) fn claim_dispatcher(&self) -> Result<Arc<()>, CoreError> {
        let mut owner = self
            .inner
            .dispatcher
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if owner.upgrade().is_some() {
            return Err(CoreError::new(
                "scheduler-exists",
                "this resource domain already owns a dispatch queue",
            ));
        }
        let token = Arc::new(());
        *owner = Arc::downgrade(&token);
        Ok(token)
    }
    pub(crate) fn check_lease(
        &self,
        lease: &ResourceLease,
        usage: ResourceUsage,
    ) -> Result<(), CoreError> {
        if !Arc::ptr_eq(&self.inner, &lease.inner.host)
            || lease.usage() != usage
            || Arc::strong_count(&lease.inner) != 1
        {
            return Err(CoreError::new(
                "invalid-reservation",
                "expected an exclusive reservation from this host",
            ));
        }
        Ok(())
    }
    pub fn resource_domain(&self) -> &str {
        &self.inner.domain
    }
    pub fn usage(&self) -> ResourceUsage {
        *self.inner.usage.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// All counters are reserved atomically; failure changes none of them.
    pub fn try_reserve(&self, demand: ResourceUsage) -> Result<ResourceLease, CoreError> {
        let mut used = self.inner.usage.lock().unwrap_or_else(|p| p.into_inner());
        let next = used
            .add(demand)
            .filter(|next| next.fits(&self.inner.policy))
            .ok_or_else(|| CoreError::new("busy", "execution resource budget is full"))?;
        *used = next;
        Ok(ResourceLease {
            inner: Arc::new(LeaseState {
                host: Arc::clone(&self.inner),
                usage: demand,
            }),
        })
    }
    /// Atomically exchange exclusively-owned leases; failed admission evicts nothing.
    pub fn try_reserve_replacing(
        &self,
        releases: &mut [&mut ResourceLease],
        demand: ResourceUsage,
    ) -> Result<ResourceLease, CoreError> {
        self.try_reserve_replacing_checked(releases, demand, || Ok(()))
            .map(|(lease, ())| lease)
    }
    /// Internal preflight runs under the usage lock, after fit validation and
    /// before any lease/counter changes. It must never run Node/user callbacks.
    pub(crate) fn try_reserve_replacing_checked<T>(
        &self,
        releases: &mut [&mut ResourceLease],
        demand: ResourceUsage,
        check: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<(ResourceLease, T), CoreError> {
        let mut released = ResourceUsage::default();
        for lease in releases.iter() {
            if !Arc::ptr_eq(&lease.inner.host, &self.inner) || Arc::strong_count(&lease.inner) != 1
            {
                return Err(CoreError::new(
                    "shared-lease",
                    "exchange requires exclusive leases from this host",
                ));
            }
            released = released
                .add(lease.inner.usage)
                .ok_or_else(|| CoreError::new("invalid-reservation", "exchange overflow"))?;
        }
        let mut used = self.inner.usage.lock().unwrap_or_else(|p| p.into_inner());
        let next = used
            .subtract(released)
            .and_then(|left| left.add(demand))
            .filter(|next| next.fits(&self.inner.policy))
            .ok_or_else(|| CoreError::new("busy", "exchange exceeds resource budget"))?;
        let checked = check()?;
        let acquired = ResourceLease {
            inner: Arc::new(LeaseState {
                host: Arc::clone(&self.inner),
                usage: demand,
            }),
        };
        for lease in releases.iter_mut() {
            Arc::get_mut(&mut lease.inner)
                .expect("exclusive lease checked")
                .usage = ResourceUsage::default();
        }
        *used = next;
        Ok((acquired, checked))
    }
}
#[derive(Debug)]
struct LeaseState {
    host: Arc<HostState>,
    usage: ResourceUsage,
}
impl Drop for LeaseState {
    fn drop(&mut self) {
        let mut used = self.host.usage.lock().unwrap_or_else(|p| p.into_inner());
        *used = used
            .subtract(self.usage)
            .expect("owned resource lease must balance");
    }
}
/// Cloning shares ownership; cancellation or dropping one owner cannot reclaim it.
#[derive(Debug, Clone)]
pub struct ResourceLease {
    inner: Arc<LeaseState>,
}
impl ResourceLease {
    pub fn usage(&self) -> ResourceUsage {
        self.inner.usage
    }
    pub fn resource_domain(&self) -> &str {
        &self.inner.host.domain
    }
    pub fn split(&mut self, part: ResourceUsage) -> Result<Self, CoreError> {
        let state = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| CoreError::new("shared-lease", "cannot repartition a shared lease"))?;
        let remaining = state.usage.subtract(part).ok_or_else(|| {
            CoreError::new("invalid-reservation", "split exceeds owned reservation")
        })?;
        let child = Self {
            inner: Arc::new(LeaseState {
                host: Arc::clone(&state.host),
                usage: part,
            }),
        };
        state.usage = remaining;
        Ok(child)
    }
    /// Transfer reserved result bytes to retained bytes without a release/reacquire gap.
    pub fn replace(&mut self, next: ResourceUsage) -> Result<(), CoreError> {
        self.replace_checked(next, || Ok(()))
    }
    pub(crate) fn replace_checked<T>(
        &mut self,
        next: ResourceUsage,
        check: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let state = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| CoreError::new("shared-lease", "cannot change a shared lease"))?;
        let mut used = state.host.usage.lock().unwrap_or_else(|p| p.into_inner());
        let total = used
            .subtract(state.usage)
            .and_then(|remaining| remaining.add(next))
            .filter(|total| total.fits(&state.host.policy))
            .ok_or_else(|| CoreError::new("busy", "replacement exceeds resource budget"))?;
        let checked = check()?;
        *used = total;
        state.usage = next;
        Ok(checked)
    }
}
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}
impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod clock_reservation_tests {
    use super::*;
    fn host(name: &str) -> ExecutionHost {
        let mut policy = super::super::ResourcePolicy::from_value(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        policy.max_physical_jobs = 1;
        ExecutionHost::new(name, policy).unwrap()
    }
    #[test]
    fn a_failed_reservation_clock_check_preserves_every_lease_and_counter() {
        let host = host("clock-check-exchange");
        let mut old = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..Default::default()
            })
            .unwrap();
        let before = host.usage();
        let next = ResourceUsage {
            observers: 1,
            ..Default::default()
        };
        let error = host
            .try_reserve_replacing_checked(&mut [&mut old], next, || {
                Err::<(), _>(CoreError::new("clock-retry", "due timer"))
            })
            .unwrap_err();
        assert_eq!(error.code, "clock-retry");
        assert_eq!(host.usage(), before);
        assert_eq!(old.usage(), before);
        let (acquired, at) = host
            .try_reserve_replacing_checked(&mut [&mut old], next, || Ok(123))
            .unwrap();
        assert_eq!(at, 123);
        assert_eq!(old.usage(), ResourceUsage::default());
        assert_eq!(host.usage(), next);
        drop(acquired);
        assert_eq!(host.usage(), ResourceUsage::default());
    }
    #[test]
    fn a_nonfitting_exchange_does_not_sample_or_consume_the_clock_check() {
        let host = host("clock-check-fit");
        let _full = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..Default::default()
            })
            .unwrap();
        let called = AtomicBool::new(false);
        assert_eq!(
            host.try_reserve_replacing_checked(
                &mut [],
                ResourceUsage {
                    physical_jobs: 1,
                    ..Default::default()
                },
                || {
                    called.store(true, Ordering::Relaxed);
                    Ok(())
                }
            )
            .unwrap_err()
            .code,
            "busy"
        );
        assert!(!called.load(Ordering::Relaxed));
    }
    #[test]
    fn queued_to_running_clock_check_failure_preserves_the_existing_reservation() {
        let host = host("clock-check-lease");
        let mut lease = host
            .try_reserve(ResourceUsage {
                queued_jobs: 1,
                queued_input_bytes: 42,
                ..Default::default()
            })
            .unwrap();
        let before = host.usage();
        assert_eq!(
            lease
                .replace_checked(
                    ResourceUsage {
                        physical_jobs: 1,
                        ..Default::default()
                    },
                    || Err::<(), _>(CoreError::new("clock-retry", "queue expired"))
                )
                .unwrap_err()
                .code,
            "clock-retry"
        );
        assert_eq!(host.usage(), before);
        assert_eq!(lease.usage(), before);
    }
}
