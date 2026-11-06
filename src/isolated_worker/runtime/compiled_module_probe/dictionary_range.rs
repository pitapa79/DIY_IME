//! Developer experiment. The sandbox asks for pages, and owns all search semantics.
use super::{
    Api, Cache, EndReason, Job, Outcome, Profile, RunControl, Runtime, WorkerScope, digest,
    execute_observed, fs, job_usage, limits,
};
use crate::execution_control::CancellationToken;
use std::{
    ffi::OsString,
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::{Duration, Instant},
};

const CHILD: &str = "isolated_worker::runtime::compiled_module_probe::dictionary_range::child";
const WASM_CHILD: &str =
    "isolated_worker::runtime::compiled_module_probe::dictionary_range::child_wasm";
const FILE_CANARY: &str =
    "isolated_worker::runtime::compiled_module_probe::dictionary_range::child_file_canary";
const READY: &[u8] = b"\nDIY-RANGE-READY\0";
const RECORD: usize = 16;
const PAGE: usize = 4096;
const ROWS_PER_PAGE: usize = PAGE / RECORD;
const MAX_READS: usize = 16;
const ASSET: u8 = 7;
const ABSENT: u64 = u64::MAX;

fn integer(input: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn value(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap())
}

enum Search {
    Before,
    After,
    Found(u64),
    Missing,
}

fn lookup(rows: usize, mut search: impl FnMut(usize) -> Search) -> u64 {
    let mut low = 0;
    let mut high = rows.div_ceil(ROWS_PER_PAGE);
    while low < high {
        let mid = low + (high - low) / 2;
        match search(mid) {
            Search::Before => high = mid,
            Search::After => low = mid + 1,
            Search::Found(value) => return value,
            Search::Missing => break,
        }
    }
    ABSENT
}

fn search_native(key: u64, page: &[u8]) -> Search {
    let keys = |index| value(&page[index * RECORD..index * RECORD + 8]);
    let count = page.len() / RECORD;
    if key < keys(0) {
        return Search::Before;
    }
    if key > keys(count - 1) {
        return Search::After;
    }
    let mut left = 0;
    let mut right = count;
    while left < right {
        let middle = left + (right - left) / 2;
        match keys(middle).cmp(&key) {
            std::cmp::Ordering::Less => left = middle + 1,
            std::cmp::Ordering::Greater => right = middle,
            std::cmp::Ordering::Equal => {
                return Search::Found(value(&page[middle * RECORD + 8..(middle + 1) * RECORD]));
            }
        }
    }
    Search::Missing
}

#[test]
#[ignore = "child of the fixed AppContainer range probe"]
fn child() {
    super::require_appcontainer().unwrap();
    child_lookup(search_native);
}

#[test]
#[ignore = "child of the fixed AppContainer asset-access canary"]
fn child_file_canary() {
    super::require_appcontainer().unwrap();
    let mut path = String::new();
    io::stdin().take(4096).read_to_string(&mut path).unwrap();
    assert!(!path.is_empty());
    assert!(fs::File::open(path).is_err());
}

fn verify_private_asset(cache: &Cache, exe: &Path, path: &Path) {
    let mut profile = Profile::new().unwrap();
    cache.allow_read_execute(&profile).unwrap();
    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
    let args = ["--exact", FILE_CANARY, "--ignored", "--nocapture"].map(OsString::from);
    {
        let scope = WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into());
        let control = RunControl {
            walltime: Some(Duration::from_secs(10)),
            ..RunControl::default()
        };
        let path = path.to_str().unwrap().as_bytes().to_vec();
        let Outcome::Exited(receipt) = execute_observed(scope, exe, &args, path, &control).unwrap()
        else {
            panic!("asset canary exit not verified")
        };
        assert_eq!(receipt.reason(), EndReason::Exited);
        assert_eq!(receipt.exit_code(), Some(0));
        assert!(receipt.input_delivered());
    }
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
}

#[test]
#[ignore = "Wasm child of the fixed AppContainer range probe"]
fn child_wasm() {
    use super::super::Bytes;
    use std::ptr;
    super::require_appcontainer().unwrap();
    let dll = std::env::current_dir().unwrap().join("wasmtime.dll");
    let api = Api::load(&dll).unwrap();
    let wat = include_bytes!("range_lookup.wat");
    let module = {
        let mut bytes = Bytes {
            size: 0,
            data: ptr::null_mut(),
        };
        // SAFETY: the pinned Wasmtime API owns the returned vector.
        unsafe {
            api.check(
                (api.wat)(wat.as_ptr(), wat.len(), &mut bytes),
                ptr::null_mut(),
            )
            .unwrap();
            assert!(!bytes.data.is_null() && (1..=64 * 1024).contains(&bytes.size));
            let module = std::slice::from_raw_parts(bytes.data, bytes.size).to_vec();
            (api.bytes_delete)(&mut bytes);
            module
        }
    };
    let mut runtime = Runtime::new(&api, &module, &limits(), None).unwrap();
    child_lookup(|key, page| {
        let mut input = Vec::with_capacity(8 + page.len());
        input.extend_from_slice(&key.to_le_bytes());
        input.extend_from_slice(page);
        let reply = runtime.invoke(&input).unwrap();
        assert_eq!(reply.len(), 9);
        match reply[0] {
            0 => Search::Missing,
            1 => Search::Before,
            2 => Search::After,
            3 => Search::Found(value(&reply[1..])),
            _ => panic!("invalid search reply"),
        }
    });
}

fn child_lookup(mut search: impl FnMut(u64, &[u8]) -> Search) {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    output.write_all(READY).unwrap();
    output.flush().unwrap();
    let rows = integer(&mut input).unwrap() as usize;
    let key = integer(&mut input).unwrap();
    let fault = integer(&mut input).unwrap();
    let found = lookup(rows, |page| {
        let asset = if fault == 1 { ASSET + 1 } else { ASSET };
        let page = if fault == 2 { u64::MAX } else { page as u64 };
        output.write_all(&[asset]).unwrap();
        output.write_all(&page.to_le_bytes()).unwrap();
        output.flush().unwrap();
        let len = integer(&mut input).unwrap() as usize;
        assert!((RECORD..=PAGE).contains(&len) && len.is_multiple_of(RECORD));
        let mut bytes = vec![0; len];
        input.read_exact(&mut bytes).unwrap();
        search(key, &bytes)
    });
    output.write_all(&[0]).unwrap();
    output.write_all(&found.to_le_bytes()).unwrap();
    output.flush().unwrap();
}

#[derive(Default)]
struct Stats {
    ready_us: u128,
    reply_at: Option<Instant>,
    query_at: Option<Instant>,
    reads: usize,
    bytes: usize,
    denied: bool,
}

struct Asset {
    file: fs::File,
    rows: usize,
}

impl Asset {
    fn page(
        &mut self,
        id: u8,
        index: u64,
        reads: usize,
        cancel: &CancellationToken,
    ) -> io::Result<Vec<u8>> {
        if id != ASSET
            || index >= self.rows.div_ceil(ROWS_PER_PAGE) as u64
            || reads >= MAX_READS
            || cancel.is_cancelled()
        {
            return Err(io::Error::other("range denied"));
        }
        let start = index as usize * PAGE;
        let length = (self.rows * RECORD - start).min(PAGE);
        self.file.seek(SeekFrom::Start(start as u64))?;
        let mut bytes = vec![0; length];
        self.file.read_exact(&mut bytes)?;
        if cancel.is_cancelled() {
            return Err(io::Error::other("range withdrawn"));
        }
        Ok(bytes)
    }
}

fn exchange(
    mut output: fs::File,
    mut input: fs::File,
    mut asset: Asset,
    (key, fault): (u64, u64),
    cancel: CancellationToken,
    stats: Arc<Mutex<Stats>>,
    launched: Instant,
) -> io::Result<Vec<u8>> {
    let mut output = BufReader::new(&mut output);
    let mut prefix = Vec::new();
    while !prefix.ends_with(READY) && prefix.len() < 4096 {
        let mut byte = [0];
        output.read_exact(&mut byte)?;
        prefix.push(byte[0]);
    }
    if !prefix.ends_with(READY) {
        return Err(io::Error::other("missing ready"));
    }
    {
        let mut stats = stats.lock().unwrap();
        stats.ready_us = launched.elapsed().as_micros();
        stats.query_at = Some(Instant::now());
    }
    for n in [asset.rows as u64, key, fault] {
        input.write_all(&n.to_le_bytes())?;
    }
    loop {
        let mut id = [0];
        output.read_exact(&mut id)?;
        let number = integer(&mut output)?;
        let mut stats = stats.lock().unwrap();
        if id[0] == 0 {
            stats.reply_at = Some(Instant::now());
            drop(stats);
            drop(input);
            output.take(4096).read_to_end(&mut Vec::new())?;
            return Ok(number.to_le_bytes().to_vec());
        }
        if fault == 3 {
            cancel.cancel();
        }
        match asset.page(id[0], number, stats.reads, &cancel) {
            Ok(page) => {
                stats.reads += 1;
                stats.bytes += page.len();
                input.write_all(&(page.len() as u64).to_le_bytes())?;
                input.write_all(&page)?;
            }
            Err(_) => {
                stats.denied = true;
                return Err(io::Error::other("page withheld"));
            }
        }
    }
}

fn run(cache: &Cache, exe: &Path, asset: Asset, key: u64, expected: u64, fault: u64, wasm: bool) {
    let rows = asset.rows;
    let mut profile = Profile::new().unwrap();
    cache.allow_read_execute(&profile).unwrap();
    let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
    let cancel = CancellationToken::default();
    let worker_cancel = cancel.clone();
    let stats = Arc::new(Mutex::new(Stats::default()));
    let worker_stats = stats.clone();
    let launched = Instant::now();
    let scope = WorkerScope::observed(&profile, &job, cache, AtomicU64::new(0).into())
        .with_test_exchange(Box::new(move |output, input| {
            exchange(
                output,
                input,
                asset,
                (key, fault),
                worker_cancel,
                worker_stats,
                launched,
            )
            .map_err(|error| error.to_string())
        }));
    let child = if wasm { WASM_CHILD } else { CHILD };
    let args = ["--exact", child, "--ignored", "--nocapture"].map(OsString::from);
    let control = RunControl {
        cancel,
        walltime: Some(Duration::from_secs(10)),
        ..RunControl::default()
    };
    {
        let Outcome::Exited(receipt) =
            execute_observed(scope, exe, &args, Vec::new(), &control).unwrap()
        else {
            panic!("no true Worker exit receipt")
        };
        let stats = stats.lock().unwrap();
        if fault == 0 {
            assert_eq!(receipt.reason(), EndReason::Exited);
            assert_eq!(receipt.exit_code(), Some(0));
            assert_eq!(value(receipt.output().unwrap()), expected);
            let query = stats.query_at.unwrap();
            let reply = stats.reply_at.unwrap().duration_since(query).as_micros();
            let exit = receipt
                .signalled_at
                .saturating_duration_since(query)
                .as_micros();
            let (cpu, peak) = job_usage(&job);
            println!(
                "range wasm={wasm} rows={rows} ready_us={} reply_us={reply} exit_us={exit} pages={} bytes={} cpu_100ns={cpu} peak_bytes={peak}",
                stats.ready_us, stats.reads, stats.bytes
            );
        } else {
            assert!(stats.denied);
            assert_eq!(stats.bytes, 0);
            assert!(stats.reply_at.is_none());
            assert!(receipt.output().is_err());
        }
    }
    drop(job);
    cache.revoke_read_execute(&profile).unwrap();
    profile.delete().unwrap();
}

#[test]
#[ignore = "requires real AppContainers"]
fn range_lookup_cost() {
    let mut naming = Profile::new().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cache = Cache::new(root, &naming).unwrap();
    let mut asset_naming = Profile::new().unwrap();
    let asset_cache = Cache::new(root, &asset_naming).unwrap();
    let exe = cache
        .copy(&std::env::current_exe().unwrap(), "probe.exe")
        .unwrap();
    let bundle = fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
    let dll = cache
        .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
        .unwrap();
    for rows in [1_000, 100_000, 1_000_000] {
        let started = Instant::now();
        let mut bytes = Vec::with_capacity(rows * RECORD);
        for index in 0..rows as u64 {
            bytes.extend_from_slice(&(index * 3 + index % 3).to_le_bytes());
            bytes.extend_from_slice(&(index * 11 + 5).to_le_bytes());
        }
        let built_us = started.elapsed().as_micros();
        let path = asset_cache.private_canary(&bytes).unwrap();
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let checked_us = {
            let check = Instant::now();
            let mut captured = Vec::new();
            file.try_clone()
                .unwrap()
                .read_to_end(&mut captured)
                .unwrap();
            assert_eq!(digest(&captured), digest(&bytes));
            check.elapsed().as_micros()
        };
        assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
        verify_private_asset(&cache, &exe, &path);
        println!(
            "range-asset rows={rows} bytes={} build_us={built_us} verify_us={checked_us}",
            bytes.len()
        );
        for sample in 0..20 {
            let index = (sample as u64 * 73_919 + 71) % rows as u64;
            let key = index * 3 + index % 3;
            let miss = sample % 4 == 3;
            let first = sample % 2 != 0;
            for wasm in [first, !first] {
                run(
                    &cache,
                    &exe,
                    Asset {
                        file: file.try_clone().unwrap(),
                        rows,
                    },
                    if miss { (index / 3) * 9 + 1 } else { key },
                    if miss { ABSENT } else { index * 11 + 5 },
                    0,
                    wasm,
                );
            }
        }
        for fault in 1..=3 {
            for wasm in [false, true] {
                run(
                    &cache,
                    &exe,
                    Asset {
                        file: file.try_clone().unwrap(),
                        rows,
                    },
                    0,
                    5,
                    fault,
                    wasm,
                );
            }
        }
        drop(file);
        fs::remove_file(path).unwrap();
    }
    fs::remove_file(exe).unwrap();
    fs::remove_file(dll).unwrap();
    fs::remove_dir(cache.path()).unwrap();
    fs::remove_dir(asset_cache.path()).unwrap();
    naming.delete().unwrap();
    asset_naming.delete().unwrap();
}
