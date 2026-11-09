//! Test-only same-module control for one-shot Windows Workers.
use super::*;
use serde_json::json;

const CASES: [(&str, &str); 3] = [
    ("generic-probe", "generic-probe"),
    ("候補", "候補"),
    ("", ""),
];

fn request(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "apiVersion": 1,
        "node": {"kind":"fixture.wasm.chain","version":1},
        "operation": {"method":"execute","config":{"mode":"forward"},
            "inputs":{"in":{"state":"data","packet":{"version":1,
                "root":{"type":"text","text":text}}}}}
    }))
    .unwrap()
}

pub(super) fn child(api: &Api, mode: &str, input: &[u8]) -> Vec<u8> {
    if mode == "generic-reuse" {
        let expected = fs::read_to_string("artifact.sha256").unwrap();
        return compiled_module::load(api, &limits(), None, input, &expected)
            .unwrap()
            .invoke(&request(CASES[0].0))
            .unwrap();
    }
    let bytes = fs::read("generic.wasm").unwrap();
    let expected = fs::read_to_string("generic.sha256").unwrap();
    if mode == "generic-compile" {
        return compiled_module::compile_fixture_for_probe(api, &limits(), &bytes, &expected)
            .unwrap();
    }
    assert_eq!(mode, "generic-jit");
    assert_eq!(digest(&bytes), expected);
    Runtime::new(api, &bytes, &limits(), None)
        .unwrap()
        .invoke(&request(CASES[0].0))
        .unwrap()
}

pub(super) fn persistent_child(api: &Api) {
    let artifact = fs::read("prepared.cwasm").unwrap();
    let expected = fs::read_to_string("artifact.sha256").unwrap();
    let limits = limits();
    let mut runtime =
        compiled_module::prepare_for_probe(api, &limits, &artifact, &expected).unwrap();
    runtime.discard_store();
    let requests = CASES.map(|(text, _)| request(text));
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output.write_all(IPC_READY).unwrap();
    output.flush().unwrap();
    let mut tag = [0];
    while input.read(&mut tag).unwrap() != 0 {
        let reply = invoke_fresh(&mut runtime, &limits, &requests[tag[0] as usize]);
        output
            .write_all(&(reply.len() as u32).to_le_bytes())
            .unwrap();
        output.write_all(&reply).unwrap();
        output.flush().unwrap();
    }
}

fn measure_persistent(cache: &Cache, exe: &Path) {
    use std::sync::{Arc, atomic::AtomicU64};
    let started = Instant::now();
    let mut profile = Profile::new().unwrap();
    let mode = cache.write("mode.txt", b"generic-persistent").unwrap();
    cache.allow_read_execute(&profile).unwrap();
    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
    let control = RunControl {
        walltime: Some(Duration::from_secs(30)),
        ..RunControl::default()
    };
    let scope = WorkerScope::observed(&profile, &job, cache, Arc::new(AtomicU64::new(0)))
        .with_test_exchange(Box::new(move |mut output, mut input| {
            ipc_wait_ready(&mut output).map_err(|error| error.to_string())?;
            let ready_us = started.elapsed().as_micros();
            let samples = ipc_series(&mut output, &mut input, CASES, 200)
                .map_err(|error| error.to_string())?;
            drop(input);
            let close_requested_us = started.elapsed().as_micros();
            let mut tail = Vec::new();
            output
                .take(8193)
                .read_to_end(&mut tail)
                .map_err(|error| error.to_string())?;
            if tail.len() > 8192 {
                return Err("unexpected probe tail size".into());
            }
            let series: serde_json::Value =
                serde_json::from_slice(&samples).map_err(|error| error.to_string())?;
            serde_json::to_vec(&json!({
                "readyUs":ready_us,"closeRequestedUs":close_requested_us,
                "series":series
            }))
            .map_err(|error| error.to_string())
        }));
    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
    let receipt = match execute_observed(scope, exe, &args, Vec::new(), &control).unwrap() {
        Outcome::Exited(receipt) => receipt,
        _ => panic!("retained probe exit was not verified"),
    };
    assert_eq!(receipt.reason(), EndReason::Exited);
    assert_eq!(receipt.exit_code(), Some(0));
    let observed_exit_us = started.elapsed().as_micros();
    let (cpu, peak) = job_usage(&job);
    let report: serde_json::Value = serde_json::from_slice(receipt.output().unwrap()).unwrap();
    assert_eq!(report["series"][0]["correct"], 200);
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
    fs::remove_file(mode).unwrap();
    println!(
        "generic-persistent observed_exit_us={observed_exit_us} recovered_us={} cpu_100ns={cpu} peak_job_bytes={peak} report={report}",
        started.elapsed().as_micros()
    );
}

#[test]
#[ignore = "requires real Windows AppContainer, pinned DLL and the test-only chain guest"]
fn compare_generic_artifact() {
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let fixture = fs::canonicalize(std::env::var_os("DIY_GENERIC_PROBE_WASM").unwrap()).unwrap();
    let mut profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    let module = cache.copy(&fixture, "generic.wasm").unwrap();
    let module_hash = digest(&fs::read(&fixture).unwrap());
    let module_hash_file = cache
        .write("generic.sha256", module_hash.as_bytes())
        .unwrap();
    let (host_us, child_us, artifact) =
        invoke(&cache, &exe, "generic-compile", Vec::new(), None, false).unwrap();
    let artifact_hash = digest(&artifact);
    println!(
        "generic-compile host_us={host_us} child_us={child_us} module_sha={module_hash} artifact_sha={artifact_hash} bytes={}",
        artifact.len()
    );
    let mut expected = None;
    let prepared = cache.write("prepared.cwasm", &artifact).unwrap();
    for pair in 0..24 {
        let modes = if pair % 2 == 0 {
            ["generic-jit", "generic-reuse", "ready-generic-module-guest"]
        } else {
            ["ready-generic-module-guest", "generic-reuse", "generic-jit"]
        };
        for mode in modes {
            let reuse = mode == "generic-reuse";
            let ready = mode == "ready-generic-module-guest";
            let input = if reuse {
                artifact.clone()
            } else if ready {
                request(CASES[0].0)
            } else {
                Vec::new()
            };
            let (host_us, child_us, reply) = invoke(
                &cache,
                &exe,
                mode,
                input,
                (reuse || ready).then_some(artifact_hash.as_str()),
                false,
            )
            .unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(
                reply["data"]["out"]["packet"]["root"]["text"],
                "generic-probe"
            );
            if let Some(previous) = &expected {
                assert_eq!(previous, &reply);
            } else {
                expected = Some(reply);
            }
            println!("{mode} pair={pair} host_us={host_us} child_us={child_us}");
        }
    }
    let mut corrupt = artifact;
    corrupt[0] ^= 1;
    assert!(
        invoke(
            &cache,
            &exe,
            "generic-reuse",
            corrupt,
            Some(&artifact_hash),
            true
        )
        .is_none()
    );
    let staged_hash = cache
        .write("artifact.sha256", artifact_hash.as_bytes())
        .unwrap();
    measure_persistent(&cache, &exe);
    for path in [staged_hash, prepared, module_hash_file, module, dll, exe] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    profile.delete().unwrap();
}

#[test]
#[ignore = "requires real Windows AppContainer, pinned DLL and the test-only chain guest"]
fn compare_generic_one_shot_refill() {
    refill(false);
}

#[test]
#[ignore = "requires real Windows AppContainer, pinned DLL and the test-only chain guest"]
fn compare_generic_raw_one_shot_refill() {
    refill(true);
}

fn refill(raw: bool) {
    use std::{
        sync::{Mutex, atomic::AtomicU64},
        thread,
    };

    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let fixture = fs::canonicalize(std::env::var_os("DIY_GENERIC_PROBE_WASM").unwrap()).unwrap();
    let mut naming = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    let module = cache.copy(&fixture, "generic.wasm").unwrap();
    let module_hash_file = cache
        .write(
            "generic.sha256",
            digest(&fs::read(&fixture).unwrap()).as_bytes(),
        )
        .unwrap();
    let artifacts = if raw { None } else {
        let (_, _, artifact) = invoke(&cache, &exe, "generic-compile", Vec::new(), None, false).unwrap();
        Some((cache.write("prepared.cwasm", &artifact).unwrap(), cache.write("artifact.sha256", digest(&artifact).as_bytes()).unwrap()))
    };
    let mode = cache.write("mode.txt", if raw { b"ready-generic-jit" } else { b"ready-generic-module-guest" }).unwrap();
    let acl = Mutex::new(());
    let count = 32;
    for interval_ms in [20, 100] {
        for lanes in [1, 2] {
            for prewarm in [false, true, true, false] {
                let epoch = Instant::now() + Duration::from_millis(180);
                thread::scope(|scope| {
                    for lane in 0..lanes {
                        let (cache, exe, acl) = (&cache, &exe, &acl);
                        scope.spawn(move || {
                            for index in (lane..count).step_by(lanes) {
                                let scheduled = epoch + Duration::from_millis(index as u64 * interval_ms);
                                if !prewarm {
                                    thread::sleep(scheduled.saturating_duration_since(Instant::now()));
                                }
                                let started = Instant::now();
                                let mut profile = Profile::new().unwrap();
                                {
                                    let _guard = acl.lock().unwrap();
                                    cache.allow_read_execute(&profile).unwrap();
                                }
                                let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
                                let control = RunControl {
                                    walltime: Some(Duration::from_secs(10)),
                                    ..RunControl::default()
                                };
                                let text = format!("入力-{index}");
                                let bytes = request(&text);
                                let (ready_send, ready_receive) = std::sync::mpsc::sync_channel(1);
                                let worker = WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into())
                                    .with_test_exchange(Box::new(move |output, mut input| {
                                        let exchange = || -> std::io::Result<_> {
                                            let mut output = std::io::BufReader::new(output);
                                            speculative::read_marker(&mut output, PROBE_READY)?;
                                            let ready = Instant::now();
                                            thread::sleep(scheduled.saturating_duration_since(ready));
                                            let sent = Instant::now();
                                            ready_send.send((ready, sent)).map_err(std::io::Error::other)?;
                                            input.write_all(&bytes)?;
                                            drop(input);
                                            let mut result = Vec::new();
                                            output.take((CAP + 1) as u64).read_to_end(&mut result)?;
                                            assert!(result.len() <= CAP);
                                            Ok(result)
                                        };
                                        exchange().map_err(|error| error.to_string())
                                    }));
                                let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
                                let Outcome::Exited(receipt) =
                                    execute_observed(worker, exe, &args, Vec::new(), &control).unwrap()
                                else {
                                    panic!("refill probe did not verify physical exit");
                                };
                                let confirmed = Instant::now();
                                assert_eq!(receipt.reason(), EndReason::Exited);
                                assert_eq!(receipt.exit_code(), Some(0));
                                let (_, bytes) = probe_frame(receipt.output().unwrap());
                                let result: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                                assert_eq!(result["ok"], true);
                                assert_eq!(result["data"]["out"]["packet"]["root"]["text"], text);
                                let (ready, sent) = ready_receive.recv().unwrap();
                                let (cpu, peak) = job_usage(&job);
                                drop(job);
                                {
                                    let _guard = acl.lock().unwrap();
                                    cache.revoke_read_execute(&profile).unwrap();
                                    profile.delete().unwrap();
                                }
                                let recovered = Instant::now();
                                println!(
                                    "generic-refill raw={raw} interval_ms={interval_ms} lanes={lanes} prewarm={prewarm} input={index} hit={} ready_us={} queue_us={} confirmed_us={} recovered_us={} cpu_100ns={cpu} peak_job_bytes={peak}",
                                    ready <= scheduled, (ready - started).as_micros(),
                                    sent.saturating_duration_since(scheduled).as_micros(),
                                    confirmed.saturating_duration_since(scheduled).as_micros(),
                                    recovered.saturating_duration_since(scheduled).as_micros(),
                                );
                            }
                        });
                    }
                });
            }
        }
    }
    if let Some((prepared, hash_file)) = artifacts {
        fs::remove_file(prepared).unwrap();
        fs::remove_file(hash_file).unwrap();
    }
    for path in [mode, module_hash_file, module, dll, exe] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}

#[test]
#[ignore = "requires real Windows AppContainer, pinned DLL and the test-only chain guest"]
fn cancel_generic_raw_idle_worker() {
    use crate::execution_control::CancellationToken;
    use std::{sync::{atomic::AtomicU64, mpsc}, thread};
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let fixture = fs::canonicalize(std::env::var_os("DIY_GENERIC_PROBE_WASM").unwrap()).unwrap();
    let mut naming = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming).unwrap();
    let exe = cache.copy(&std::env::current_exe().unwrap(), "probe.exe").unwrap();
    let dll = cache.copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll").unwrap();
    let module = cache.copy(&fixture, "generic.wasm").unwrap();
    let hash = cache.write("generic.sha256", digest(&fs::read(&fixture).unwrap()).as_bytes()).unwrap();
    let mode = cache.write("mode.txt", b"ready-generic-jit").unwrap();
    for index in 0..16 {
        let mut profile = Profile::new().unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
        let cancel = CancellationToken::default();
        let control = RunControl { cancel: cancel.clone(), walltime: Some(Duration::from_secs(10)), ..RunControl::default() };
        let (ready_send, ready_receive) = mpsc::sync_channel(1);
        let started = Instant::now();
        thread::scope(|scope| {
            let worker = WorkerScope::observed(&profile, &job, &cache, AtomicU64::new(0).into())
                .with_test_exchange(Box::new(move |output, _input| {
                    let mut output = std::io::BufReader::new(output);
                    speculative::read_marker(&mut output, PROBE_READY).map_err(|e| e.to_string())?;
                    ready_send.send(Instant::now()).map_err(|e| e.to_string())?;
                    let mut tail = Vec::new();
                    output.take((CAP + 1) as u64).read_to_end(&mut tail).map_err(|e| e.to_string())?;
                    Ok(tail)
                }));
            let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
            let cancel_thread = scope.spawn(move || {
                let ready = ready_receive.recv_timeout(Duration::from_secs(10)).unwrap();
                let cancelled = Instant::now();
                cancel.cancel();
                (ready, cancelled)
            });
            let Outcome::Exited(receipt) = execute_observed(worker, &exe, &args, Vec::new(), &control).unwrap() else { panic!("physical exit unconfirmed") };
            let (ready, cancelled) = cancel_thread.join().unwrap();
            assert_eq!(receipt.reason(), EndReason::Cancelled);
            let signalled_us = receipt.signalled_at.saturating_duration_since(cancelled).as_micros();
            let (cpu, peak) = job_usage(&job);
            drop(job);
            cache.revoke_read_execute(&profile).unwrap();
            profile.delete().unwrap();
            println!("raw-idle-cancel index={index} ready_us={} signalled_us={signalled_us} recovered_us={} cpu_100ns={cpu} peak_job_bytes={peak}", ready.duration_since(started).as_micros(), cancelled.elapsed().as_micros());
        });
    }
    for path in [mode, hash, module, dll, exe] { fs::remove_file(path).unwrap(); }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}
#[test]
#[ignore = "requires real Windows AppContainer, pinned DLL and the test-only chain guest"]
fn compare_raw_controlled_miss_fallback() {
    use crate::execution_control::CancellationToken;
    use std::{sync::{atomic::AtomicU64, mpsc}, thread};
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let fixture = fs::canonicalize(std::env::var_os("DIY_GENERIC_PROBE_WASM").unwrap()).unwrap();
    let mut naming = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming).unwrap();
    let exe = cache.copy(&std::env::current_exe().unwrap(), "probe.exe").unwrap();
    let dll = cache.copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll").unwrap();
    let module = cache.copy(&fixture, "generic.wasm").unwrap();
    let hash = cache.write("generic.sha256", digest(&fs::read(&fixture).unwrap()).as_bytes()).unwrap();
    let mode = cache.write("mode.txt", b"ready-generic-jit").unwrap();
    for pair in 0..16 {
        for miss in if pair % 2 == 0 { [false, true] } else { [true, false] } {
            let text = format!("fallback-{pair}");
            let mut arrival = None;
            let mut speculative_exit_us = 0;
            let mut speculative_recovery_us = 0;
            if miss {
                let mut profile = Profile::new().unwrap();
                cache.allow_read_execute(&profile).unwrap();
                let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
                let cancel = CancellationToken::default();
                let control = RunControl { cancel: cancel.clone(), walltime: Some(Duration::from_secs(10)), ..RunControl::default() };
                let (marker_tx, marker_rx) = mpsc::sync_channel(1);
                thread::scope(|scope| {
                    let worker = WorkerScope::observed(&profile, &job, &cache, AtomicU64::new(0).into())
                        .with_test_exchange(Box::new(move |output, _input| {
                            let mut output = std::io::BufReader::new(output);
                            speculative::read_marker(&mut output, PROBE_READY).map_err(|e| e.to_string())?;
                            marker_tx.send(()).map_err(|e| e.to_string())?;
                            let mut tail = Vec::new();
                            output.take((CAP + 1) as u64).read_to_end(&mut tail).map_err(|e| e.to_string())?;
                            Ok(tail)
                        }));
                    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
                    let trigger = scope.spawn(move || {
                        marker_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                        // The test deliberately withholds ready from the scheduler.
                        let arrival = Instant::now();
                        cancel.cancel();
                        arrival
                    });
                    let Outcome::Exited(receipt) = execute_observed(worker, &exe, &args, Vec::new(), &control).unwrap() else { panic!("speculative exit unconfirmed") };
                    let at = trigger.join().unwrap();
                    assert_eq!(receipt.reason(), EndReason::Cancelled);
                    speculative_exit_us = receipt.signalled_at.saturating_duration_since(at).as_micros();
                    arrival = Some(at);
                });
                drop(job);
                cache.revoke_read_execute(&profile).unwrap();
                profile.delete().unwrap();
                speculative_recovery_us = arrival.unwrap().elapsed().as_micros();
            }
            let at = arrival.unwrap_or_else(Instant::now);
            // This launch occurs only after the speculative process and profile are recovered.
            let mut profile = Profile::new().unwrap();
            cache.allow_read_execute(&profile).unwrap();
            let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
            let control = RunControl { walltime: Some(Duration::from_secs(10)), ..RunControl::default() };
            let bytes = request(&text);
            let worker = WorkerScope::observed(&profile, &job, &cache, AtomicU64::new(0).into())
                .with_test_exchange(Box::new(move |output, mut input| {
                    let mut output = std::io::BufReader::new(output);
                    speculative::read_marker(&mut output, PROBE_READY).map_err(|e| e.to_string())?;
                    input.write_all(&bytes).map_err(|e| e.to_string())?;
                    drop(input);
                    let mut result = Vec::new();
                    output.take((CAP + 1) as u64).read_to_end(&mut result).map_err(|e| e.to_string())?;
                    Ok(result)
                }));
            let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
            let Outcome::Exited(receipt) = execute_observed(worker, &exe, &args, Vec::new(), &control).unwrap() else { panic!("fallback exit unconfirmed") };
            assert_eq!(receipt.reason(), EndReason::Exited);
            assert_eq!(receipt.exit_code(), Some(0));
            let confirmed_us = receipt.signalled_at.saturating_duration_since(at).as_micros();
            let (_, result) = probe_frame(receipt.output().unwrap());
            let value: serde_json::Value = serde_json::from_slice(result).unwrap();
            assert_eq!(value["ok"], true);
            assert_eq!(value["data"]["out"]["packet"]["root"]["text"], text);
            drop(job);
            cache.revoke_read_execute(&profile).unwrap();
            profile.delete().unwrap();
            println!("raw-controlled-fallback pair={pair} miss={miss} speculative_exit_us={speculative_exit_us} speculative_recovery_us={speculative_recovery_us} confirmed_us={confirmed_us} recovered_us={}", at.elapsed().as_micros());
        }
    }
    for path in [mode, hash, module, dll, exe] { fs::remove_file(path).unwrap(); }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}