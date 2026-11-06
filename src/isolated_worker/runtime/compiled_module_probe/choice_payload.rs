//! Developer-only comparison of candidate bodies and opaque test labels.
use super::{Api, Cache, Profile, compiled_module, digest, fs, invoke, limits};
use serde_json::{Value, json};
use std::{path::Path, time::Instant};

const SAMPLES: usize = 20;

fn surface(index: usize, points: usize) -> String {
    format!("{index:02}{}", "漢".repeat(points - 2))
}

fn request(count: usize, points: usize, labels: bool) -> Vec<u8> {
    let options: Vec<_> = (0..count)
        .map(|index| {
            let text = if labels {
                format!("{index:02}")
            } else {
                surface(index, points)
            };
            let score = if index == 1 || index == 2 { 8.0 } else { -1.0 };
            json!({"value":{"type":"text","text":text},"score":score})
        })
        .collect();
    serde_json::to_vec(&json!({
        "apiVersion":1,"node":{"kind":"text.select-candidates","version":1},
        "operation":{"method":"execute","mode":"compose","config":{"unscored":0.0},
        "inputs":{"in":{"state":"data","packet":{"version":1,"root":{"type":"choice","options":options}}}}}
    }))
    .unwrap()
}

#[derive(Default, serde::Serialize)]
struct Samples {
    success_us: Vec<u128>,
    rejected_us: Vec<u128>,
    fuel_us: Vec<u128>,
}

pub(super) fn guest(api: &Api, artifact: &[u8]) -> Vec<u8> {
    let hash = fs::read_to_string("artifact.sha256").unwrap();
    let points: usize = fs::read_to_string("choice-points.txt")
        .unwrap()
        .parse()
        .unwrap();
    let requests = [
        fs::read("choice-full.json").unwrap(),
        fs::read("choice-labels.json").unwrap(),
    ];
    let [mut full, mut labels] = [Samples::default(), Samples::default()];
    let limits = limits();
    for trial in 0..SAMPLES {
        for index in [trial % 2, 1 - trial % 2] {
            let samples = if index == 0 { &mut full } else { &mut labels };
            let mut runtime = compiled_module::load(api, &limits, None, artifact, &hash).unwrap();
            let start = Instant::now();
            let result = runtime.invoke(&requests[index]);
            let elapsed = start.elapsed().as_micros();
            match result {
                Ok(bytes) => {
                    let reply: Value = serde_json::from_slice(&bytes).unwrap();
                    if reply["ok"] == true {
                        let text = reply["data"]["out"]["packet"]["root"]["text"]
                            .as_str()
                            .unwrap();
                        if index == 1 {
                            assert_eq!(text, "01");
                            assert_eq!(surface(text.parse().unwrap(), points), surface(1, points));
                        } else {
                            assert_eq!(text, surface(1, points));
                        }
                        samples.success_us.push(elapsed);
                    } else {
                        assert_eq!(reply["ok"], false);
                        assert_eq!(index, 0, "compact control was rejected");
                        samples.rejected_us.push(elapsed);
                    }
                }
                Err(error) => {
                    let error = error.to_ascii_lowercase();
                    assert!(error.contains("all fuel consumed") || error.contains("out of fuel"));
                    assert_eq!(index, 0, "compact control exhausted fuel");
                    samples.fuel_us.push(elapsed);
                }
            }
        }
    }
    serde_json::to_vec(&json!({"full":full,"labels":labels})).unwrap()
}

#[test]
#[ignore = "requires fixed public Windows bundle and real AppContainer"]
fn candidate_payload_cost() {
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
    let hash = digest(&artifact);

    for (count, points) in [(4, 4096), (64, 256), (64, 1024), (64, 4096)] {
        let full = request(count, points, false);
        let labels = request(count, points, true);
        let paths = [
            cache.write("choice-full.json", &full).unwrap(),
            cache.write("choice-labels.json", &labels).unwrap(),
            cache
                .write("choice-points.txt", points.to_string().as_bytes())
                .unwrap(),
        ];
        let (_, _, reply) = invoke(
            &cache,
            &exe,
            "candidate-payload",
            artifact.clone(),
            Some(&hash),
            false,
        )
        .unwrap();
        let report: Value = serde_json::from_slice(&reply).unwrap();
        println!(
            "choice-payload count={count} points={points} full_bytes={} labels_bytes={} report={report}",
            full.len(),
            labels.len()
        );
        for path in paths {
            fs::remove_file(path).unwrap();
        }
    }
    for path in [exe, dll, wasm] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    profile.delete().unwrap();
}
