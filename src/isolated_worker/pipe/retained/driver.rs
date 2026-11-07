//! One input at a time, bounded stage exchange. Caller still proves official
//! image/graph eligibility and publishes through the Core completion permit.
use super::*;
use crate::{
    execution_control::RunKey,
    isolated_worker::{adapter::retained_reply, resource_transport},
};
use serde_json::Value;
use std::{
    io::{Read, Write},
    sync::mpsc,
    thread,
};

#[derive(Clone, Debug)]
pub struct DriverPolicy {
    pub idle_timeout: Duration,
    pub input_timeout: Duration,
    pub max_calls: u64,
    pub max_input_bytes: u64,
    pub max_reply_bytes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverStop {
    Closed,
    InputCancelled,
    InputDeadline,
    Idle,
    Authority,
    Transport,
    NodeError,
}
#[derive(Clone)]
struct Active {
    key: RunKey,
    cancel: CancellationToken,
    deadline: Instant,
    call_deadline: Option<Instant>,
}
struct Shared {
    active: Mutex<Option<Active>>,
    cause: Mutex<Option<DriverStop>>,
    cancel: CancellationToken,
}
impl Shared {
    fn stop(&self, cause: DriverStop) {
        let mut first = self.cause.lock().unwrap_or_else(|p| p.into_inner());
        if first.is_none() {
            *first = Some(cause);
        }
        self.cancel.cancel();
    }
}
struct Call {
    binding: usize,
    input: Vec<u8>,
    scope: ResourceRangeSession,
    reply: mpsc::SyncSender<Result<(Value, ResourceRangeSession, usize), CoreError>>,
    _budget: ResourceLease,
}
// None closes the driver; storing the bounded call inline avoids a heap
// allocation per stage merely to send a distinct stop variant.
type Command = Option<Call>;
fn error(code: &str) -> CoreError {
    CoreError::new(code, "retained input could not complete")
}
fn check_active(shared: &Shared) -> Result<(), CoreError> {
    let active = shared.active.lock().unwrap_or_else(|p| p.into_inner());
    let active = active.as_ref().ok_or_else(|| error("retained-input"))?;
    if active.cancel.is_cancelled() {
        return Err(error("worker-cancelled"));
    }
    if Instant::now() >= active.deadline
        || active.call_deadline.is_some_and(|d| Instant::now() >= d)
    {
        return Err(error("worker-deadline"));
    }
    if shared.cancel.is_cancelled() {
        return Err(error("retained-closed"));
    }
    Ok(())
}
struct Watchdog {
    done: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Watchdog {
    fn start(shared: Arc<Shared>) -> Result<Self, String> {
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let handle = thread::Builder::new()
            .name("diyime2-retained-input-watch".into())
            .spawn(move || {
                while !worker_done.load(Ordering::Acquire) && !shared.cancel.is_cancelled() {
                    let active = shared
                        .active
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    if let Some(active) = active {
                        if active.cancel.is_cancelled() {
                            shared.stop(DriverStop::InputCancelled);
                            break;
                        }
                        if Instant::now() >= active.deadline
                            || active.call_deadline.is_some_and(|d| Instant::now() >= d)
                        {
                            shared.stop(DriverStop::InputDeadline);
                            break;
                        }
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            })
            .map_err(|_| "retained watchdog unavailable")?;
        Ok(Self {
            done,
            handle: Some(handle),
        })
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub struct RetainedClient {
    host: ExecutionHost,
    pin: RetainedReservation,
    stages: Vec<usize>,
    policy: DriverPolicy,
    send: mpsc::SyncSender<Command>,
    ready: mpsc::Receiver<Result<(), String>>,
    is_ready: bool,
    issued_calls: u64,
    shared: Arc<Shared>,
}
impl RetainedClient {
    /// Stage indexes refer to the reservation's exact grant/config/resource
    /// bindings, not guest-supplied names. No package becomes eligible here.
    pub fn channel(
        host: ExecutionHost,
        pin: RetainedReservation,
        stages: Vec<usize>,
        policy: DriverPolicy,
    ) -> Result<(Self, RetainedWorkerRequest), CoreError> {
        if host.resource_domain() != pin.inner.lease.resource_domain()
            || stages.is_empty()
            || stages.len() > 16
            || policy.idle_timeout.is_zero()
            || policy.idle_timeout > Duration::from_secs(60)
            || policy.input_timeout.is_zero()
            || policy.input_timeout > Duration::from_secs(30)
            || policy.max_calls < stages.len() as u64
            || policy.max_calls > 4096
            || policy.max_input_bytes == 0
            || policy.max_input_bytes > host.policy().max_queued_input_bytes
            || policy.max_input_bytes > resource_transport::MAX_INPUT as u64
            || policy.max_reply_bytes == 0
            || policy.max_reply_bytes > host.policy().max_result_bytes_per_run
            || policy.max_reply_bytes > resource_transport::MAX_INPUT as u64
        {
            return Err(error("retained-policy"));
        }
        pin.validate()?;
        {
            let bindings = pin.inner.bindings.lock().unwrap_or_else(|p| p.into_inner());
            if stages.iter().any(|index| *index >= bindings.len()) {
                return Err(error("retained-policy"));
            }
        }
        let (send, receive) = mpsc::sync_channel::<Command>(1);
        let (ready_send, ready) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            active: Mutex::new(None),
            cause: Mutex::new(None),
            cancel: pin.inner.cancel.clone(),
        });
        let driver_pin = pin.clone();
        let driver_shared = shared.clone();
        let driver_policy = policy.clone();
        let driver_stages = stages.clone();
        let exchange = Box::new(move |mut output: std::fs::File, mut input: std::fs::File| {
            let result = (|| -> Result<Vec<u8>, String> {
                let _watchdog = Watchdog::start(driver_shared.clone())?;
                let mut ready = [0; 12];
                output
                    .read_exact(&mut ready)
                    .map_err(|_| "retained readiness unavailable")?;
                let modules = u32::from_le_bytes(ready[8..].try_into().unwrap());
                if &ready[..8] != b"DIYRDY01" || modules == 0 || modules > 4 {
                    return Err("invalid retained readiness".into());
                }
                {
                    let bindings = driver_pin
                        .inner
                        .bindings
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if driver_stages
                        .iter()
                        .any(|index| bindings[*index].module_slot >= modules)
                    {
                        return Err("retained binding module absent".into());
                    }
                }
                driver_pin.validate().map_err(|e| e.code)?;
                ready_send
                    .send(Ok(()))
                    .map_err(|_| "retained client abandoned readiness")?;
                let mut last = Instant::now();
                let mut sequence = 0u64;
                loop {
                    if driver_pin.is_closed() {
                        return Err("retained owner closed".into());
                    }
                    let command = match receive.recv_timeout(Duration::from_millis(5)) {
                        Ok(command) => command,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            driver_shared.stop(DriverStop::Closed);
                            return Ok(Vec::new());
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if driver_shared
                                .active
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .is_none()
                                && last.elapsed() >= driver_policy.idle_timeout
                            {
                                driver_shared.stop(DriverStop::Idle);
                                return Ok(Vec::new());
                            }
                            continue;
                        }
                    };
                    let Some(mut call) = command else {
                        return Ok(Vec::new());
                    };
                    let reply = (|| -> Result<(Value, usize), CoreError> {
                        check_active(&driver_shared)?;
                        if driver_shared
                            .active
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .as_ref()
                            .is_none_or(|active| &active.key != call.scope.run_key())
                        {
                            return Err(error("retained-scope"));
                        }
                        if sequence >= driver_policy.max_calls {
                            return Err(error("retained-call-limit"));
                        }
                        let slot = driver_pin.authorize_binding(call.binding, &mut call.scope)?;
                        sequence += 1;
                        let mut header = b"DIYRET01".to_vec();
                        header.extend(sequence.to_le_bytes());
                        header.extend(slot.to_le_bytes());
                        input
                            .write_all(&header)
                            .and_then(|()| input.flush())
                            .map_err(|_| error("retained-transport"))?;
                        let mut ack = [0; 20];
                        output
                            .read_exact(&mut ack)
                            .map_err(|_| error("retained-transport"))?;
                        if ack.as_slice() != header {
                            return Err(error("retained-transport"));
                        }
                        check_active(&driver_shared)?;
                        let bytes = resource_transport::serve_retained(
                            &mut output,
                            &mut input,
                            &call.input,
                            &mut call.scope,
                            driver_policy.max_reply_bytes as usize,
                        )
                        .map_err(|_| error("retained-transport"))?;
                        check_active(&driver_shared)?;
                        driver_pin.authorize_binding(call.binding, &mut call.scope)?;
                        Ok((retained_reply(&bytes)??, bytes.len()))
                    })();
                    match reply {
                        Ok((data, size)) => {
                            call.reply
                                .send(Ok((data, call.scope, size)))
                                .map_err(|_| "retained reply abandoned")?;
                        }
                        Err(failure) => {
                            driver_shared.stop(match failure.code.as_str() {
                                "worker-cancelled" => DriverStop::InputCancelled,
                                "worker-deadline" => DriverStop::InputDeadline,
                                "retained-scope" | "worker-authority" => DriverStop::Authority,
                                "retained-transport" | "worker-reply" => DriverStop::Transport,
                                _ => DriverStop::NodeError,
                            });
                            let _ = call.reply.send(Err(failure));
                            return Err("retained call failed".into());
                        }
                    }
                    last = Instant::now();
                }
            })();
            if result.is_err() {
                driver_shared.stop(DriverStop::Transport);
                let _ = ready_send.try_send(Err("retained connection failed".into()));
            }
            result
        });
        Ok((
            Self {
                host,
                pin: pin.clone(),
                stages,
                policy,
                send,
                ready,
                is_ready: false,
                issued_calls: 0,
                shared,
            },
            RetainedWorkerRequest {
                reservation: pin,
                exchange,
            },
        ))
    }
    /// Blocking execution-thread operation. The startup cost is recorded apart
    /// from ready-input latency, never silently counted as an already-ready input.
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<(), CoreError> {
        if self.is_ready {
            return self.pin.validate();
        }
        if timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(error("retained-policy"));
        }
        match self.ready.recv_timeout(timeout) {
            Ok(Ok(())) => {
                self.pin.validate()?;
                self.is_ready = true;
                Ok(())
            }
            _ => {
                self.stop(DriverStop::Transport);
                Err(error("retained-not-ready"))
            }
        }
    }
    pub fn stop(&self, cause: DriverStop) {
        self.shared.stop(cause);
        let _ = self.send.try_send(None);
    }
    pub fn stop_cause(&self) -> Option<DriverStop> {
        *self.shared.cause.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn begin_input(
        &mut self,
        key: RunKey,
        cancel: CancellationToken,
    ) -> Result<RetainedInput<'_>, CoreError> {
        if !self.is_ready {
            return Err(error("retained-not-ready"));
        }
        self.pin.validate()?;
        if cancel.same_source(&self.pin.inner.cancel) {
            self.stop(DriverStop::Authority);
            return Err(error("retained-scope"));
        }
        if self
            .issued_calls
            .checked_add(self.stages.len() as u64)
            .is_none_or(|calls| calls > self.policy.max_calls)
        {
            self.stop(DriverStop::Closed);
            return Err(error("retained-call-limit"));
        }
        {
            let bindings = self
                .pin
                .inner
                .bindings
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let context = bindings[0].scope.run_key();
            if key.session_epoch != context.session_epoch
                || key.plan_version != context.plan_version
            {
                drop(bindings);
                self.stop(DriverStop::Authority);
                return Err(error("retained-scope"));
            }
        }
        if cancel.is_cancelled() {
            self.stop(DriverStop::InputCancelled);
            return Err(error("worker-cancelled"));
        }
        let deadline = Instant::now()
            .checked_add(self.policy.input_timeout)
            .ok_or_else(|| error("retained-policy"))?
            .min(self.pin.inner.deadline);
        let budget = self.host.try_reserve(ResourceUsage {
            queued_jobs: 1,
            queued_input_bytes: self.policy.max_input_bytes,
            reserved_result_bytes: self
                .policy
                .max_reply_bytes
                .checked_mul(2)
                .ok_or_else(|| error("retained-policy"))?,
            ..ResourceUsage::default()
        })?;
        let mut active = self.shared.active.lock().unwrap_or_else(|p| p.into_inner());
        if active.is_some() {
            return Err(error("retained-busy"));
        }
        *active = Some(Active {
            key: key.clone(),
            cancel: cancel.clone(),
            deadline,
            call_deadline: None,
        });
        drop(active);
        Ok(RetainedInput {
            client: self,
            key,
            cancel,
            deadline,
            budget,
            completed: 0,
            input_bytes: 0,
            reply_bytes: 0,
            last_scope: None,
            finished: false,
        })
    }
}
impl Drop for RetainedClient {
    fn drop(&mut self) {
        self.stop(DriverStop::Closed);
    }
}

pub struct RetainedInput<'a> {
    client: &'a mut RetainedClient,
    key: RunKey,
    cancel: CancellationToken,
    deadline: Instant,
    budget: ResourceLease,
    completed: usize,
    input_bytes: u64,
    reply_bytes: u64,
    last_scope: Option<ResourceRangeSession>,
    finished: bool,
}
impl RetainedInput<'_> {
    pub fn call(
        &mut self,
        input: Vec<u8>,
        mut scope: ResourceRangeSession,
    ) -> Result<Value, CoreError> {
        let result = (|| {
            check_active(&self.client.shared)?;
            if input.is_empty() {
                return Err(error("retained-input-limit"));
            }
            let binding = *self
                .client
                .stages
                .get(self.completed)
                .ok_or_else(|| error("retained-stage-order"))?;
            if scope.run_key() != &self.key || !scope.cancellation_matches(&self.cancel) {
                return Err(error("retained-scope"));
            }
            self.client.pin.authorize_binding(binding, &mut scope)?;
            self.input_bytes = self
                .input_bytes
                .checked_add(input.len() as u64)
                .filter(|v| *v <= self.client.policy.max_input_bytes)
                .ok_or_else(|| error("retained-input-limit"))?;
            let call_deadline = scope.absolute_deadline().min(self.deadline);
            if let Some(active) = self
                .client
                .shared
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
            {
                active.call_deadline = Some(call_deadline);
            }
            let (reply, receive) = mpsc::sync_channel(1);
            self.client
                .send
                .try_send(Some(Call {
                    binding,
                    input,
                    scope,
                    reply,
                    _budget: self.budget.clone(),
                }))
                .map_err(|_| error("retained-busy"))?;
            let (data, scope, size) = receive
                .recv_timeout(call_deadline.saturating_duration_since(Instant::now()))
                .map_err(|failure| {
                    error(match failure {
                        mpsc::RecvTimeoutError::Timeout => "worker-deadline",
                        mpsc::RecvTimeoutError::Disconnected => "retained-transport",
                    })
                })??;
            check_active(&self.client.shared)?;
            self.reply_bytes = self
                .reply_bytes
                .checked_add(size as u64)
                .filter(|v| *v <= self.client.policy.max_reply_bytes)
                .ok_or_else(|| error("retained-reply-limit"))?;
            self.completed += 1;
            self.client.issued_calls += 1;
            self.last_scope = Some(scope);
            if let Some(active) = self
                .client
                .shared
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
            {
                active.call_deadline = None;
            }
            Ok(data)
        })();
        if let Err(failure) = &result {
            self.client.stop(if self.cancel.is_cancelled() {
                DriverStop::InputCancelled
            } else if failure.code == "worker-deadline" {
                DriverStop::InputDeadline
            } else if failure.code == "retained-scope" {
                DriverStop::Authority
            } else {
                DriverStop::Transport
            });
        }
        result
    }
    /// Caller validates Node port/Packet contracts and uses the current Core
    /// completion permit inside this publication; this is not direct UI access.
    pub fn finish<T>(
        mut self,
        publish: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        if self.completed != self.client.stages.len() {
            return Err(error("retained-stage-order"));
        }
        check_active(&self.client.shared)?;
        let mut scope = self
            .last_scope
            .take()
            .ok_or_else(|| error("retained-input"))?;
        let result = self.client.pin.publish(&mut scope, || {
            check_active(&self.client.shared)?;
            publish()
        });
        if result.is_ok() {
            self.finished = true;
            *self
                .client
                .shared
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
        }
        result
    }
}
impl Drop for RetainedInput<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.client.stop(DriverStop::InputCancelled);
        }
        *self
            .client
            .shared
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
    }
}
