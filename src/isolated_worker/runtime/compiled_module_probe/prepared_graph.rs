//! Best case for a fixed graph: all four one-use children ready before input.
use super::{
    CAP, CHILD, Cache, EndReason, GRAPH_CASES, Job, Outcome, PROBE_READY, Profile, RunControl,
    WorkerScope, digest, execute_observed, fs, invoke, job_usage, probe_frame, romaji_request,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{BufReader, Read, Write},
    path::Path,
    sync::{Mutex, atomic::AtomicU64, mpsc},
    thread,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(30);

fn run_graph(cache: &Cache, exe: &Path, dictionary: &Value, input: &str, expected: &str) {
    let acl = Mutex::new(());
    let prepare = Instant::now();
    thread::scope(|scope| {
        let lanes: Vec<_> = (0..4)
            .map(|_| {
                let (ready_send, ready) = mpsc::sync_channel(1);
                let (send, request) = mpsc::sync_channel::<Vec<u8>>(1);
                let (reply, receive) = mpsc::sync_channel(1);
                let acl = &acl;
                let worker = scope.spawn(move || {
                    let mut profile = Profile::new().unwrap();
                    {
                        let _guard = acl.lock().unwrap();
                        cache.allow_read_execute(&profile).unwrap();
                    }
                    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
                    let control = RunControl {
                        walltime: Some(TIMEOUT),
                        ..RunControl::default()
                    };
                    let worker =
                        WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into())
                            .with_test_exchange(Box::new(move |output, mut input| {
                                let exchange = || -> std::io::Result<Vec<u8>> {
                                    let mut output = BufReader::new(output);
                                    let mut prefix = Vec::new();
                                    while prefix.len() < 4096 && !prefix.ends_with(PROBE_READY) {
                                        let mut byte = [0];
                                        output.read_exact(&mut byte)?;
                                        prefix.push(byte[0]);
                                    }
                                    if !prefix.ends_with(PROBE_READY) {
                                        return Err(std::io::Error::other(
                                            "invalid readiness frame",
                                        ));
                                    }
                                    ready_send.send(()).map_err(std::io::Error::other)?;
                                    let request = request
                                        .recv_timeout(TIMEOUT)
                                        .map_err(std::io::Error::other)?;
                                    assert!(request.len() <= CAP);
                                    input.write_all(&request)?;
                                    drop(input);
                                    let mut bytes = Vec::new();
                                    output.take((CAP + 1) as u64).read_to_end(&mut bytes)?;
                                    assert!(bytes.len() <= CAP);
                                    Ok(bytes)
                                };
                                exchange().map_err(|e| e.to_string())
                            }));
                    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
                    {
                        let Outcome::Exited(receipt) =
                            execute_observed(worker, exe, &args, Vec::new(), &control).unwrap()
                        else {
                            panic!("graph worker did not verify physical exit");
                        };
                        assert_eq!(receipt.reason(), EndReason::Exited);
                        assert_eq!(receipt.exit_code(), Some(0));
                        let (_, bytes) = probe_frame(receipt.output().unwrap());
                        reply.send(bytes.to_vec()).unwrap();
                    }
                    let usage = job_usage(&job);
                    drop(job);
                    let _guard = acl.lock().unwrap();
                    cache.revoke_read_execute(&profile).unwrap();
                    profile.delete().unwrap();
                    usage
                });
                (ready, send, receive, worker)
            })
            .collect();
        for (ready, _, _, _) in &lanes {
            ready.recv_timeout(TIMEOUT).unwrap();
        }
        let prepare_us = prepare.elapsed().as_micros();
        let start = Instant::now();
        let call = |index: usize, request| {
            let (_, send, receive, _) = &lanes[index];
            send.send(request).unwrap();
            let bytes = receive.recv_timeout(TIMEOUT).unwrap();
            let result: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(result["ok"], true);
            result["data"]["out"].clone()
        };
        let mut packet = call(0, romaji_request(input));
        for (index, (kind, config)) in [
            ("text.dictionary-exact", json!({"path":"dictionary.tsv"})),
            ("text.select-candidates", json!({"unscored":0.0})),
            ("text.stringify", json!({})),
        ]
        .into_iter()
        .enumerate()
        {
            let mut operation =
                json!({"method":"execute","config":config,"inputs":{"in":packet},"mode":"compose"});
            if index == 0 {
                operation["resources"] = dictionary.clone();
            }
            let request =
                json!({"apiVersion":1,"node":{"kind":kind,"version":1},"operation":operation});
            packet = call(index + 1, serde_json::to_vec(&request).unwrap());
        }
        assert_eq!(packet["packet"]["root"]["text"], expected);
        let graph_us = start.elapsed().as_micros();
        let usage: Vec<_> = lanes
            .into_iter()
            .map(|(_, _, _, worker)| worker.join().unwrap())
            .collect();
        println!(
            "ready-graph prepare_us={prepare_us} graph_us={graph_us} recovered_us={} jobs={usage:?}",
            start.elapsed().as_micros()
        );
    });
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainers"]
fn prepared_graph_cost() {
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
        cache.write("mode.txt", b"ready-module-guest").unwrap(),
    ];
    let seed = include_str!("../../../../distribution/defaults/dictionary.tsv");
    let dictionary = json!({"dictionary.tsv":seed.as_bytes()});
    for (input, expected) in GRAPH_CASES.into_iter().cycle().take(12) {
        run_graph(&cache, &exe, &dictionary, input, expected);
    }
    for path in staged.into_iter().chain([exe, dll, wasm]) {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}
