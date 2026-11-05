//! Fake-data OS integration fixture for the Host-owned Worker pipe.
#[cfg(windows)]
mod probe {
    use diy_ime2_core::{
        execution_control::CancellationToken,
        isolated_worker::{self as worker, EndReason, ExitReceipt, Outcome, RunControl},
    };
    use serde_json::{Value, json};
    use std::{
        ffi::OsString,
        io::{Read, Write},
        path::Path,
        time::{Duration, Instant},
    };

    fn resolve(outcome: Outcome<'_>) -> Result<(ExitReceipt, bool), String> {
        match outcome {
            Outcome::Exited(reply) => Ok((reply, false)),
            Outcome::Unconfirmed(owner) => match owner.retry(Duration::from_secs(5)) {
                Outcome::Exited(reply) => Ok((reply, true)),
                _ => Err("Worker remained unconfirmed; owner retained until actual exit".into()),
            },
            Outcome::CancelledBeforeStart => {
                Err("fixture unexpectedly cancelled before start".into())
            }
        }
    }
    fn case(
        program: &Path,
        args: &[OsString],
        input: Vec<u8>,
        control: &RunControl,
    ) -> Result<(ExitReceipt, bool), String> {
        let mut profile = worker::Profile::new()?;
        let cache = worker::Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
        let exe = cache.copy(program, "pipe-worker.exe")?;
        cache.allow_read_execute(&profile)?;
        let job = worker::Job::new()?;
        let (reply, quarantined) = resolve(worker::execute(
            &profile, &job, &cache, &exe, args, input, control,
        )?)?;
        // Only executable assets were staged; the invocation was never written here.
        if std::fs::read_dir(cache.path())
            .map_err(|_| "cache inspect failed")?
            .count()
            != 1
        {
            return Err("invocation persisted in Worker cache".into());
        }
        drop(job);
        profile.delete()?;
        Ok((reply, quarantined))
    }
    fn child_worker() -> Result<Option<u32>, String> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
        };
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("process observation unavailable".into());
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut valid = unsafe { Process32FirstW(snapshot, &mut entry) };
        let mut found = None;
        while valid != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if entry.th32ParentProcessID == std::process::id()
                && name.eq_ignore_ascii_case("pipe-worker.exe")
            {
                found = Some(entry.th32ProcessID);
                break;
            }
            valid = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        unsafe {
            CloseHandle(snapshot);
        }
        Ok(found)
    }
    fn cancel_after(delay: Duration) -> (RunControl, std::thread::JoinHandle<Result<u32, String>>) {
        let cancel = CancellationToken::default();
        let owner = cancel.clone();
        let thread = std::thread::spawn(move || {
            // Slow executable staging is not part of the live-cancellation delay.
            // Observe only this fixture's actual staged child, never conhost/PID guesses.
            let detected = (|| {
                let end = Instant::now() + Duration::from_secs(20);
                loop {
                    if let Some(id) = child_worker()? {
                        return Ok(id);
                    }
                    if Instant::now() >= end {
                        return Err("fixture child was not observed before cancellation".into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })();
            if detected.is_ok() {
                std::thread::sleep(delay);
            }
            owner.cancel();
            detected
        });
        (
            RunControl {
                cancel,
                termination_grace: Duration::ZERO,
                ..Default::default()
            },
            thread,
        )
    }
    fn suite() -> Result<Value, String> {
        let source = std::env::current_exe().map_err(|e| e.to_string())?;
        let input = "fake-pipe-canary\0😀".repeat(32768).into_bytes();
        let (reply, _) = case(
            &source,
            &[OsString::from("--echo")],
            input.clone(),
            &RunControl::default(),
        )?;
        if reply.reason() != EndReason::Exited
            || reply.exit_code() != Some(0)
            || !reply.input_delivered()
            || reply.output()? != input
        {
            return Err("pipe roundtrip failed".into());
        }
        let started = Instant::now();
        let (reply, _) = case(
            &source,
            &[OsString::from("--slow")],
            b"slow-canary".to_vec(),
            &RunControl::default(),
        )?;
        if reply.exit_code() != Some(0) || started.elapsed() < Duration::from_millis(200) {
            return Err("deadline-free healthy Worker did not complete".into());
        }
        let mut quarantined = 0;
        for mode in ["--hold", "--fake-exit", "--no-read"] {
            let (control, cancel_thread) = cancel_after(Duration::from_millis(150));
            let outcome = case(
                &source,
                &[OsString::from(mode)],
                vec![42; 1024 * 1024],
                &control,
            );
            let observed = cancel_thread
                .join()
                .map_err(|_| "cancel fixture panicked")??;
            let (reply, pending) = outcome?;
            if reply.process_id() != observed {
                return Err("cancellation observed a different fixture process".into());
            }
            if reply.reason() != EndReason::Cancelled {
                return Err("cancellation did not await actual exit".into());
            }
            quarantined += usize::from(pending);
        }
        let control = RunControl {
            walltime: Some(Duration::from_millis(100)),
            ..Default::default()
        };
        let (reply, _) = case(&source, &[OsString::from("--hold")], Vec::new(), &control)?;
        if reply.reason() != EndReason::Deadline {
            return Err("explicit deadline not observed".into());
        }
        let (reply, _) = case(
            &source,
            &[OsString::from("--spam")],
            Vec::new(),
            &RunControl::default(),
        )?;
        if reply.reason() != EndReason::OutputLimit || reply.output().is_ok() {
            return Err("oversized output was not stopped/refused".into());
        }
        let mut profile = worker::Profile::new()?;
        let cache = worker::Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
        let exe = cache.copy(&source, "pipe-worker.exe")?;
        cache.allow_read_execute(&profile)?;
        let job = worker::Job::new()?;
        let control = RunControl::default();
        control.cancel.cancel();
        if !matches!(
            worker::execute(&profile, &job, &cache, &exe, &[], Vec::new(), &control)?,
            Outcome::CancelledBeforeStart
        ) {
            return Err("pre-start cancellation launched a Worker".into());
        }
        if worker::execute(
            &profile,
            &job,
            &cache,
            &exe,
            &[],
            vec![0; 8 * 1024 * 1024 + 1],
            &RunControl::default(),
        )
        .is_ok()
        {
            return Err("oversized input accepted".into());
        }
        drop(job);
        profile.delete()?;
        Ok(
            json!({"kind":"worker-pipe-suite","roundtrip":true,"requestFiles":0,"noDeadline":true,"cancelCases":3,"explicitDeadline":true,"outputLimit":true,"preStartCancel":true,"inputLimit":true,"unconfirmedOwnersObserved":quarantined,"physicalExit":true}),
        )
    }
    fn wasm(args: &[OsString]) -> Result<Value, String> {
        if args.len() != 4 {
            return Err("expected probe dll wasm fake-request.json".into());
        }
        let mut profile = worker::Profile::new()?;
        let cache = worker::Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
        let exe = cache.copy(Path::new(&args[0]), "wasm-worker.exe")?;
        let dll = cache.copy(Path::new(&args[1]), "wasmtime.dll")?;
        let module = cache.copy(Path::new(&args[2]), "node.wasm")?;
        let input = std::fs::read(&args[3]).map_err(|_| "fake request unavailable")?;
        cache.allow_read_execute(&profile)?;
        let job = worker::Job::new()?;
        let (reply, _) = resolve(worker::execute(
            &profile,
            &job,
            &cache,
            &exe,
            &[
                dll.into_os_string(),
                module.into_os_string(),
                OsString::from("-"),
            ],
            input,
            &RunControl::default(),
        )?)?;
        if reply.exit_code() != Some(0) || !reply.input_delivered() {
            return Err(format!(
                "isolated Wasm failed: {}",
                String::from_utf8_lossy(reply.output()?)
            ));
        }
        let result: Value =
            serde_json::from_slice(reply.output()?).map_err(|_| "invalid Wasm JSON")?;
        if std::fs::read_dir(cache.path())
            .map_err(|_| "cache inspect failed")?
            .count()
            != 3
        {
            return Err("request was persisted".into());
        }
        drop(job);
        profile.delete()?;
        Ok(
            json!({"kind":"worker-pipe-wasm","physicalExit":true,"requestFiles":0,"profileDeleted":true,"result":result}),
        )
    }
    pub fn run() -> Result<Option<Value>, String> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let mode = args.first().and_then(|s| s.to_str());
        match mode {
            Some("--no-read") => loop {
                std::thread::sleep(Duration::from_secs(1));
            },
            Some("--spam") => {
                let bytes = vec![42; 9 * 1024 * 1024];
                let _ = std::io::stdout().write_all(&bytes);
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            Some("--echo" | "--slow" | "--hold" | "--fake-exit") => {
                let mut input = Vec::new();
                std::io::stdin()
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut input)
                    .map_err(|_| "input failed")?;
                if mode == Some("--fake-exit") {
                    println!("{{\"physicalExit\":true}}");
                }
                if matches!(mode, Some("--hold" | "--fake-exit")) {
                    loop {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                if mode == Some("--slow") {
                    std::thread::sleep(Duration::from_millis(250));
                }
                std::io::stdout()
                    .write_all(&input)
                    .map_err(|_| "output failed")?;
                Ok(None)
            }
            Some("suite") => suite().map(Some),
            Some("wasm") => wasm(&args[1..]).map(Some),
            _ => Err(
                "usage: worker_pipe_probe suite | wasm <probe> <dll> <wasm> <fake-request>".into(),
            ),
        }
    }
}
fn main() {
    #[cfg(windows)]
    match probe::run() {
        Ok(Some(value)) => println!("{value}"),
        Ok(None) => (),
        Err(error) => {
            eprintln!("worker-pipe: {error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Worker OS isolation unavailable; refusing launch");
        std::process::exit(1);
    }
}
