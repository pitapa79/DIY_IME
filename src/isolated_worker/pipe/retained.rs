//! Trusted-native lifetime pin. Never a package opt-in or official-image proof.
use super::super::adapter::ResourceRangeSession;
use super::*;
use crate::{
    execution_control::{ExecutionHost, ResourceLease, ResourceUsage},
    node::CoreError,
};
use std::sync::Mutex;
mod driver;
pub use driver::{DriverPolicy, DriverStop, RetainedClient, RetainedInput};

pub struct RetainedBinding {
    pub module_slot: u32,
    pub scope: ResourceRangeSession,
}
struct State {
    bindings: Mutex<Vec<RetainedBinding>>,
    lease: ResourceLease,
    cancel: CancellationToken,
    deadline: Instant,
    launched: AtomicBool,
    ended: Mutex<bool>,
}
/// Cloning retains the same charged reservation, not a second budget domain.
/// Running owns an independent clone through original-process ending and IO join.
#[derive(Clone)]
pub struct RetainedReservation {
    inner: Arc<State>,
}
fn invalid() -> CoreError {
    CoreError::new("retained-scope", "invalid or expired retained Worker scope")
}
impl RetainedReservation {
    /// Caller supplies only fully prepared resources and grants for a verified
    /// fixed image/plan/context. This low-level API does not decide eligibility.
    pub fn reserve(
        host: &ExecutionHost,
        mut bindings: Vec<RetainedBinding>,
        cancel: &CancellationToken,
    ) -> Result<Self, CoreError> {
        if bindings.is_empty() || bindings.len() > 16 || cancel.is_cancelled() {
            return Err(invalid());
        }
        let mut bytes = 0u64;
        if bindings
            .iter()
            .any(|binding| !binding.scope.same_execution_context(&bindings[0].scope))
        {
            return Err(invalid());
        }
        let mut deadline = None;
        for binding in &mut bindings {
            if binding.module_slot >= 4 || !binding.scope.cancellation_matches(cancel) {
                return Err(invalid());
            }
            binding.scope.check_current()?;
            bytes = bytes
                .checked_add(binding.scope.snapshot_bytes()?)
                .ok_or_else(invalid)?;
            deadline = Some(
                deadline.map_or(binding.scope.absolute_deadline(), |value: Instant| {
                    value.min(binding.scope.absolute_deadline())
                }),
            );
        }
        let lease = host.try_reserve(ResourceUsage {
            physical_jobs: 1,
            retained_bytes: bytes,
            ..ResourceUsage::default()
        })?;
        Ok(Self {
            inner: Arc::new(State {
                bindings: Mutex::new(bindings),
                lease,
                cancel: cancel.clone(),
                deadline: deadline.unwrap(),
                launched: AtomicBool::new(false),
                ended: Mutex::new(false),
            }),
        })
    }
    pub fn usage(&self) -> ResourceUsage {
        self.inner.lease.usage()
    }
    /// Control-plane closure only, never an OS exit receipt or a quota refund.
    pub fn is_closed(&self) -> bool {
        *self.inner.ended.lock().unwrap_or_else(|p| p.into_inner())
            || self.inner.cancel.is_cancelled()
            || Instant::now() >= self.inner.deadline
    }
    /// A repeated numeric ID or equal bytes cannot substitute another grant,
    /// plan, node, module slot or immutable resource allocation.
    pub fn authorize(&self, slot: u32, scope: &mut ResourceRangeSession) -> Result<(), CoreError> {
        let result = (|| {
            self.validate()?;
            scope.check_current()?;
            let bindings = self
                .inner
                .bindings
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if !bindings.iter().any(|binding| {
                binding.module_slot == slot && binding.scope.same_snapshot_scope(scope)
            }) {
                return Err(invalid());
            }
            Ok(())
        })();
        if result.is_err() {
            scope.withdraw();
            self.inner.cancel.cancel();
        }
        result
    }
    fn validate(&self) -> Result<(), CoreError> {
        if self.is_closed() {
            return Err(invalid());
        }
        for binding in self
            .inner
            .bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter_mut()
        {
            binding.scope.check_current()?;
        }
        Ok(())
    }
    fn authorize_binding(
        &self,
        index: usize,
        scope: &mut ResourceRangeSession,
    ) -> Result<u32, CoreError> {
        self.validate()?;
        scope.check_current()?;
        let bindings = self
            .inner
            .bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let binding = bindings.get(index).ok_or_else(invalid)?;
        if !binding.scope.same_snapshot_scope(scope) {
            return Err(invalid());
        }
        Ok(binding.module_slot)
    }
    fn publish<T>(
        &self,
        scope: &mut ResourceRangeSession,
        action: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        self.validate()?;
        scope.publish_retained(|| {
            let ended = self.inner.ended.lock().unwrap_or_else(|p| p.into_inner());
            if *ended || self.inner.cancel.is_cancelled() || Instant::now() >= self.inner.deadline {
                return Err(invalid());
            }
            action()
        })
    }
}
struct ProcessPin(RetainedReservation, Option<Arc<AtomicU64>>);
impl ResourceOwner for ProcessPin {
    fn observe_start(&self, process_id: u32) {
        if let Some(id) = &self.1 {
            id.store(process_id as u64, Ordering::Release);
        }
    }
    fn observe_exit(&self) {
        *self.0.inner.ended.lock().unwrap_or_else(|p| p.into_inner()) = true;
    }
}
impl Drop for ProcessPin {
    fn drop(&mut self) {
        self.observe_exit();
    }
}
pub struct RetainedWorkerRequest {
    pub reservation: RetainedReservation,
    /// Trusted Host execution-thread driver: bounded commands, per-call scope
    /// checks and cancellation/deadline forwarding; never execute on the UI thread.
    pub exchange: Box<dyn FnOnce(std::fs::File, std::fs::File) -> Result<Vec<u8>, String> + Send>,
}
/// Runs the physical lifetime; logical replies may be delivered by the driver
/// before this returns. Caller retains its own pin through ACL/asset cleanup.
/// Unconfirmed always owns the same original process, IO and reservation clone.
pub fn execute_retained<'a>(
    profile: &'a Profile,
    job: &'a Job,
    cache: &'a Cache,
    program: &Path,
    args: &[OsString],
    request: RetainedWorkerRequest,
    control: &RunControl,
) -> Result<Outcome<'a>, String> {
    execute_retained_observed(profile, job, cache, program, args, request, control, None)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::isolated_worker) fn execute_retained_observed<'a>(
    profile: &'a Profile,
    job: &'a Job,
    cache: &'a Cache,
    program: &Path,
    args: &[OsString],
    request: RetainedWorkerRequest,
    control: &RunControl,
    observation: Option<RetainedObservation>,
) -> Result<Outcome<'a>, String> {
    if args
        .first()
        .is_none_or(|arg| arg != "--retained-resources-v1")
        || control
            .walltime
            .is_none_or(|value| value.is_zero() || value > Duration::from_secs(600))
        || control.termination_grace > Duration::from_secs(5)
        || !request
            .reservation
            .inner
            .cancel
            .same_source(&control.cancel)
    {
        return Err("invalid retained native invocation".into());
    }
    request
        .reservation
        .validate()
        .map_err(|_| "retained authority unavailable")?;
    if request
        .reservation
        .inner
        .launched
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("retained reservation already launched".into());
    }
    let cancel = control.cancel.clone();
    let exchange = request.exchange;
    let wrapped = Box::new(move |output, input| {
        let result = exchange(output, input);
        if result.is_err() {
            cancel.cancel();
        }
        result
    });
    let deadline = request.reservation.inner.deadline;
    let (activity, process_id) = observation.map_or((None, None), |value| {
        (Some(value.activity), Some(value.process_id))
    });
    execute_observed(
        WorkerScope {
            profile,
            job,
            cache,
            activity,
            resource_deadline: Some(deadline),
            exchange: Some(wrapped),
            resource_owner: Some(Box::new(ProcessPin(request.reservation, process_id))),
            #[cfg(test)]
            prepared: false,
        },
        program,
        args,
        Vec::new(),
        control,
    )
}
