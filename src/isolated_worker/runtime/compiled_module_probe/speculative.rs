//! Test-only transaction for a closed, fixed official graph.
use super::{
    CAP, CHILD, Cache, EndReason, GRAPH_CASES, Job, MARKER, Outcome, PROBE_READY, Profile,
    RunControl, WorkerScope, digest, execute_observed, fs, invoke, job_usage, probe_frame,
    romaji_request,
};
use crate::execution_control::CancellationToken;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, BufReader, Read, Write},
    path::Path,
    sync::{Mutex, atomic::AtomicU64, mpsc},
    thread,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(10);
type Response = (Instant, Vec<u8>);
type Receipt = (Instant, Option<Vec<u8>>);

pub(super) fn read_marker(reader: &mut impl Read, marker: &[u8]) -> io::Result<()> {
    let mut prefix = Vec::new();
    while prefix.len() < 4096 {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        prefix.push(byte[0]);
        if prefix.ends_with(marker) {
            return Ok(());
        }
    }
    Err(io::Error::other("probe marker missing"))
}

fn operation(index: usize, packet: Value, dictionary: &Value) -> Vec<u8> {
    let (kind, config) = match index {
        1 => ("text.dictionary-exact", json!({"path":"dictionary.tsv"})),
        2 => ("text.select-candidates", json!({"unscored":0.0})),
        3 => ("text.stringify", json!({})),
        _ => unreachable!(),
    };
    let mut operation =
        json!({"method":"execute","config":config,"inputs":{"in":packet},"mode":"compose"});
    if index == 1 {
        operation["resources"] = dictionary.clone();
    }
    serde_json::to_vec(&json!({
        "apiVersion":1,"node":{"kind":kind,"version":1},"operation":operation
    }))
    .unwrap()
}

fn validated_output(bytes: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let out = &value["data"]["out"];
    if value["ok"] != true || out["state"] != "data" {
        return None;
    }
    let packet: crate::Packet = serde_json::from_value(out["packet"].clone()).ok()?;
    packet.validate().ok()?;
    Some(out.clone())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    Normal,
    EarlyCrash,
    Malformed,
    Missing,
    LateCrash,
    Deadline,
    Cancel,
    Revision,
    Protection,
}

fn run(
    cache: &Cache,
    exe: &Path,
    dictionary: &Value,
    (input, expected): (&str, &str),
    speculate: bool,
    scenario: Scenario,
    planned: Option<(Instant, u64)>,
) {
    let acl = Mutex::new(());
    let cancel = CancellationToken::default();
    let prepare = Instant::now();
    thread::scope(|scope| {
        let lanes: Vec<_> = (0..4)
            .map(|_| {
                let (ready_send, ready) = mpsc::sync_channel(1);
                let (send, request) = mpsc::sync_channel::<Vec<u8>>(1);
                let (respond, response) = mpsc::sync_channel::<Response>(1);
                let (finish, finished) = mpsc::sync_channel::<Receipt>(1);
                let cancel = cancel.clone();
                let acl = &acl;
                let worker = scope.spawn(move || {
                    let mut profile = Profile::new().unwrap();
                    {
                        let _guard = acl.lock().unwrap();
                        cache.allow_read_execute(&profile).unwrap();
                    }
                    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
                    let control = RunControl {
                        cancel,
                        walltime: Some(TIMEOUT),
                        ..RunControl::default()
                    };
                    let worker =
                        WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into())
                            .with_test_exchange(Box::new(move |output, mut input| {
                                let exchange = || -> io::Result<Vec<u8>> {
                                    let mut output = BufReader::new(output);
                                    read_marker(&mut output, PROBE_READY)?;
                                    ready_send.send(()).map_err(io::Error::other)?;
                                    let request =
                                        request.recv_timeout(TIMEOUT).map_err(io::Error::other)?;
                                    assert!(request.len() <= CAP);
                                    input.write_all(&request)?;
                                    drop(input);
                                    read_marker(&mut output, MARKER)?;
                                    let mut header = [0; 12];
                                    output.read_exact(&mut header)?;
                                    let length = u32::from_le_bytes(header[8..].try_into().unwrap())
                                        as usize;
                                    if !(1..=CAP).contains(&length) {
                                        return Err(io::Error::other("invalid result size"));
                                    }
                                    let mut payload = vec![0; length];
                                    output.read_exact(&mut payload)?;
                                    let _ = respond.send((Instant::now(), payload.clone()));
                                    let mut bytes = MARKER.to_vec();
                                    bytes.extend_from_slice(&header);
                                    bytes.extend(payload);
                                    output.take(4097).read_to_end(&mut bytes)?;
                                    Ok(bytes)
                                };
                                exchange().map_err(|e| e.to_string())
                            }));
                    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
                    let Outcome::Exited(receipt) =
                        execute_observed(worker, exe, &args, Vec::new(), &control).unwrap()
                    else {
                        panic!("launched probe must return an OS exit receipt");
                    };
                    let success =
                        receipt.reason() == EndReason::Exited && receipt.exit_code() == Some(0);
                    let output = if success {
                        receipt
                            .output()
                            .ok()
                            .map(|bytes| probe_frame(bytes).1.to_vec())
                    } else {
                        None
                    };
                    finish.send((receipt.signalled_at, output)).unwrap();
                    let usage = job_usage(&job);
                    drop(job);
                    let _guard = acl.lock().unwrap();
                    cache.revoke_read_execute(&profile).unwrap();
                    profile.delete().unwrap();
                    usage
                });
                (ready, Some(send), response, finished, worker)
            })
            .collect();
        let mut lanes = lanes;
        for (ready, _, _, _, _) in &lanes {
            ready.recv_timeout(TIMEOUT).unwrap();
        }
        let prepared_us = prepare.elapsed().as_micros();
        if let Some((scheduled, _)) = planned {
            thread::sleep(scheduled.saturating_duration_since(Instant::now()));
        }
        let start = Instant::now();
        let since = |stamp: Instant| stamp.saturating_duration_since(start).as_micros();
        let (queued_us, interval_ms) = planned.map_or((0, 0), |(scheduled, interval)| {
            (
                start.saturating_duration_since(scheduled).as_micros(),
                interval,
            )
        });
        let mut request = romaji_request(input);
        let mut result = None;
        let mut provisional = Vec::new();
        let mut exit_times = [None; 4];
        let mut stages = Vec::new();
        for (index, (_, send, response, finished, _)) in lanes.iter_mut().enumerate() {
            let fault = match (scenario, index) {
                (Scenario::EarlyCrash, 0) | (Scenario::LateCrash, 1) => 1,
                (Scenario::Malformed, 0) => 2,
                (Scenario::Missing, 0) => 4,
                (Scenario::Deadline, 1) => 3,
                _ => 0,
            };
            let sent = since(Instant::now());
            send.take()
                .unwrap()
                .send([vec![fault], request].concat())
                .unwrap();
            let wait = if scenario == Scenario::Deadline && index == 1 {
                Duration::from_millis(150)
            } else {
                TIMEOUT
            };
            let Ok((stamp, bytes)) = response.recv_timeout(wait) else {
                break;
            };
            stages.push((sent, since(stamp)));
            let Some(out) = validated_output(&bytes) else {
                break;
            };
            if !speculate {
                let (stamp, verified) = finished.recv_timeout(TIMEOUT).unwrap();
                exit_times[index] = Some(since(stamp));
                if verified.as_deref() != Some(&bytes) {
                    break;
                }
            }
            provisional.push(bytes);
            if index == 1
                && matches!(
                    scenario,
                    Scenario::Cancel | Scenario::Revision | Scenario::Protection
                )
            {
                break;
            }
            if index == 3 {
                result = Some(out);
                break;
            }
            request = operation(index + 1, out, dictionary);
        }
        if result.is_none() {
            cancel.cancel();
            for (_, send, _, _, _) in &mut lanes {
                send.take();
            }
        }
        for (index, (_, _, _, finished, _)) in lanes.iter().enumerate() {
            if exit_times[index].is_none() {
                let (stamp, verified) = finished.recv_timeout(TIMEOUT).unwrap();
                exit_times[index] = Some(since(stamp));
                if verified.as_ref() != provisional.get(index) || verified.is_none() {
                    result = None;
                    cancel.cancel();
                }
            }
        }
        let published_us = result.as_ref().map(|value| {
            assert_eq!(value["packet"]["root"]["text"], expected);
            since(Instant::now())
        });
        assert_eq!(published_us.is_some(), scenario == Scenario::Normal);
        let jobs: Vec<_> = lanes
            .into_iter()
            .map(|(_, _, _, _, join)| join.join().unwrap())
            .collect();
        println!(
            "speculative={speculate} case={scenario:?} interval_ms={interval_ms} queued_us={queued_us} prepare_us={prepared_us} stages={stages:?} exits={exit_times:?} publish_us={published_us:?} recovered_us={} jobs={jobs:?}",
            since(Instant::now())
        );
    });
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainers"]
fn compare_speculation() {
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
    let staged = [
        cache.write("prepared.cwasm", &artifact).unwrap(),
        cache
            .write("artifact.sha256", digest(&artifact).as_bytes())
            .unwrap(),
        cache.write("mode.txt", b"ready-speculative-guest").unwrap(),
    ];
    let seed = include_str!("../../../../distribution/defaults/dictionary.tsv");
    let dictionary = json!({"dictionary.tsv":seed.as_bytes()});
    for (index, case) in GRAPH_CASES.into_iter().cycle().take(12).enumerate() {
        let first = index % 2 != 0;
        for speculative in [first, !first] {
            run(
                &cache,
                &exe,
                &dictionary,
                case,
                speculative,
                Scenario::Normal,
                None,
            );
        }
    }
    for scenario in [
        Scenario::EarlyCrash,
        Scenario::Malformed,
        Scenario::Missing,
        Scenario::LateCrash,
        Scenario::Deadline,
        Scenario::Cancel,
        Scenario::Revision,
        Scenario::Protection,
    ] {
        run(
            &cache,
            &exe,
            &dictionary,
            GRAPH_CASES[0],
            true,
            scenario,
            None,
        );
    }
    for interval in [20, 100] {
        for speculate in [false, true] {
            let first_input = Instant::now();
            for (index, case) in GRAPH_CASES.into_iter().cycle().take(6).enumerate() {
                run(
                    &cache,
                    &exe,
                    &dictionary,
                    case,
                    speculate,
                    Scenario::Normal,
                    Some((
                        first_input + Duration::from_millis(interval * index as u64),
                        interval,
                    )),
                );
            }
        }
    }
    for path in staged.into_iter().chain([exe, dll, wasm]) {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}
