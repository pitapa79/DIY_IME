//! Fixed developer-only graph; no package or product routing.
use super::dictionary_range::{
    ABSENT, ASSET, Asset, READY, integer, lookup, page_runtime, search_wasm, verify_private_asset,
};
use super::{
    Api, Cache, EndReason, Job, Outcome, Profile, RunControl, WorkerScope, compiled_module, digest,
    execute_observed, fixed_operation, fs, invoke, invoke_fresh, job_usage, limits, romaji_request,
};
use crate::execution_control::CancellationToken;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::{Duration, Instant},
};

const CHILD: &str = "isolated_worker::runtime::compiled_module_probe::range_pipeline::child";
const ROWS: usize = 1_000_000;
const CASES: [(&str, &str, &str); 4] = [
    ("kousui", "こうすい", "降水"),
    ("nihonn", "にほん", "日本"),
    ("kya", "きゃ", "きゃ"),
    ("ky", "", ""),
];
const PAYLOAD: u8 = ASSET + 1;
const PAYLOAD_PAGE: usize = 4096;
const GROUPS: u64 = 2;
const BAD_PAGE: u8 = 254;
const CRASH_AFTER_REPLY: u8 = 253;
const CHECKED_REPLY: u8 = 19;
const FAULT_EXIT: u32 = 73;

fn key(reading: &str) -> u64 {
    reading.bytes().fold(0u64, |hash, byte| {
        hash.wrapping_mul(257).wrapping_add(u64::from(byte))
    }) | (1 << 63)
}

fn choice(options: Vec<Value>) -> Value {
    json!({"state":"data","packet":{"version":1,"root":{"type":"choice","options":options}}})
}

fn expected_choice(tag: usize) -> Value {
    let (_, kana, _) = CASES[tag];
    let options = match tag {
        0 => vec![
            option("降水", Some(90.0)),
            option("香水", Some(30.0)),
            option("洪水", Some(90.0)),
        ],
        1 => vec![option("日本", Some(80.0)), option("二本", Some(55.0))],
        _ => vec![option(kana, None)],
    };
    choice(options)
}

fn request_page(input: &mut impl Read, output: &mut impl Write, asset: u8, page: u64) -> Vec<u8> {
    output.write_all(&[asset]).unwrap();
    output.write_all(&page.to_le_bytes()).unwrap();
    output.flush().unwrap();
    let len = integer(input).unwrap() as usize;
    assert!((1..=PAYLOAD_PAGE).contains(&len));
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes).unwrap();
    bytes
}

fn candidate_page(
    file: &mut fs::File,
    page: u64,
    reads: usize,
    cancel: &CancellationToken,
) -> io::Result<Vec<u8>> {
    if page >= GROUPS || reads >= 16 || cancel.is_cancelled() {
        return Err(io::Error::other("candidate page denied"));
    }
    file.seek(SeekFrom::Start(page * PAYLOAD_PAGE as u64))?;
    let mut bytes = vec![0; PAYLOAD_PAGE];
    file.read_exact(&mut bytes)?;
    if cancel.is_cancelled() {
        return Err(io::Error::other("candidate page withdrawn"));
    }
    Ok(bytes)
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
        let fault = tag[0] == BAD_PAGE;
        let crash = tag[0] == CRASH_AFTER_REPLY;
        let (reading, _, _) = CASES[if fault || crash { 0 } else { tag[0] as usize }];
        let raw = invoke_fresh(&mut official, &limits, &romaji_request(reading));
        let romaji: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(romaji["ok"], true);
        let kana = romaji["data"]["out"]["packet"]["root"]["text"]
            .as_str()
            .unwrap();
        search.install_store(&limits).unwrap();
        search.instantiate().unwrap();
        let found = lookup(ROWS, |page| {
            let page = request_page(&mut input, &mut output, ASSET, page as u64);
            assert!(page.len().is_multiple_of(16));
            search_wasm(&mut search, key(kana), &page)
        });
        search.discard_store();
        let choice = if found == ABSENT {
            choice(vec![option(kana, None)])
        } else {
            let page = if fault { GROUPS } else { found };
            let bytes = request_page(&mut input, &mut output, PAYLOAD, page);
            serde_json::from_slice(bytes.trim_ascii_end()).unwrap()
        };
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
        if crash {
            let mut acknowledgement = [0];
            input.read_exact(&mut acknowledgement).unwrap();
            assert_eq!(acknowledgement, [CHECKED_REPLY]);
            std::process::exit(FAULT_EXIT as i32);
        }
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
    InvalidPayload,
    CrashAfterReply,
}

impl Boundary {
    fn stops_worker(self) -> bool {
        matches!(
            self,
            Self::Cancel
                | Self::SwitchDocument
                | Self::Protect
                | Self::RevokeDictionary
                | Self::InvalidPayload
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
    samples: Vec<(Instant, Instant, Instant, usize, usize)>,
}

fn exchange(
    (output, mut input): (fs::File, fs::File),
    (mut asset, mut payload): (Asset, fs::File),
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
                let tag = if sample.tag == CRASH_AFTER_REPLY {
                    0
                } else {
                    sample.tag as usize
                };
                let (_, kana, text) = CASES[tag];
                assert_eq!(actual, json!([kana, expected_choice(tag), text]));
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
                    reads,
                ));
                if boundary == Boundary::CrashAfterReply {
                    input.write_all(&[CHECKED_REPLY])?;
                    input.flush()?;
                }
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
            let page = if id[0] == PAYLOAD {
                match candidate_page(&mut payload, index, reads, &cancel) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        cancel.cancel();
                        observations.lock().unwrap().denied_reads += 1;
                        return Err(error);
                    }
                }
            } else {
                asset.page(id[0], index, reads, &cancel)?
            };
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
    files: (&fs::File, &fs::File),
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
        file: files.0.try_clone().unwrap(),
        rows: ROWS,
    };
    let payload = files.1.try_clone().unwrap();
    let launched = Instant::now();
    let count = samples.len();
    let scope = WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into())
        .with_test_exchange(Box::new(move |output, input| {
            exchange(
                (output, input),
                (asset, payload),
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
    let (ended, normal_exit) = {
        let Outcome::Exited(receipt) =
            execute_observed(scope, exe, &args, Vec::new(), &control).unwrap()
        else {
            panic!("missing physical exit receipt")
        };
        let normal_exit = receipt.reason() == EndReason::Exited
            && receipt.exit_code() == Some(0)
            && receipt.output().is_ok()
            && receipt.input_delivered();
        if boundary.stops_worker() {
            assert!(receipt.output().is_err());
        } else if boundary == Boundary::CrashAfterReply {
            assert_eq!(receipt.reason(), EndReason::Exited);
            assert_eq!(receipt.exit_code(), Some(FAULT_EXIT));
            assert!(receipt.output().is_ok());
            assert!(receipt.input_delivered());
        } else {
            assert!(normal_exit);
        }
        (receipt.signalled_at, normal_exit)
    };
    let (cpu, peak) = job_usage(&job);
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
    let measured = observations.lock().unwrap();
    let expected = match boundary {
        Boundary::None | Boundary::CrashAfterReply => count,
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
            Boundary::Cancel | Boundary::RevokeDictionary | Boundary::InvalidPayload
        ))
    );
    println!(
        "pipeline-boundary {boundary:?} accepted={} rejected={} denied_reads={}",
        measured.samples.len(),
        measured.rejected_replies,
        measured.denied_reads,
    );
    if boundary == Boundary::CrashAfterReply {
        let [(_, sent, received, _, _)] = measured.samples.as_slice() else {
            panic!("missing verified reply");
        };
        assert!(ended > *received);
        assert!(!normal_exit);
        println!(
            "reply-exit-boundary retained={retained} correct_reply=1 normal_exit=0 reply_us={} reply_to_exit_us={} exit_code={FAULT_EXIT}",
            received.duration_since(*sent).as_micros(),
            ended.duration_since(*received).as_micros(),
        );
        return;
    }
    let withdraw = boundary.stops_worker();
    println!(
        "pipeline-process retained={retained} interval={interval} ready_us={} cpu_100ns={cpu} peak_bytes={peak} withdrawn={withdraw}",
        measured.ready_us
    );
    for (planned, sent, received, bytes, reads) in &measured.samples {
        let applicable = if retained {
            *received
        } else {
            ended.max(*received)
        };
        println!(
            "pipeline-sample retained={retained} interval={interval} queued_us={} reply_us={} applicable_us={} bytes={bytes} reads={reads}",
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
    for index in 0..ROWS as u64 - GROUPS {
        bytes.extend_from_slice(&index.to_le_bytes());
        bytes.extend_from_slice(&(index + 2).to_le_bytes());
    }
    let mut entries = [(key(CASES[0].1), 0u64), (key(CASES[1].1), 1u64)];
    entries.sort_unstable();
    assert!(entries[0].0 < entries[1].0);
    for (reading, group) in entries {
        bytes.extend_from_slice(&reading.to_le_bytes());
        bytes.extend_from_slice(&group.to_le_bytes());
    }
    let mut payload_bytes = Vec::new();
    for index in 0..GROUPS as usize {
        let group = serde_json::to_vec(&expected_choice(index)).unwrap();
        assert!(group.len() <= PAYLOAD_PAGE);
        payload_bytes.extend(group);
        payload_bytes.resize((index + 1) * PAYLOAD_PAGE, b' ');
    }
    let mut payload_naming = Profile::new().unwrap();
    let payload_cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &payload_naming).unwrap();
    let payload_path = payload_cache.private_canary(&payload_bytes).unwrap();
    verify_private_asset(&cache, &exe, &payload_path);
    let payload = fs::OpenOptions::new()
        .read(true)
        .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
        .open(&payload_path)
        .unwrap();
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
                (&file, &payload),
                vec![Sample {
                    tag: (index % CASES.len() as u64) as u8,
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
                tag: (index % CASES.len() as u64) as u8,
                planned: if interval == 0 && index > 0 {
                    None
                } else {
                    Some(start + Duration::from_millis(interval * index))
                },
            })
            .collect();
        run(
            &cache,
            &exe,
            (&file, &payload),
            samples,
            interval,
            true,
            Boundary::None,
        );
    }
    for retained in [false, true] {
        run(
            &cache,
            &exe,
            (&file, &payload),
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
            Boundary::InvalidPayload,
        ] {
            let tags: &[u8] = match boundary {
                Boundary::Supersede => &[0, 1],
                Boundary::InvalidPayload => &[BAD_PAGE],
                _ => &[0],
            };
            let samples = tags
                .iter()
                .map(|&tag| Sample { tag, planned: None })
                .collect();
            run(&cache, &exe, (&file, &payload), samples, 0, true, boundary);
        }
    }
    for retained in [false, true] {
        for _ in 0..5 {
            run(
                &cache,
                &exe,
                (&file, &payload),
                vec![Sample {
                    tag: CRASH_AFTER_REPLY,
                    planned: None,
                }],
                0,
                retained,
                Boundary::CrashAfterReply,
            );
        }
    }
    drop(file);
    drop(payload);
    for path in [asset, payload_path, hash, compiled, wasm, dll, exe] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
    fs::remove_dir(payload_cache.path()).unwrap();
    payload_naming.delete().unwrap();
}
