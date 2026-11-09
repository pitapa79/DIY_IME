//! Test-only same-module control for one-shot Windows Workers.
use super::*;
use serde_json::json;

fn request() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "apiVersion": 1,
        "node": {"kind":"fixture.wasm.chain","version":1},
        "operation": {"method":"execute","config":{"mode":"forward"},
            "inputs":{"in":{"state":"data","packet":{"version":1,
                "root":{"type":"text","text":"generic-probe"}}}}}
    }))
    .unwrap()
}

pub(super) fn child(api: &Api, mode: &str, input: &[u8]) -> Vec<u8> {
    if mode == "generic-reuse" {
        let expected = fs::read_to_string("artifact.sha256").unwrap();
        return compiled_module::load(api, &limits(), None, input, &expected)
            .unwrap()
            .invoke(&request())
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
        .invoke(&request())
        .unwrap()
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
    for pair in 0..24 {
        let modes = if pair % 2 == 0 {
            ["generic-jit", "generic-reuse"]
        } else {
            ["generic-reuse", "generic-jit"]
        };
        for mode in modes {
            let reuse = mode == "generic-reuse";
            let input = if reuse { artifact.clone() } else { Vec::new() };
            let (host_us, child_us, reply) = invoke(
                &cache,
                &exe,
                mode,
                input,
                reuse.then_some(artifact_hash.as_str()),
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
    for path in [module_hash_file, module, dll, exe] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(cache.path()).unwrap();
    profile.delete().unwrap();
}
