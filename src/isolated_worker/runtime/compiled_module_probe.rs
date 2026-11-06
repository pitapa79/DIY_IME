//! Developer-only experiment. The parent accepts native bytes only from the
//! successful exit of its own compile-only AppContainer, then holds them in memory.
use super::{Api, Limits, Runtime, compiled_module, require_appcontainer, verify_runtime_bytes};
use crate::isolated_worker::pipe::{PROBE_READY, PROBE_RESULT, WorkerScope, execute_observed};
use crate::isolated_worker::{Cache, EndReason, Job, Outcome, Profile, RunControl, execute};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::Path,
    ptr,
    time::{Duration, Instant},
};

const MARKER: &[u8] = PROBE_RESULT;
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
    if mode == "blocked" {
        std::thread::sleep(Duration::from_secs(10));
        panic!("blocked probe must be stopped before reading input");
    }
    let dll = std::env::current_dir().unwrap().join("wasmtime.dll");
    let measured = mode.ends_with("dll");
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
        "transport" | "ready-transport" => None,
        // SAFETY: the exact read-only DLL was verified immediately above.
        "dll" | "ready-dll" => Some(unsafe { Api::load_verified(&dll) }.unwrap()),
        _ => Some(Api::load(&dll).expect("fixed Wasmtime API")),
    };
    let load_us = load_started.map(|start| start.elapsed().as_micros());
    let prepared_artifact = mode
        .ends_with("-guest")
        .then(|| fs::read("prepared.cwasm").unwrap());
    if mode.starts_with("ready-") {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(PROBE_READY).unwrap();
        stdout.flush().unwrap();
    }
    let input_started = Instant::now();
    let mut input = Vec::new();
    std::io::stdin()
        .take((CAP + 1) as u64)
        .read_to_end(&mut input)
        .unwrap();
    assert!(input.len() <= CAP);
    let started = Instant::now();
    let output = if matches!(
        mode.as_str(),
        "dll" | "transport" | "ready-dll" | "ready-transport"
    ) {
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
            "ready-guest" | "cold-guest" => {
                assert_eq!(input, [42; 32]);
                let expected = fs::read_to_string("artifact.sha256").unwrap();
                compiled_module::load(
                    api,
                    &limits(),
                    None,
                    prepared_artifact.as_ref().unwrap(),
                    &expected,
                )
                .unwrap()
            }
            _ => panic!("invalid probe mode"),
        };
        let request = serde_json::json!({
            "apiVersion": 1,
            "node": {"kind": "text.romaji-to-hiragana", "version": 1},
            "operation": {
                "method": "execute",
                "config": {"overrides": [], "unknown": "preserve"},
                "inputs": {"in": {"state": "data", "packet": {
                    "version": 1, "root": {"type": "text", "text": "kanji"}
                }}},
                "mode": "compose"
            }
        });
        runtime
            .invoke(&serde_json::to_vec(&request).unwrap())
            .unwrap()
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
    let outcome = if mode.starts_with("ready-") {
        execute_observed(
            WorkerScope::prepared(&profile, &job, cache),
            exe,
            &args,
            input,
            &control,
        )
    } else {
        execute(&profile, &job, cache, exe, &args, input, &control)
    };
    let receipt = match outcome.unwrap() {
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
    if mode.starts_with("ready-") {
        let [prepare, delivered_to_result, delivered_to_exit] = **receipt
            .prepared_micros
            .as_ref()
            .expect("readiness and delivery");
        let [_, _, pipe_join] = receipt.phase_micros.expect("resumed probe");
        println!(
            "{mode} prepare_us={prepare} delivered_to_result_us={delivered_to_result} delivered_to_exit_us={delivered_to_exit} pipe_join_us={pipe_join}"
        );
    }
    if matches!(
        mode,
        "dll" | "transport" | "ready-dll" | "ready-transport" | "ready-guest" | "cold-guest"
    ) {
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
    residual_modes(["transport", "dll"]);
}

fn measure_residual(cache: &Cache, exe: &Path, modes: [&str; 2]) {
    for pair in 0..12 {
        let order = if pair % 2 == 0 {
            modes
        } else {
            [modes[1], modes[0]]
        };
        for mode in order {
            let (_, _, result) = invoke(cache, exe, mode, vec![42; 32], None, false).unwrap();
            let result: serde_json::Value = serde_json::from_slice(&result).unwrap();
            assert_eq!(result["verifyUs"].is_number(), mode.ends_with("dll"));
            assert_eq!(
                result["loadAndSymbolsUs"].is_number(),
                mode.ends_with("dll")
            );
            println!("{mode} {result}");
        }
    }
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn prepared() {
    residual_modes(["ready-transport", "ready-dll"]);
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn prepared_guest() {
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let mut naming_profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming_profile).unwrap();
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
    let asset = cache.write("prepared.cwasm", &artifact).unwrap();
    for pair in 0..12 {
        let modes = if pair % 2 == 0 {
            ["ready-guest", "cold-guest"]
        } else {
            ["cold-guest", "ready-guest"]
        };
        for mode in modes {
            let (_, _, reply) =
                invoke(&cache, &exe, mode, vec![42; 32], Some(&expected), false).unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(reply["data"]["out"]["packet"]["root"]["text"], "かんじ");
        }
    }
    for path in [asset, wasm, dll, exe] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming_profile.delete().unwrap();
}

#[test]
#[ignore = "requires real AppContainer"]
fn prepared_withdrawal() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    for reason in [EndReason::Deadline, EndReason::Cancelled] {
        let mut profile = Profile::new().unwrap();
        let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
        let exe = cache
            .copy(&std::env::current_exe().unwrap(), "probe.exe")
            .unwrap();
        let mode = cache.write("mode.txt", b"blocked").unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let job = Job::new().unwrap();
        let control = RunControl {
            walltime: Some(if reason == EndReason::Deadline {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(5)
            }),
            ..RunControl::default()
        };
        let active = Arc::new(AtomicU64::new(0));
        let cancel = (reason == EndReason::Cancelled).then(|| {
            let observed = active.clone();
            let token = control.cancel.clone();
            std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(5);
                while observed.load(Ordering::Acquire) == 0 {
                    assert!(Instant::now() < until, "probe did not launch");
                    std::thread::sleep(Duration::from_millis(1));
                }
                token.cancel();
            })
        });
        let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
        let receipt = match execute_observed(
            WorkerScope::prepared(&profile, &job, &cache).observe_start(active.clone()),
            &exe,
            &args,
            vec![42; 32],
            &control,
        )
        .unwrap()
        {
            Outcome::Exited(receipt) => receipt,
            _ => panic!("withdrawal did not verify the original process exit"),
        };
        if let Some(cancel) = cancel {
            cancel.join().unwrap();
        }
        assert_eq!(receipt.reason(), reason);
        assert!(!receipt.input_delivered());
        assert!(receipt.prepared_micros.is_none());
        assert_eq!(active.load(Ordering::Acquire), 0);
        drop(job);
        cache.revoke_read_execute(&profile).unwrap();
        profile.delete().unwrap();
        fs::remove_file(exe).unwrap();
        fs::remove_file(mode).unwrap();
        fs::remove_dir(cache.path()).unwrap();
    }
}

fn residual_modes(modes: [&str; 2]) {
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let mut naming_profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming_profile).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    measure_residual(&cache, &exe, modes);
    fs::remove_file(dll).unwrap();
    fs::remove_file(exe).unwrap();
    fs::remove_dir(cache.path()).unwrap();
    naming_profile.delete().unwrap();
}
