//! Fixed public dictionary, no live data or product transport changes.
use super::{Api, Cache, Profile, compiled_module, digest, fs, invoke, limits};
use serde_json::{Value, json};
use std::{hint::black_box, path::Path, time::Instant};

const SEED: &str = include_str!("../../../../distribution/defaults/dictionary.tsv");
const CONTRACT: &str = include_str!("../../../../tests/contracts/dictionary_exact.json");
const SAMPLES: usize = 20;

fn fixture(rows: usize) -> Vec<u8> {
    assert!(SEED.ends_with('\n'));
    let mut tsv = SEED.to_owned();
    for id in 3..rows {
        tsv.push_str(&format!("試験用{id:05}\t合成{id:05}\t1\n"));
    }
    tsv.into_bytes()
}

fn operation(method: &str, tsv: &[u8]) -> Value {
    let mut operation = json!({
        "method":method, "config":{"path":"dictionary.tsv"},
        "resources":{"dictionary.tsv":tsv}
    });
    if method == "execute" {
        operation["inputs"] = json!({"in":{"state":"data","packet":{
            "version":1, "root":{"type":"text","text":"こうすい"}
        }}});
        operation["mode"] = json!("compose");
    }
    json!({"apiVersion":1,"node":{"kind":"text.dictionary-exact","version":1},"operation":operation})
}

fn elapsed_us(start: Instant) -> u128 {
    start.elapsed().as_micros()
}

fn summary(mut values: Vec<u128>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_unstable();
    let n = values.len();
    json!({"n":n,"p50Us":values[(n * 50).div_ceil(100) - 1],
        "p95Us":values[(n * 95).div_ceil(100) - 1],"maxUs":values[n - 1]})
}

pub(super) fn guest(api: &Api, artifact: &[u8]) -> Vec<u8> {
    let digest = fs::read_to_string("artifact.sha256").unwrap();
    let prepare = fs::read("prepare.json").unwrap();
    let execute = fs::read("execute.json").unwrap();
    let contract: Value = serde_json::from_str(CONTRACT).unwrap();
    let expected: crate::Packet =
        serde_json::from_value(contract["cases"][0]["expected"]["packet"].clone()).unwrap();
    let mut prepare_us = Vec::new();
    let mut execute_us = Vec::new();
    let mut reply_decode_us = Vec::new();
    let mut fuel_failures = 0;
    for _ in 0..SAMPLES {
        let mut runtime = compiled_module::load(api, &limits(), None, artifact, &digest).unwrap();
        let started = Instant::now();
        let reply = runtime.invoke(&prepare);
        let elapsed = elapsed_us(started);
        match reply {
            Err(error) => {
                let error = error.to_ascii_lowercase();
                assert!(error.contains("all fuel consumed") || error.contains("out of fuel"));
                fuel_failures += 1;
                continue;
            }
            Ok(bytes) => {
                let reply: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(reply["ok"], true);
                assert_eq!(reply["data"], json!({"valid":true,"prepared":true}));
                prepare_us.push(elapsed);
            }
        }
        drop(runtime);
        let mut runtime = compiled_module::load(api, &limits(), None, artifact, &digest).unwrap();
        let started = Instant::now();
        let bytes = runtime.invoke(&execute).unwrap();
        execute_us.push(elapsed_us(started));
        let started = Instant::now();
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["data"]["out"]["state"], "data");
        let packet: crate::Packet =
            serde_json::from_value(reply["data"]["out"]["packet"].clone()).unwrap();
        assert_eq!(packet, expected);
        reply_decode_us.push(elapsed_us(started));
    }
    serde_json::to_vec(
        &json!({"attempts":SAMPLES,"prepareFuelFailures":fuel_failures,
        "prepare":summary(prepare_us),"execute":summary(execute_us),
        "replyDecodeAndCheck":summary(reply_decode_us)}),
    )
    .unwrap()
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn dictionary_wire_cost() {
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
    let expected = digest(&artifact);
    for rows in [3, 1000, 3000, 10000] {
        let tsv = fixture(rows);
        let prepare = serde_json::to_vec(&operation("prepare", &tsv)).unwrap();
        let cached = serde_json::to_vec(&operation("execute", &tsv)).unwrap();
        let mut projection = Vec::new();
        let mut encode = Vec::new();
        let mut cached_copy = Vec::new();
        for _ in 0..100 {
            let started = Instant::now();
            let request = operation("execute", black_box(&tsv));
            projection.push(elapsed_us(started));
            let started = Instant::now();
            let generated = serde_json::to_vec(&request).unwrap();
            encode.push(elapsed_us(started));
            let started = Instant::now();
            let reused = black_box(&cached).clone();
            cached_copy.push(elapsed_us(started));
            assert_eq!(generated, reused);
        }
        let prepare_path = cache.write("prepare.json", &prepare).unwrap();
        let execute_path = cache.write("execute.json", &cached).unwrap();
        let (_, _, result) = invoke(
            &cache,
            &exe,
            "dictionary-wire",
            artifact.clone(),
            Some(&expected),
            false,
        )
        .unwrap();
        let guest: Value = serde_json::from_slice(&result).unwrap();
        println!(
            "wire rows={rows} report={}",
            json!({"tsvBytes":tsv.len(),"wireBytes":cached.len(),
            "projection":summary(projection),"encode":summary(encode),
            "cachedCopy":summary(cached_copy),"guest":guest})
        );
        for path in [prepare_path, execute_path] {
            fs::remove_file(path).unwrap();
        }
    }
    for path in [exe, dll, wasm] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    naming.delete().unwrap();
}
