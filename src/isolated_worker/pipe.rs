//! Host-owned stdin/stdout transport. Never run its blocking owner on a UI thread.
use super::*;
use crate::execution_control::CancellationToken;
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const WIRE_LIMIT: usize = 8 * 1024 * 1024;
#[cfg(target_arch = "x86_64")]
mod preparation;
#[cfg(target_arch = "x86_64")]
pub use preparation::{
    PreparationExit, PreparationOutcome, PreparationRunning, PreparationWorkerRequest,
    execute_preparation,
};
#[cfg(test)]
pub(super) const PROBE_READY: &[u8] = b"\nDIY-READY-PROBE\0";
#[cfg(test)]
pub(super) const PROBE_RESULT: &[u8] = b"\nDIY-MODULE-PROBE\0";
#[cfg(test)]
#[derive(Default)]
struct ProbeClocks {
    sent: Option<Instant>,
    response: Option<Instant>,
}

fn diagnostic_pipe() -> Result<(OwnedHandle, OwnedHandle), String> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    ok(
        unsafe { CreatePipe(&mut read, &mut write, &sa, 4096) },
        "create diagnostic pipe",
    )?;
    let read = OwnedHandle::new(read, "diagnostic read")?;
    let write = OwnedHandle::new(write, "diagnostic write")?;
    ok(
        unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) },
        "protect diagnostic reader",
    )?;
    Ok((read, write))
}

#[derive(Clone)]
pub struct RunControl {
    pub cancel: CancellationToken,
    pub walltime: Option<Duration>,
    pub termination_grace: Duration,
}
impl Default for RunControl {
    fn default() -> Self {
        Self {
            cancel: CancellationToken::default(),
            walltime: None,
            termination_grace: Duration::from_secs(5),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Exited,
    Cancelled,
    Deadline,
    OutputLimit,
    StartRejected,
    WaitFailed,
}

/// Only a signalled OS process handle can create this receipt.
/// ~~~compile_fail
/// use diy_ime2_core::isolated_worker::{EndReason, ExitReceipt};
/// let forged = ExitReceipt { process_id: 1, exit_code: Some(0),
///     reason: EndReason::Exited, output: Ok(Vec::new()), input: Ok(()) };
/// ~~~
#[derive(Debug)]
pub struct ExitReceipt {
    process_id: u32,
    exit_code: Option<u32>,
    reason: EndReason,
    output: Result<Vec<u8>, String>,
    input: Result<(), String>,
    diagnostics: Option<Result<Vec<u8>, ()>>,
    #[cfg(test)]
    pub(super) phase_micros: Option<[u128; 3]>,
    #[cfg(test)]
    pub(super) prepared_micros: Option<Box<[u128; 3]>>,
    #[cfg(test)]
    pub(super) signalled_at: Instant,
}
impl ExitReceipt {
    pub(super) fn diagnostics(&self) -> Option<&Result<Vec<u8>, ()>> {
        self.diagnostics.as_ref()
    }
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    pub fn exit_code(&self) -> Option<u32> {
        self.exit_code
    }
    pub fn reason(&self) -> EndReason {
        self.reason
    }
    pub fn output(&self) -> Result<&[u8], &str> {
        self.output.as_deref().map_err(String::as_str)
    }
    pub fn input_delivered(&self) -> bool {
        self.input.is_ok()
    }
}
pub enum Outcome<'a> {
    CancelledBeforeStart,
    Exited(ExitReceipt),
    /// Holds the original profile/Job/cache borrows and live process handle.
    Unconfirmed(Box<Running<'a>>),
}
/// An unconfirmed process cannot become a receipt through EOF, a guest message,
/// or elapsed time. Dropping this owner blocks until actual exit: keep it on a
/// Host execution thread and retain the physical reservation until it returns.
pub struct Running<'a> {
    _profile: &'a Profile,
    job: &'a Job,
    _cache: &'a Cache,
    process: OwnedHandle,
    process_id: u32,
    output: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    input: Option<thread::JoinHandle<Result<(), String>>>,
    diagnostics: Option<thread::JoinHandle<Result<Vec<u8>, ()>>>,
    diagnostics_requested: bool,
    reason: EndReason,
    finished: bool,
    activity: Option<Arc<AtomicU64>>,
    observed: bool,
    exchange_used: bool,
    #[cfg(target_arch = "x86_64")]
    _range_session: Option<Arc<std::sync::Mutex<adapter::ResourceRangeSession>>>,
    #[cfg(test)]
    launch_started: Instant,
    #[cfg(test)]
    resumed: Option<Instant>,
    #[cfg(test)]
    delivery: Option<Arc<std::sync::Mutex<ProbeClocks>>>,
}
fn milliseconds(value: Duration) -> u32 {
    value.as_millis().min((u32::MAX - 1) as u128) as u32
}
impl<'a> Running<'a> {
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    fn signalled(&self, timeout: u32) -> bool {
        (unsafe { WaitForSingleObject(self.process.0, timeout) }) == WAIT_OBJECT_0
    }
    fn finish(&mut self) -> ExitReceipt {
        // Every caller checked the actual process handle, never child output.
        #[cfg(test)]
        let signalled = Instant::now();
        let mut code = 0;
        let exit_code = if unsafe { GetExitCodeProcess(self.process.0, &mut code) } != 0 {
            Some(code)
        } else {
            None
        };
        let output = self.output.take().map_or_else(
            || Ok(Vec::new()),
            |t| {
                t.join()
                    .map_err(|_| "Worker reader panicked".to_owned())
                    .and_then(|r| r)
            },
        );
        let input = self.input.take().map_or_else(
            || {
                if self.exchange_used && output.is_err() {
                    Err("Worker resource exchange failed".into())
                } else {
                    Ok(())
                }
            },
            |t| {
                t.join()
                    .map_err(|_| "Worker writer panicked".to_owned())
                    .and_then(|r| r)
            },
        );
        let diagnostics = self.diagnostics_requested.then(|| {
            self.diagnostics
                .take()
                .ok_or(())
                .and_then(|t| t.join().map_err(|_| ()).and_then(|r| r))
        });
        if self.observed {
            self.activity
                .as_ref()
                .expect("observed activity")
                .fetch_sub(1, Ordering::AcqRel);
            self.observed = false;
        }
        self.finished = true;
        ExitReceipt {
            process_id: self.process_id,
            exit_code,
            reason: self.reason,
            output,
            input,
            diagnostics,
            #[cfg(test)]
            signalled_at: signalled,
            #[cfg(test)]
            prepared_micros: self.delivery.as_ref().and_then(|delivery| {
                let clocks = delivery.lock().unwrap();
                let sent = clocks.sent?;
                Some(Box::new([
                    (sent - self.launch_started).as_micros(),
                    (clocks.response? - sent).as_micros(),
                    (signalled - sent).as_micros(),
                ]))
            }),
            #[cfg(test)]
            phase_micros: self.resumed.map(|resumed| {
                [
                    (resumed - self.launch_started).as_micros(),
                    (signalled - resumed).as_micros(),
                    signalled.elapsed().as_micros(),
                ]
            }),
        }
    }
    pub fn retry(mut self: Box<Self>, grace: Duration) -> Outcome<'a> {
        if self.signalled(milliseconds(grace)) {
            Outcome::Exited(self.finish())
        } else {
            Outcome::Unconfirmed(self)
        }
    }
    fn stop(mut self: Box<Self>, reason: EndReason, grace: Duration) -> Outcome<'a> {
        self.reason = reason;
        self.job.terminate();
        self.retry(grace)
    }
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.job.terminate();
        // Fail closed: no receipt or ownership release while the handle is unsignalled.
        while !self.signalled(1000) {
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.finish();
    }
}
fn verify(profile: &Profile, job: &Job, process: HANDLE) -> Result<(), String> {
    let mut in_job = 0;
    if unsafe { IsProcessInJob(process, job.0.0, &mut in_job) } == 0 || in_job == 0 {
        return Err("Worker Job ownership not confirmed".into());
    }
    let mut token = ptr::null_mut();

    ok(
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) },
        "open Worker token",
    )?;
    let token = OwnedHandle::new(token, "Worker token")?;
    let mut is_container = 0u32;
    let mut length = 0;
    ok(
        unsafe {
            GetTokenInformation(
                token.0,
                TokenIsAppContainer,
                ptr::from_mut(&mut is_container).cast(),
                size_of::<u32>() as u32,
                &mut length,
            )
        },
        "query Worker AppContainer",
    )?;
    if is_container != 1 {
        return Err("Worker token is not AppContainer".into());
    }
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenAppContainerSid, ptr::null_mut(), 0, &mut size);
    }
    if size < size_of::<TOKEN_APPCONTAINER_INFORMATION>() as u32 || size > 65536 {
        return Err("invalid Worker AppContainer SID length".into());
    }
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    ok(
        unsafe {
            GetTokenInformation(
                token.0,
                TokenAppContainerSid,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        },
        "query Worker profile SID",
    )?;
    let info = unsafe { &*buffer.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>() };
    if info.TokenAppContainer.is_null()
        || unsafe { EqualSid(info.TokenAppContainer, profile.sid) } == 0
    {
        return Err("Worker profile SID mismatch".into());
    }
    Ok(())
}
/// Validation errors occur before CreateProcess. Every post-create result is
/// either a verified exit or an owner which still holds the live process.
pub(super) struct WorkerScope<'a> {
    profile: &'a Profile,
    job: &'a Job,
    cache: &'a Cache,
    activity: Option<Arc<AtomicU64>>,
    exchange: Option<HostExchange>,
    #[cfg(target_arch = "x86_64")]
    range_session: Option<Arc<std::sync::Mutex<adapter::ResourceRangeSession>>>,
    #[cfg(test)]
    prepared: bool,
}
type HostExchange = Box<dyn FnOnce(std::fs::File, std::fs::File) -> Result<Vec<u8>, String> + Send>;
impl<'a> WorkerScope<'a> {
    pub(super) fn observed(
        profile: &'a Profile,
        job: &'a Job,
        cache: &'a Cache,
        activity: Arc<AtomicU64>,
    ) -> Self {
        Self {
            profile,
            job,
            cache,
            activity: Some(activity),
            exchange: None,
            #[cfg(target_arch = "x86_64")]
            range_session: None,
            #[cfg(test)]
            prepared: false,
        }
    }
    #[cfg(test)]
    pub(super) fn prepared(profile: &'a Profile, job: &'a Job, cache: &'a Cache) -> Self {
        Self {
            profile,
            job,
            cache,
            activity: None,
            exchange: None,
            #[cfg(target_arch = "x86_64")]
            range_session: None,
            prepared: true,
        }
    }
    #[cfg(test)]
    pub(super) fn observe_start(mut self, activity: Arc<AtomicU64>) -> Self {
        self.activity = Some(activity);
        self
    }
    #[cfg(test)]
    pub(super) fn with_test_exchange(mut self, exchange: HostExchange) -> Self {
        self.exchange = Some(exchange);
        self
    }
}
pub fn execute<'a>(
    profile: &'a Profile,
    job: &'a Job,
    cache: &'a Cache,
    program: &Path,
    args: &[OsString],
    request: Vec<u8>,
    control: &RunControl,
) -> Result<Outcome<'a>, String> {
    execute_observed(
        WorkerScope {
            profile,
            job,
            cache,
            activity: None,
            exchange: None,
            #[cfg(target_arch = "x86_64")]
            range_session: None,
            #[cfg(test)]
            prepared: false,
        },
        program,
        args,
        request,
        control,
    )
}
/// Trusted Host composition binds prepared resources to this exact invocation.
#[cfg(target_arch = "x86_64")]
pub struct ResourceWorkerRequest {
    pub input: Vec<u8>,
    pub resources: adapter::ResourceRangeSession,
}

#[cfg(target_arch = "x86_64")]
pub fn execute_resources<'a>(
    profile: &'a Profile,
    job: &'a Job,
    cache: &'a Cache,
    program: &Path,
    args: &[OsString],
    mut request: ResourceWorkerRequest,
    control: &RunControl,
) -> Result<Outcome<'a>, String> {
    if request.input.len() > super::resource_transport::MAX_INPUT
        || !request.resources.cancellation_matches(&control.cancel)
    {
        request.resources.withdraw();
        return Err("invalid resource invocation owner or input bound".into());
    }
    request
        .resources
        .check_current()
        .map_err(|_| "resource invocation unavailable")?;
    // Blocking pipe IO is always bounded by the same absolute resource deadline.
    let remaining = request.resources.remaining_budget();
    let mut control = control.clone();
    control.walltime = Some(
        control
            .walltime
            .map_or(remaining, |limit| limit.min(remaining)),
    );
    let session = Arc::new(std::sync::Mutex::new(request.resources));
    let io_session = session.clone();
    let exchange = Box::new(move |output, input| {
        let mut session = io_session
            .lock()
            .map_err(|_| "resource owner unavailable")?;
        let result = super::resource_transport::serve(output, input, &request.input, &mut session);
        if result.is_err() {
            session.withdraw();
        }
        result.map_err(|_| "Worker resource exchange failed".into())
    });
    execute_observed(
        WorkerScope {
            profile,
            job,
            cache,
            activity: None,
            exchange: Some(exchange),
            range_session: Some(session),
            #[cfg(test)]
            prepared: false,
        },
        program,
        args,
        Vec::new(),
        &control,
    )
}
pub(super) fn execute_observed<'a>(
    scope: WorkerScope<'a>,
    program: &Path,
    args: &[OsString],
    request: Vec<u8>,
    control: &RunControl,
) -> Result<Outcome<'a>, String> {
    execute_with_diagnostics(scope, program, args, request, control, false)
}
pub(super) fn execute_with_diagnostics<'a>(
    scope: WorkerScope<'a>,
    program: &Path,
    args: &[OsString],
    request: Vec<u8>,
    control: &RunControl,
    detailed: bool,
) -> Result<Outcome<'a>, String> {
    #[cfg(test)]
    let launch_started = Instant::now();
    let WorkerScope {
        profile,
        job,
        cache,
        activity,
        exchange,
        #[cfg(target_arch = "x86_64")]
        range_session,
        #[cfg(test)]
        prepared,
    } = scope;
    if control.cancel.is_cancelled() {
        return Ok(Outcome::CancelledBeforeStart);
    }
    if request.len() > WIRE_LIMIT {
        return Err("Worker request exceeds wire limit".into());
    }
    if args.iter().any(|a| a.encode_wide().any(|c| c == 0)) {
        return Err("Worker argument contains NUL".into());
    }
    let program = std::fs::canonicalize(program).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(cache.path()).map_err(|e| e.to_string())?;
    if !program.starts_with(&root) {
        return Err("Worker program is not a cache asset".into());
    }
    let executable = wide(program.as_os_str());
    // A diagnostics-only setup failure never changes execution authority/results.
    let mut diagnostics = detailed.then(diagnostic_pipe).transpose().ok().flatten();
    let mut command = quote(program.as_os_str());
    for arg in args {
        command.push(b' ' as u16);
        command.extend(quote(arg));
    }
    let normal_command_length = command.len();
    if let Some((_, write)) = &diagnostics {
        command.push(b' ' as u16);
        command.extend(
            wide(OsStr::new(&format!("--diagnostics={}", write.0 as usize)))
                .into_iter()
                .take_while(|c| *c != 0),
        );
    }
    command.push(0);
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    ok(
        unsafe { CreatePipe(&mut read, &mut write, &sa, 0) },
        "create Worker pipe",
    )?;
    let read = OwnedHandle::new(read, "Worker read pipe")?;
    let write = OwnedHandle::new(write, "Worker write pipe")?;
    ok(
        unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) },
        "protect Parent pipe handle",
    )?;
    let mut input = ptr::null_mut();
    let mut send = ptr::null_mut();
    ok(
        unsafe { CreatePipe(&mut input, &mut send, &sa, 0) },
        "create Worker input pipe",
    )?;
    let input = OwnedHandle::new(input, "Worker stdin")?;
    let send = OwnedHandle::new(send, "Parent input writer")?;
    ok(
        unsafe { SetHandleInformation(send.0, HANDLE_FLAG_INHERIT, 0) },
        "protect Parent writer",
    )?;
    let caps = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    // Both arrays outlive Attributes, including the diagnostics-failure branch.
    // A temporary array is invalid: Windows retains the supplied value pointer.
    let ordinary_handles = [input.0, write.0];
    let handles = [
        input.0,
        write.0,
        diagnostics.as_ref().map_or(ptr::null_mut(), |(_, w)| w.0),
    ];
    let jobs = [job.0.0];
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &caps)?;
    if diagnostics.is_some() {
        if attributes
            .set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)
            .is_err()
        {
            // Preserve the ordinary isolated launch if only the optional handle
            // could not be installed. Never remove AppContainer or Job policy.
            diagnostics = None;
            command.truncate(normal_command_length);
            command.push(0);
            attributes = Attributes::new()?;
            attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &caps)?;
            attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &ordinary_handles)?;
        }
    } else {
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &ordinary_handles)?;
    }
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input.0;
    startup.StartupInfo.hStdOutput = write.0;
    startup.StartupInfo.hStdError = write.0;
    startup.lpAttributeList = attributes.list;
    let system = std::env::var_os("SystemRoot").ok_or("SystemRoot unavailable")?;
    let mut variables = std::collections::BTreeMap::new();
    for name in [
        "SYSTEMROOT",
        "WINDIR",
        "SYSTEMDRIVE",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
    ] {
        if let Some(value) = std::env::var_os(name) {
            variables.insert(name.to_string(), value);
        }
    }
    variables.insert("TEMP".into(), root.as_os_str().to_owned());
    variables.insert("TMP".into(), root.as_os_str().to_owned());
    variables.insert(
        "PATH".into(),
        Path::new(&system).join("System32").into_os_string(),
    );
    let mut environment = Vec::new();
    for (name, value) in variables {
        let mut entry = OsString::from(name);
        entry.push("=");
        entry.push(value);
        environment.extend(wide(&entry));
    }
    environment.push(0);
    let directory = wide(root.as_os_str());
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    // No code runs before AppContainer, handle/env allowlists and Job assignment apply.
    ok(
        unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | CREATE_NO_WINDOW,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        },
        "create isolated Worker (no fallback)",
    )?;
    // Success guarantees both handles; from here the owner cannot return an untyped error.
    let process_handle = OwnedHandle(process.hProcess);
    let thread_handle = OwnedHandle(process.hThread);
    drop(write);
    drop(input);
    let diagnostics_read = diagnostics.map(|(read, write)| {
        drop(write);
        read
    });
    let mut running = Box::new(Running {
        _profile: profile,
        job,
        _cache: cache,
        process: process_handle,
        process_id: process.dwProcessId,
        output: None,
        input: None,
        diagnostics: None,
        diagnostics_requested: detailed,
        reason: EndReason::Exited,
        finished: false,
        activity,
        observed: false,
        exchange_used: exchange.is_some(),
        #[cfg(target_arch = "x86_64")]
        _range_session: range_session,
        #[cfg(test)]
        launch_started,
        #[cfg(test)]
        resumed: None,
        #[cfg(test)]
        delivery: prepared.then(|| Arc::new(std::sync::Mutex::new(ProbeClocks::default()))),
    });
    if verify(profile, job, running.process.0).is_err() {
        return Ok(running.stop(EndReason::StartRejected, control.termination_grace));
    }
    if let Some(read) = diagnostics_read {
        let file = unsafe { std::fs::File::from_raw_handle(read.0) };
        std::mem::forget(read);
        running.diagnostics = thread::Builder::new()
            .name("diyime2-worker-diagnostics".into())
            .spawn(move || super::worker_trace::read(file))
            .ok();
    }
    let file = unsafe { std::fs::File::from_raw_handle(read.0) };
    std::mem::forget(read);
    let writer = unsafe { std::fs::File::from_raw_handle(send.0) };
    std::mem::forget(send);
    let overflow = Arc::new(AtomicBool::new(false));
    let exceeded = overflow.clone();
    let pipes = if let Some(exchange) = exchange {
        running.output = Some(thread::spawn(move || exchange(file, writer)));
        None
    } else {
        Some((file, writer))
    };
    if let Some((file, mut writer)) = pipes {
        #[cfg(test)]
        let (ready_send, ready_receive) = if prepared {
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            (Some(send), Some(receive))
        } else {
            (None, None)
        };
        #[cfg(test)]
        let delivery = running.delivery.clone();
        #[cfg(test)]
        let response = running.delivery.clone();
        #[cfg(test)]
        let delivery_cancel = control.cancel.clone();
        running.output = Some(thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut output = file.take((WIRE_LIMIT + 1) as u64);
            #[cfg(test)]
            if let Some(ready) = ready_send {
                // This signal belongs only to the fixed test child.
                while bytes.len() < 4096 && !bytes.ends_with(PROBE_READY) {
                    let mut byte = [0];
                    if output.read_exact(&mut byte).is_err() {
                        return Err("prepared probe exited before readiness".into());
                    }
                    bytes.push(byte[0]);
                }
                if !bytes.ends_with(PROBE_READY) || ready.send(()).is_err() {
                    return Err("prepared probe readiness failed".into());
                }
            }
            #[cfg(test)]
            if let Some(response) = response {
                let mut chunk = [0u8; 4096];
                while let Ok(n) = output.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(frame) = bytes
                        .windows(PROBE_RESULT.len())
                        .position(|b| b == PROBE_RESULT)
                    {
                        let start = frame + PROBE_RESULT.len();
                        if let Some(length) = bytes.get(start + 8..start + 12) {
                            let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
                            if bytes.len() >= start + 12 + length {
                                response.lock().unwrap().response = Some(Instant::now());
                                break;
                            }
                        }
                    }
                }
            }
            let result = output.read_to_end(&mut bytes);
            if bytes.len() > WIRE_LIMIT {
                exceeded.store(true, Ordering::Release);
                return Err("Worker output exceeds wire limit".into());
            }
            match result {
                Ok(_) => Ok(bytes),
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(bytes),
                Err(_) => Err("Worker output pipe failed".into()),
            }
        }));
        running.input = Some(thread::spawn(move || {
            #[cfg(test)]
            if let Some(ready) = ready_receive {
                ready
                    .recv_timeout(Duration::from_secs(30))
                    .map_err(|_| "prepared probe not ready")?;
                if delivery_cancel.is_cancelled() {
                    return Err("prepared probe cancelled before delivery".into());
                }
                delivery
                    .expect("prepared delivery clock")
                    .lock()
                    .unwrap()
                    .sent = Some(Instant::now());
            }
            writer
                .write_all(&request)
                .map_err(|_| "Worker input pipe failed".into())
        }));
    }
    if control.cancel.is_cancelled() {
        return Ok(running.stop(EndReason::Cancelled, control.termination_grace));
    }
    if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
        return Ok(running.stop(EndReason::StartRejected, control.termination_grace));
    }
    #[cfg(test)]
    {
        running.resumed = Some(Instant::now());
    }
    if let Some(activity) = &running.activity {
        activity.fetch_add(1, Ordering::AcqRel);
        running.observed = true;
    }
    let started = Instant::now();
    loop {
        let wait = unsafe { WaitForSingleObject(running.process.0, 10) };
        if wait == WAIT_OBJECT_0 {
            return Ok(Outcome::Exited(running.finish()));
        }
        let reason = if wait != WAIT_TIMEOUT {
            Some(EndReason::WaitFailed)
        } else if control.cancel.is_cancelled() {
            Some(EndReason::Cancelled)
        } else if overflow.load(Ordering::Acquire) {
            Some(EndReason::OutputLimit)
        } else if control
            .walltime
            .is_some_and(|limit| started.elapsed() >= limit)
        {
            Some(EndReason::Deadline)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(running.stop(reason, control.termination_grace));
        }
    }
}
