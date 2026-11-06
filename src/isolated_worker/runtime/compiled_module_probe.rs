//! Developer-only experiment. The parent accepts native bytes only from the
//! successful exit of its own compile-only AppContainer, then holds them in memory.
use super::{Api, Bytes, Limits, P, Runtime, create_engine, require_appcontainer};
use crate::isolated_worker::{Cache, EndReason, Job, Outcome, Profile, RunControl, execute};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::Path,
    ptr,
    time::{Duration, Instant},
};

const MODULE_HASH: &str = "105c0ceb2854dba27a747eae5faa6e393bb99a88ce5ab3048a468fa9add88134";
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
    assert_eq!(digest(&wasm), MODULE_HASH);
    wasm
}

#[test]
#[ignore = "only invoked by compare from its owned AppContainer"]
fn child() {
    require_appcontainer().expect("isolated probe child");
    let mode = fs::read_to_string("mode.txt").expect("read-only probe mode");
    let dll = std::env::current_dir().unwrap().join("wasmtime.dll");
    let api = Api::load(&dll).expect("fixed Wasmtime API");
    let mut input = Vec::new();
    std::io::stdin()
        .take((CAP + 1) as u64)
        .read_to_end(&mut input)
        .unwrap();
    assert!(input.len() <= CAP);
    let started = Instant::now();
    let output = if mode == "compile" {
        assert!(input.is_empty());
        let wasm = wasm();
        let engine = create_engine(&api, &limits()).unwrap();
        let mut module = ptr::null_mut();
        let mut bytes = Bytes {
            size: 0,
            data: ptr::null_mut(),
        };
        // SAFETY: signatures and ownership follow the pinned C header. This
        // branch never creates a Store or invokes code from the input module.
        unsafe {
            let serialize: unsafe extern "C" fn(P, *mut Bytes) -> P =
                symbol(&api, c"wasmtime_module_serialize");
            api.check(
                (api.module_new)(engine, wasm.as_ptr(), wasm.len(), &mut module),
                ptr::null_mut(),
            )
            .unwrap();
            api.check(serialize(module, &mut bytes), ptr::null_mut())
                .unwrap();
            assert!((1..=CAP).contains(&bytes.size) && !bytes.data.is_null());
            let output = std::slice::from_raw_parts(bytes.data, bytes.size).to_vec();
            (api.bytes_delete)(&mut bytes);
            (api.module_delete)(module);
            (api.engine_delete)(engine);
            output
        }
    } else {
        let mut runtime = match mode.as_str() {
            "jit" => {
                assert!(input.is_empty());
                Runtime::new(&api, &wasm(), &limits(), None).unwrap()
            }
            "reuse" => {
                let expected = fs::read_to_string("artifact.sha256").unwrap();
                assert_eq!(digest(&input), expected, "compiled bytes changed");
                Runtime::with_module(&api, &limits(), None, |engine, module| {
                    // SAFETY: the parent generated the bytes with the same DLL,
                    // Wasm and engine policy and staged their digest read-only.
                    unsafe {
                        let deserialize: unsafe extern "C" fn(P, *const u8, usize, *mut P) -> P =
                            symbol(&api, c"wasmtime_module_deserialize");
                        api.check(
                            deserialize(engine, input.as_ptr(), input.len(), module),
                            ptr::null_mut(),
                        )
                    }
                })
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

unsafe fn symbol<T: Copy>(api: &Api, name: &std::ffi::CStr) -> T {
    // SAFETY: callers select exact types for the verified pinned DLL exports.
    let pointer = unsafe { super::GetProcAddress(api._library.0, name.as_ptr().cast()) };
    assert!(!pointer.is_null());
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of_val(&pointer));
    unsafe { std::mem::transmute_copy(&pointer) }
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
    let receipt = match execute(&profile, &job, cache, exe, &args, input, &control).unwrap() {
        Outcome::Exited(receipt) => receipt,
        _ => panic!("probe did not verify physical exit"),
    };
    let usage = if rejects_corruption {
        None
    } else {
        Some(job_usage(&job))
    };
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
    fs::remove_file(mode_file).unwrap();
    if let Some(file) = digest_file {
        fs::remove_file(file).unwrap();
    }
    let host_us = started.elapsed().as_micros();
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
