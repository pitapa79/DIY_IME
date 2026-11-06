//! Developer-only experiment. The parent accepts native bytes only from the
//! successful exit of its own compile-only AppContainer, then holds them in memory.
use super::{Api, Limits, Runtime, compiled_module, require_appcontainer, verify_runtime_bytes};
use crate::isolated_worker::{Cache, EndReason, Job, Outcome, Profile, RunControl, execute};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::Path,
    ptr,
    time::{Duration, Instant},
};

const MARKER: &[u8] = b"\nDIY-MODULE-PROBE\0";
const CAP: usize = 8 * 1024 * 1024 - 4096;
const CHILD: &str = "isolated_worker::runtime::compiled_module_probe::child";

fn digest(bytes: &[u8]) -> String {
    crate::isolated_worker::adapter::sha256(bytes).expect("probe SHA-256")
}

fn limits() -> Limits {
    Limits {
        fuel: Some(50_000_000),
        memory: Some(128 * 1024 * 1024),
        table_elements: None,
        tables: None,
        memories: None,
    }
}

fn wasm() -> Vec<u8> {
    let wasm = fs::read("official.wasm").expect("staged probe Wasm");
    assert_eq!(digest(&wasm), compiled_module::OFFICIAL_WASM_HASH);
    wasm
}

#[test]
#[ignore = "only invoked by compare from its owned AppContainer"]
fn child() {
    require_appcontainer().expect("isolated probe child");
    let mode = fs::read_to_string("mode.txt").expect("read-only probe mode");
    let dll = std::env::current_dir().unwrap().join("wasmtime.dll");
    if mode == "persistent-ipc" {
        ipc_child(&Api::load(&dll).unwrap());
        return;
    }
    let measured = mode == "dll";
    let verify_started = measured.then(Instant::now);
    let (dll_read_us, sha_us) = if measured {
        let bytes = fs::read(&dll).unwrap();
        let read_us = verify_started.unwrap().elapsed().as_micros();
        let hash_started = Instant::now();
        verify_runtime_bytes(&bytes).unwrap();
        (Some(read_us), Some(hash_started.elapsed().as_micros()))
    } else {
        (None, None)
    };
    let verify_us = verify_started.map(|start| start.elapsed().as_micros());
    let load_started = measured.then(Instant::now);
    let api = match mode.as_str() {
        "transport" => None,
        // SAFETY: the exact read-only DLL was verified immediately above.
        "dll" => Some(unsafe { Api::load_verified(&dll) }.unwrap()),
        _ => Some(Api::load(&dll).expect("fixed Wasmtime API")),
    };
    let load_us = load_started.map(|start| start.elapsed().as_micros());
    let input_started = Instant::now();
    let mut input = Vec::new();
    std::io::stdin()
        .take((CAP + 1) as u64)
        .read_to_end(&mut input)
        .unwrap();
    assert!(input.len() <= CAP);
    let started = Instant::now();
    let output = if matches!(mode.as_str(), "dll" | "transport") {
        assert_eq!(input, [42; 32]);
        serde_json::to_vec(&serde_json::json!({
            "verifyUs": verify_us, "loadAndSymbolsUs": load_us,
            "dllReadUs": dll_read_us, "shaUs": sha_us,
            "stdinReadUs": input_started.elapsed().as_micros(),
        }))
        .unwrap()
    } else if mode == "compile" {
        assert!(input.is_empty());
        compiled_module::compile(api.as_ref().unwrap(), &limits(), &wasm()).unwrap()
    } else if mode == "persistent" {
        let expected = fs::read_to_string("artifact.sha256").unwrap();
        persistent_batch(api.as_ref().unwrap(), &input, &expected)
    } else if mode == "persistent-state" {
        assert!(input.is_empty());
        state_probe(api.as_ref().unwrap())
    } else {
        let api = api.as_ref().unwrap();
        let mut runtime = match mode.as_str() {
            "jit" => {
                assert!(input.is_empty());
                Runtime::new(api, &wasm(), &limits(), None).unwrap()
            }
            "reuse" => {
                let expected = fs::read_to_string("artifact.sha256").unwrap();
                compiled_module::load(api, &limits(), None, &input, &expected).unwrap()
            }
            _ => panic!("invalid probe mode"),
        };
        runtime.invoke(&romaji_request("kanji")).unwrap()
    };
    let elapsed = started.elapsed().as_micros() as u64;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(MARKER).unwrap();
    stdout.write_all(&elapsed.to_le_bytes()).unwrap();
    stdout
        .write_all(&(output.len() as u32).to_le_bytes())
        .unwrap();
    stdout.write_all(&output).unwrap();
    stdout.flush().unwrap();
}

fn state_probe(api: &Api) -> Vec<u8> {
    use super::Bytes;
    let fixture = br#"(module
      (memory 1)
      (global $g (mut i32) (i32.const 0))
      (func (export "read") (result i32)
        global.get $g i32.const 0 i32.load i32.or)
      (func (export "mark")
        i32.const 17 global.set $g
        i32.const 0 i32.const 34 i32.store)
      (func (export "trap") unreachable))"#;
    let mut binary = Bytes {
        size: 0,
        data: ptr::null_mut(),
    };
    // SAFETY: the fixture is a fixed local WAT and the vector is Wasmtime-owned.
    let wasm = unsafe {
        api.check(
            (api.wat)(fixture.as_ptr(), fixture.len(), &mut binary),
            ptr::null_mut(),
        )
        .unwrap();
        assert!(!binary.data.is_null() && binary.size > 0);
        let wasm = std::slice::from_raw_parts(binary.data, binary.size).to_vec();
        (api.bytes_delete)(&mut binary);
        wasm
    };
    let limits = limits();
    let mut runtime = Runtime::new(api, &wasm, &limits, None).unwrap();
    runtime.discard_store();
    for _ in 0..64 {
        runtime.install_store(&limits).unwrap();
        runtime.instantiate().unwrap();
        assert_eq!(runtime.call(c"read", &[], Some(0)).unwrap(), 0);
        runtime.call(c"mark", &[], None).unwrap();
        assert_eq!(runtime.call(c"read", &[], Some(0)).unwrap(), 51);
        assert!(runtime.call(c"trap", &[], None).is_err());
        runtime.discard_store();
    }
    br#"{"cleanStores":64,"dirtyControl":true,"traps":64}"#.to_vec()
}

fn romaji_request(text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "apiVersion": 1,
        "node": {"kind": "text.romaji-to-hiragana", "version": 1},
        "operation": {
            "method": "execute",
            "config": {"overrides": [], "unknown": "preserve"},
            "inputs": {"in": {"state": "data", "packet": {
                "version": 1, "root": {"type": "text", "text": text}
            }}},
            "mode": "compose"
        }
    }))
    .unwrap()
}

fn persistent_batch(api: &Api, artifact: &[u8], expected: &str) -> Vec<u8> {
    let limits = limits();
    let initial = Instant::now();
    let mut runtime = compiled_module::load(api, &limits, None, artifact, expected).unwrap();
    runtime.discard_store();
    let initial_us = initial.elapsed().as_micros();
    let requests = [("kanji", "かんじ"), ("kya", "きゃ"), ("ky", "")]
        .map(|(input, output)| (romaji_request(input), output));
    let mut series = Vec::new();
    for (interval_ms, count) in [(0, 2000), (20, 50), (100, 50)] {
        let start = Instant::now();
        let mut rows = Vec::new();
        let mut late_us = Vec::new();
        for index in 0..count {
            let scheduled = start + Duration::from_millis(interval_ms * index);
            if interval_ms != 0 {
                std::thread::sleep(scheduled.saturating_duration_since(Instant::now()));
            }
            let issued = Instant::now();
            let (request, expected) = &requests[index as usize % requests.len()];
            runtime.install_store(&limits).unwrap();
            runtime.instantiate().unwrap();
            let reply = runtime.invoke(request).unwrap();
            runtime.discard_store();
            rows.push(issued.elapsed().as_micros());
            if interval_ms != 0 {
                late_us.push((issued - scheduled).as_micros());
            }
            let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(reply["data"]["out"]["packet"]["root"]["text"], *expected);
        }
        series.push(
            serde_json::json!({"intervalMs":interval_ms,"attempts":count,"correct":count,
            "storeInvokeAndDiscardUs":rows,"scheduleDelayUs":late_us}),
        );
    }
    serde_json::to_vec(&serde_json::json!({"initialModuleUs":initial_us,"series":series})).unwrap()
}

fn invoke(
    cache: &Cache,
    exe: &Path,
    mode: &str,
    input: Vec<u8>,
    expected: Option<&str>,
    rejects_corruption: bool,
) -> Option<(u128, u64, Vec<u8>)> {
    let started = Instant::now();
    let mut profile = Profile::new().unwrap();
    let mode_file = cache.write("mode.txt", mode.as_bytes()).unwrap();
    let digest_file = expected.map(|hash| cache.write("artifact.sha256", hash.as_bytes()).unwrap());
    cache.allow_read_execute(&profile).unwrap();
    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
    let control = RunControl {
        walltime: Some(Duration::from_secs(30)),
        ..RunControl::default()
    };
    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
    let launched = Instant::now();
    let receipt = match execute(&profile, &job, cache, exe, &args, input, &control).unwrap() {
        Outcome::Exited(receipt) => receipt,
        _ => panic!("probe did not verify physical exit"),
    };
    let settled = Instant::now();
    let usage = if rejects_corruption {
        None
    } else {
        Some(job_usage(&job))
    };
    let cleanup_started = Instant::now();
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
    fs::remove_file(mode_file).unwrap();
    if let Some(file) = digest_file {
        fs::remove_file(file).unwrap();
    }
    let host_us = started.elapsed().as_micros();
    if matches!(mode, "dll" | "transport") {
        let [launch, running, join] = receipt.phase_micros.expect("resumed probe");
        println!("{mode} launch_us={launch} resumed_to_exit_us={running} pipe_join_us={join}");
        println!(
            "{mode} staging_us={} process_and_pipes_us={} accounting_us={} cleanup_us={}",
            (launched - started).as_micros(),
            (settled - launched).as_micros(),
            (cleanup_started - settled).as_micros(),
            cleanup_started.elapsed().as_micros()
        );
    }
    assert_eq!(receipt.reason(), EndReason::Exited);
    if rejects_corruption {
        assert_ne!(receipt.exit_code(), Some(0));
        assert!(
            String::from_utf8_lossy(receipt.output().unwrap()).contains("compiled bytes changed")
        );
        return None;
    }
    assert_eq!(
        receipt.exit_code(),
        Some(0),
        "probe child failed: {}",
        String::from_utf8_lossy(receipt.output().unwrap_or_default())
    );
    assert!(receipt.input_delivered());
    let (cpu_100ns, peak_job_bytes) = usage.expect("successful probe usage");
    println!("{mode} cpu_100ns={cpu_100ns} peak_job_bytes={peak_job_bytes}");
    let output = receipt.output().unwrap();
    let start = output
        .windows(MARKER.len())
        .position(|bytes| bytes == MARKER)
        .unwrap()
        + MARKER.len();
    let elapsed = u64::from_le_bytes(output[start..start + 8].try_into().unwrap());
    let len = u32::from_le_bytes(output[start + 8..start + 12].try_into().unwrap()) as usize;
    assert!((1..=CAP).contains(&len));
    Some((
        host_us,
        elapsed,
        output[start + 12..start + 12 + len].to_vec(),
    ))
}

fn job_usage(job: &Job) -> (i64, usize) {
    use windows_sys::Win32::System::JobObjects::{
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject,
    };
    let mut accounting = std::mem::MaybeUninit::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>::uninit();
    let mut limits = std::mem::MaybeUninit::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>::uninit();
    // SAFETY: the still-owned Job handle and each output buffer match the
    // information class. Accounting is read after the process has exited.
    let deadline = Instant::now() + Duration::from_secs(5);
    let accounting = loop {
        let value = unsafe {
            assert_ne!(
                QueryInformationJobObject(
                    job.0.0,
                    JobObjectBasicAccountingInformation,
                    accounting.as_mut_ptr().cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    ptr::null_mut()
                ),
                0
            );
            accounting.assume_init_read()
        };
        if value.ActiveProcesses == 0 {
            break value;
        }
        // A signalled process handle can precede the Job's accounting update.
        assert!(Instant::now() < deadline, "Job accounting did not settle");
        std::thread::sleep(Duration::from_millis(5));
    };
    let limits = unsafe {
        assert_ne!(
            QueryInformationJobObject(
                job.0.0,
                JobObjectExtendedLimitInformation,
                limits.as_mut_ptr().cast(),
                std::mem::size_of_val(&limits) as u32,
                ptr::null_mut()
            ),
            0
        );
        limits.assume_init()
    };
    (
        accounting.TotalUserTime + accounting.TotalKernelTime,
        limits.PeakJobMemoryUsed,
    )
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn compare() {
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let mut staging_profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &staging_profile).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    cache
        .copy(&bundle.join("official/official.wasm"), "official.wasm")
        .unwrap();
    let (compile_host, compile_child, artifact) =
        invoke(&cache, &exe, "compile", Vec::new(), None, false).unwrap();
    let expected = digest(&artifact);
    let mut output = None;
    println!(
        "compile host_us={compile_host} child_us={compile_child} bytes={}",
        artifact.len()
    );
    for pair in 0..8 {
        for mode in if pair % 2 == 0 {
            ["jit", "reuse"]
        } else {
            ["reuse", "jit"]
        } {
            let input = if mode == "reuse" {
                artifact.clone()
            } else {
                Vec::new()
            };
            let (host, child, bytes) = invoke(
                &cache,
                &exe,
                mode,
                input,
                (mode == "reuse").then_some(expected.as_str()),
                false,
            )
            .unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(reply["data"]["out"]["packet"]["root"]["text"], "かんじ");
            if let Some(prior) = &output {
                assert_eq!(prior, &reply, "different guest output");
            } else {
                output = Some(reply);
            }
            println!("{mode} host_us={host} child_us={child}");
        }
    }
    let mut corrupt = artifact;
    corrupt[0] ^= 1;
    assert!(invoke(&cache, &exe, "reuse", corrupt, Some(&expected), true).is_none());
    fs::remove_file(exe).unwrap();
    fs::remove_file(cache.path().join("wasmtime.dll")).unwrap();
    fs::remove_file(cache.path().join("official.wasm")).unwrap();
    fs::remove_dir(cache.path()).unwrap();
    staging_profile.delete().unwrap();
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn residual() {
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let mut naming_profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming_profile).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    for pair in 0..12 {
        let order = if pair % 2 == 0 {
            ["transport", "dll"]
        } else {
            ["dll", "transport"]
        };
        for mode in order {
            let (_, _, result) = invoke(&cache, &exe, mode, vec![42; 32], None, false).unwrap();
            let result: serde_json::Value = serde_json::from_slice(&result).unwrap();
            assert_eq!(result["verifyUs"].is_number(), mode == "dll");
            assert_eq!(result["loadAndSymbolsUs"].is_number(), mode == "dll");
            println!("{mode} {result}");
        }
    }
    fs::remove_file(dll).unwrap();
    fs::remove_file(exe).unwrap();
    fs::remove_dir(cache.path()).unwrap();
    naming_profile.delete().unwrap();
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn persistent() {
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let mut profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    let wasm = cache
        .copy(&bundle.join("official/official.wasm"), "official.wasm")
        .unwrap();
    let (_, _, artifact) = invoke(&cache, &exe, "compile", Vec::new(), None, false).unwrap();
    let expected = digest(&artifact);
    let (host, _, result) =
        invoke(&cache, &exe, "persistent", artifact, Some(&expected), false).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(report["series"].as_array().unwrap().len(), 3);
    println!("persistent host_us={host} report={report}");
    let (_, _, result) = invoke(&cache, &exe, "persistent-state", Vec::new(), None, false).unwrap();
    let state: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(state["cleanStores"], 64);
    println!("persistent-state report={state}");
    for path in [exe, dll, wasm] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    profile.delete().unwrap();
}

const IPC_READY: &[u8] = b"\nDIY-PERSIST-READY\0";
const IPC_CASES: [(&str, &str); 3] = [("kanji", "かんじ"), ("kya", "きゃ"), ("ky", "")];

fn ipc_child(api: &Api) {
    let limits = limits();
    let expected = fs::read_to_string("artifact.sha256").unwrap();
    let artifact = fs::read("probe.cwasm").unwrap();
    assert!(artifact.len() <= CAP);
    let mut runtime = compiled_module::load(api, &limits, None, &artifact, &expected).unwrap();
    runtime.discard_store();
    let requests = IPC_CASES.map(|(input, _)| romaji_request(input));
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output.write_all(IPC_READY).unwrap();
    output.flush().unwrap();
    let mut tag = [0];
    while input.read(&mut tag).unwrap() != 0 {
        if tag[0] == 255 {
            panic!("deliberate isolated probe exit");
        }
        let request = &requests[tag[0] as usize];
        runtime.install_store(&limits).unwrap();
        runtime.instantiate().unwrap();
        let reply = runtime.invoke(request).unwrap();
        runtime.discard_store();
        output
            .write_all(&(reply.len() as u32).to_le_bytes())
            .unwrap();
        output.write_all(&reply).unwrap();
        output.flush().unwrap();
    }
}

fn ipc_wait_ready(output: &mut impl Read) -> std::io::Result<()> {
    let mut prefix = Vec::new();
    while prefix.len() < 4096 {
        let mut byte = [0];
        output.read_exact(&mut byte)?;
        prefix.push(byte[0]);
        if prefix.ends_with(IPC_READY) {
            return Ok(());
        }
    }
    Err(std::io::Error::other("readiness prefix exceeded limit"))
}

fn ipc_request(output: &mut impl Read, input: &mut impl Write, tag: u8) -> std::io::Result<()> {
    input.write_all(&[tag])?;
    let mut size = [0; 4];
    output.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if !(1..=4096).contains(&size) {
        return Err(std::io::Error::other("invalid response size"));
    }
    let mut bytes = vec![0; size];
    output.read_exact(&mut bytes)?;
    let reply: serde_json::Value = serde_json::from_slice(&bytes)?;
    if reply["ok"] != true
        || reply["data"]["out"]["packet"]["root"]["text"] != IPC_CASES[tag as usize].1
    {
        return Err(std::io::Error::other("unexpected probe result"));
    }
    Ok(())
}

fn ipc_series(output: &mut impl Read, input: &mut impl Write) -> std::io::Result<Vec<u8>> {
    let mut series = Vec::new();
    for (interval, count) in [(0, 2000), (20, 50), (100, 50)] {
        let start = Instant::now();
        let mut elapsed = Vec::new();
        let mut late = Vec::new();
        for i in 0..count {
            let scheduled = start + Duration::from_millis(interval * i);
            if interval != 0 {
                std::thread::sleep(scheduled.saturating_duration_since(Instant::now()));
            }
            let issued = Instant::now();
            ipc_request(output, input, (i % 3) as u8)?;
            elapsed.push(issued.elapsed().as_micros());
            if interval != 0 {
                late.push((issued - scheduled).as_micros());
            }
        }
        series.push(
            serde_json::json!({"intervalMs":interval,"correct":count,"attempts":count,
            "roundtripUs":elapsed,"scheduleDelayUs":late}),
        );
    }
    serde_json::to_vec(&series).map_err(std::io::Error::other)
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn persistent_ipc() {
    use crate::isolated_worker::pipe::{WorkerScope, execute_observed};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let mut naming = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    let wasm = cache
        .copy(&bundle.join("official/official.wasm"), "official.wasm")
        .unwrap();
    let (_, _, artifact) = invoke(&cache, &exe, "compile", Vec::new(), None, false).unwrap();
    let hash = cache
        .write("artifact.sha256", digest(&artifact).as_bytes())
        .unwrap();
    let compiled = cache.write("probe.cwasm", &artifact).unwrap();
    let mode_file = cache.write("mode.txt", b"persistent-ipc").unwrap();
    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
    for scenario in ["normal", "cancel", "crash", "reopen"] {
        let mut profile = Profile::new().unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let job = Job::new().unwrap();
        let control = RunControl {
            walltime: Some(Duration::from_secs(30)),
            ..RunControl::default()
        };
        let token = control.cancel.clone();
        let activity = Arc::new(AtomicU64::new(0));
        let scope = WorkerScope::observed(&profile, &job, &cache, activity.clone())
            .with_test_exchange(Box::new(move |mut output, mut input| {
                let result = (|| -> std::io::Result<Vec<u8>> {
                    ipc_wait_ready(&mut output)?;
                    match scenario {
                        "normal" => ipc_series(&mut output, &mut input),
                        "cancel" => {
                            ipc_request(&mut output, &mut input, 0)?;
                            token.cancel();
                            // Keep stdin open until the cancellation path kills the child.
                            let mut byte = [0];
                            assert_ne!(output.read(&mut byte).unwrap_or(0), 1);
                            Ok(Vec::new())
                        }
                        "crash" => {
                            input.write_all(&[255])?;
                            let mut size = [0; 4];
                            if output.read_exact(&mut size).is_ok()
                                && u32::from_le_bytes(size) <= 4096
                            {
                                return Err(std::io::Error::other("unexpected crash result"));
                            }
                            Ok(Vec::new())
                        }
                        _ => {
                            ipc_request(&mut output, &mut input, 1)?;
                            Ok(Vec::new())
                        }
                    }
                })();
                drop(input);
                let mut tail = Vec::new();
                output
                    .take(8193)
                    .read_to_end(&mut tail)
                    .map_err(|e| e.to_string())?;
                if tail.len() > 8192 {
                    return Err("unexpected probe tail size".into());
                }
                result.map_err(|e| e.to_string())
            }));
        let receipt = match execute_observed(scope, &exe, &args, Vec::new(), &control).unwrap() {
            Outcome::Exited(receipt) => receipt,
            _ => panic!("session did not prove process exit"),
        };
        assert_eq!(activity.load(Ordering::Acquire), 0);
        let bytes = receipt.output().unwrap();
        match scenario {
            "cancel" => assert_eq!(receipt.reason(), EndReason::Cancelled),
            "crash" => {
                assert_eq!(receipt.reason(), EndReason::Exited);
                assert_ne!(receipt.exit_code(), Some(0));
            }
            _ => {
                assert_eq!(receipt.reason(), EndReason::Exited);
                assert_eq!(receipt.exit_code(), Some(0));
            }
        }
        let (cpu, peak) = job_usage(&job);
        println!("ipc {scenario} cpu_100ns={cpu} peak_job_bytes={peak}");
        if scenario == "normal" {
            println!("ipc report={}", String::from_utf8_lossy(bytes));
        }
        drop(job);
        cache.revoke_read_execute(&profile).unwrap();
        profile.delete().unwrap();
    }
    for path in [exe, dll, wasm, hash, compiled, mode_file] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}
