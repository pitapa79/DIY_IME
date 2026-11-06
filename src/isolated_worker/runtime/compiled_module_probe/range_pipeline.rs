//! Fixed developer-only graph; no package or product routing.
use super::dictionary_range::{
    ABSENT, ASSET, Asset, READY, integer, lookup, page_runtime, search_wasm,
};
use super::{
    Api, Cache, EndReason, Job, Outcome, Profile, RunControl, WorkerScope, compiled_module, digest,
    execute_observed, fixed_operation, fs, invoke, invoke_fresh, job_usage, limits, romaji_request,
};
use crate::execution_control::CancellationToken;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, BufReader, Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::{Duration, Instant},
};

const CHILD: &str = "isolated_worker::runtime::compiled_module_probe::range_pipeline::child";
const ROWS: usize = 1_000_000;
const CASES: [(&str, &str, &str); 3] = [
    ("kousui", "こうすい", "降水"),
    ("kya", "きゃ", "きゃ"),
    ("ky", "", ""),
];

fn key(reading: &str) -> u64 {
    reading.bytes().fold(0u64, |hash, byte| {
        hash.wrapping_mul(257).wrapping_add(u64::from(byte))
    }) | (1 << 63)
}

fn candidates(kana: &str, hit: bool) -> Value {
    let options = if hit {
        vec![
            option("降水", Some(90.0)),
            option("香水", Some(30.0)),
            option("洪水", Some(90.0)),
        ]
    } else {
        vec![option(kana, None)]
    };
    json!({"state":"data","packet":{"version":1,"root":{"type":"choice","options":options}}})
}

fn option(text: &str, score: Option<f64>) -> Value {
    let mut value = json!({"value":{"type":"text","text":text}});
    if let Some(score) = score {
        value["score"] = json!(score);
    }
    value
}

#[test]
#[ignore = "child of the isolated fixed pipeline experiment"]
fn child() {
    super::require_appcontainer().unwrap();
    let api = Api::load(&std::env::current_dir().unwrap().join("wasmtime.dll")).unwrap();
    let artifact = fs::read("pipeline.cwasm").unwrap();
    let hash = fs::read_to_string("pipeline.sha256").unwrap();
    let limits = limits();
    let mut official = compiled_module::prepare_for_probe(&api, &limits, &artifact, &hash).unwrap();
    official.discard_store();
    let mut search = page_runtime(&api);
    search.discard_store();
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    output.write_all(READY).unwrap();
    output.flush().unwrap();
    let mut tag = [0];
    while input.read(&mut tag).unwrap() != 0 {
        let (reading, _, _) = CASES[tag[0] as usize];
        let raw = invoke_fresh(&mut official, &limits, &romaji_request(reading));
        let romaji: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(romaji["ok"], true);
        let kana = romaji["data"]["out"]["packet"]["root"]["text"]
            .as_str()
            .unwrap();
        search.install_store(&limits).unwrap();
        search.instantiate().unwrap();
        let found = lookup(ROWS, |page| {
            output.write_all(&[ASSET]).unwrap();
            output.write_all(&(page as u64).to_le_bytes()).unwrap();
            output.flush().unwrap();
            let len = integer(&mut input).unwrap() as usize;
            assert!((16..=4096).contains(&len) && len.is_multiple_of(16));
            let mut page = vec![0; len];
            input.read_exact(&mut page).unwrap();
            search_wasm(&mut search, key(kana), &page)
        });
        search.discard_store();
        assert!(found == 1 || found == ABSENT);
        let choice = candidates(kana, found == 1);
        let selected = fixed_operation(
            &mut official,
            &limits,
            "text.select-candidates",
            json!({"method":"execute","config":{"unscored":0.0},"inputs":{"in":choice},"mode":"compose"}),
        );
        let final_text = fixed_operation(
            &mut official,
            &limits,
            "text.stringify",
            json!({"method":"execute","config":{},"inputs":{"in":selected["out"]},"mode":"compose"}),
        );
        let reply = serde_json::to_vec(&json!([
            kana,
            choice,
            final_text["out"]["packet"]["root"]["text"]
        ]))
        .unwrap();
        output.write_all(&[0]).unwrap();
        output
            .write_all(&(reply.len() as u64).to_le_bytes())
            .unwrap();
        output.write_all(&reply).unwrap();
        output.flush().unwrap();
    }
}

#[derive(Clone, Copy)]
struct Sample {
    tag: u8,
    planned: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    None,
    Cancel,
    Supersede,
    SwitchDocument,
    Protect,
    RevokeDictionary,
}

impl Boundary {
    fn stops_worker(self) -> bool {
        matches!(
            self,
            Self::Cancel | Self::SwitchDocument | Self::Protect | Self::RevokeDictionary
        )
    }
}

#[derive(Default)]
struct Context {
    generation: u64,
    document: u64,
    protected: bool,
}

impl Context {
    fn accepts(&self, generation: u64, document: u64) -> bool {
        self.generation == generation && self.document == document && !self.protected
    }
}

#[derive(Default)]
struct Observations {
    ready_us: u128,
    rejected_replies: usize,
    denied_reads: usize,
    samples: Vec<(Instant, Instant, Instant, usize)>,
}

fn exchange(
    (output, mut input): (fs::File, fs::File),
    mut asset: Asset,
    samples: Vec<Sample>,
    cancel: CancellationToken,
    boundary: Boundary,
    observations: Arc<Mutex<Observations>>,
    launch: Instant,
) -> io::Result<Vec<u8>> {
    let mut output = BufReader::new(output).take(256 * 1024);
    let mut prefix = Vec::new();
    while !prefix.ends_with(READY) && prefix.len() < 4096 {
        let mut byte = [0];
        output.read_exact(&mut byte)?;
        prefix.push(byte[0]);
    }
    if !prefix.ends_with(READY) {
        return Err(io::Error::other("missing ready"));
    }
    observations.lock().unwrap().ready_us = launch.elapsed().as_micros();
    let mut context = Context::default();
    for (request, sample) in samples.into_iter().enumerate() {
        let (generation, document) = (context.generation, context.document);
        if let Some(planned) = sample.planned {
            std::thread::sleep(planned.saturating_duration_since(Instant::now()));
        }
        let sent = Instant::now();
        input.write_all(&[sample.tag])?;
        let mut reads = 0;
        let mut bytes = 0;
        loop {
            let mut id = [0];
            output.read_exact(&mut id)?;
            let index = integer(&mut output)?;
            if id[0] == 0 {
                if cancel.is_cancelled() || index > 4096 {
                    return Err(io::Error::other("result denied"));
                }
                let mut reply = vec![0; index as usize];
                output.read_exact(&mut reply)?;
                let received = Instant::now();
                let actual: Value = serde_json::from_slice(&reply)?;
                let (_, kana, text) = CASES[sample.tag as usize];
                assert_eq!(
                    actual,
                    json!([kana, candidates(kana, sample.tag == 0), text])
                );
                if request == 0 {
                    match boundary {
                        Boundary::Supersede => context.generation += 1,
                        Boundary::SwitchDocument => context.document += 1,
                        Boundary::Protect => context.protected = true,
                        _ => {}
                    }
                }
                if !context.accepts(generation, document) {
                    observations.lock().unwrap().rejected_replies += 1;
                    if boundary.stops_worker() {
                        cancel.cancel();
                        return Err(io::Error::other("context changed before publication"));
                    }
                    break;
                }
                observations.lock().unwrap().samples.push((
                    sample.planned.unwrap_or(sent),
                    sent,
                    received,
                    bytes,
                ));
                break;
            }
            if boundary == Boundary::Cancel
                || (boundary == Boundary::RevokeDictionary && reads == 1)
            {
                cancel.cancel();
                assert!(asset.page(id[0], index, reads, &cancel).is_err());
                observations.lock().unwrap().denied_reads += 1;
                return Err(io::Error::other("page access withdrawn"));
            }
            let page = asset.page(id[0], index, reads, &cancel)?;
            reads += 1;
            bytes += page.len();
            input.write_all(&(page.len() as u64).to_le_bytes())?;
            input.write_all(&page)?;
        }
    }
    drop(input);
    let mut tail = Vec::new();
    output.read_to_end(&mut tail)?;
    Ok(tail)
}

fn run(
    cache: &Cache,
    exe: &Path,
    file: &fs::File,
    samples: Vec<Sample>,
    interval: u64,
    retained: bool,
    boundary: Boundary,
) {
    let mut profile = Profile::new().unwrap();
    cache.allow_read_execute(&profile).unwrap();
    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
    let cancel = CancellationToken::default();
    let worker_cancel = cancel.clone();
    let observations = Arc::new(Mutex::new(Observations::default()));
    let worker_observations = observations.clone();
    let asset = Asset {
        file: file.try_clone().unwrap(),
        rows: ROWS,
    };
    let launched = Instant::now();
    let count = samples.len();
    let scope = WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into())
        .with_test_exchange(Box::new(move |output, input| {
            exchange(
                (output, input),
                asset,
                samples,
                worker_cancel,
                boundary,
                worker_observations,
                launched,
            )
            .map_err(|error| error.to_string())
        }));
    let args = ["--exact", CHILD, "--ignored", "--nocapture"].map(OsString::from);
    let control = RunControl {
        cancel,
        walltime: Some(Duration::from_secs(30)),
        ..RunControl::default()
    };
    let ended = {
        let Outcome::Exited(receipt) =
            execute_observed(scope, exe, &args, Vec::new(), &control).unwrap()
        else {
            panic!("missing physical exit receipt")
        };
        if boundary.stops_worker() {
            assert!(receipt.output().is_err());
        } else {
            assert_eq!(receipt.reason(), EndReason::Exited);
            assert_eq!(receipt.exit_code(), Some(0));
            assert!(receipt.output().is_ok());
        }
        receipt.signalled_at
    };
    let (cpu, peak) = job_usage(&job);
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
    let measured = observations.lock().unwrap();
    let expected = match boundary {
        Boundary::None => count,
        Boundary::Supersede => count - 1,
        _ => 0,
    };
    assert_eq!(measured.samples.len(), expected);
    assert_eq!(
        measured.rejected_replies,
        usize::from(matches!(
            boundary,
            Boundary::Supersede | Boundary::SwitchDocument | Boundary::Protect
        ))
    );
    assert_eq!(
        measured.denied_reads,
        usize::from(matches!(
            boundary,
            Boundary::Cancel | Boundary::RevokeDictionary
        ))
    );
    println!(
        "pipeline-boundary {boundary:?} accepted={} rejected={} denied_reads={}",
        measured.samples.len(),
        measured.rejected_replies,
        measured.denied_reads,
    );
    let withdraw = boundary.stops_worker();
    println!(
        "pipeline-process retained={retained} interval={interval} ready_us={} cpu_100ns={cpu} peak_bytes={peak} withdrawn={withdraw}",
        measured.ready_us
    );
    for (planned, sent, received, bytes) in &measured.samples {
        let applicable = if retained {
            *received
        } else {
            ended.max(*received)
        };
        println!(
            "pipeline-sample retained={retained} interval={interval} queued_us={} reply_us={} applicable_us={} bytes={bytes}",
            sent.saturating_duration_since(*planned).as_micros(),
            received.duration_since(*sent).as_micros(),
            applicable.duration_since(*sent).as_micros()
        );
    }
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainers"]
fn range_pipeline_cost() {
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
    let compiled = cache.write("pipeline.cwasm", &artifact).unwrap();
    let hash = cache
        .write("pipeline.sha256", digest(&artifact).as_bytes())
        .unwrap();
    let mut bytes = Vec::with_capacity(ROWS * 16);
    for index in 0..ROWS as u64 - 1 {
        bytes.extend_from_slice(&index.to_le_bytes());
        bytes.extend_from_slice(&(index + 2).to_le_bytes());
    }
    bytes.extend_from_slice(&key("こうすい").to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    let asset = cache.private_canary(&bytes).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
        .open(&asset)
        .unwrap();
    let mut check = Vec::new();
    file.try_clone().unwrap().read_to_end(&mut check).unwrap();
    assert_eq!(digest(&check), digest(&bytes));
    drop(check);
    drop(bytes);
    for interval in [0, 20, 100] {
        let start = Instant::now();
        for index in 0..12 {
            let planned = if interval == 0 {
                Instant::now()
            } else {
                start + Duration::from_millis(interval * index)
            };
            run(
                &cache,
                &exe,
                &file,
                vec![Sample {
                    tag: (index % 3) as u8,
                    planned: Some(planned),
                }],
                interval,
                false,
                Boundary::None,
            );
        }
        let start = Instant::now() + Duration::from_secs(1);
        let samples = (0..60)
            .map(|index| Sample {
                tag: (index % 3) as u8,
                planned: if interval == 0 && index > 0 {
                    None
                } else {
                    Some(start + Duration::from_millis(interval * index))
                },
            })
            .collect();
        run(&cache, &exe, &file, samples, interval, true, Boundary::None);
    }
    for retained in [false, true] {
        run(
            &cache,
            &exe,
            &file,
            vec![Sample {
                tag: 0,
                planned: Some(Instant::now()),
            }],
            0,
            retained,
            Boundary::Cancel,
        );
    }
    for _ in 0..5 {
        for boundary in [
            Boundary::Supersede,
            Boundary::SwitchDocument,
            Boundary::Protect,
            Boundary::RevokeDictionary,
        ] {
            let tags: &[u8] = if boundary == Boundary::Supersede {
                &[0, 1]
            } else {
                &[0]
            };
            let samples = tags
                .iter()
                .map(|&tag| Sample { tag, planned: None })
                .collect();
            run(&cache, &exe, &file, samples, 0, true, boundary);
        }
    }
    drop(file);
    for path in [asset, hash, compiled, wasm, dll, exe] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}
