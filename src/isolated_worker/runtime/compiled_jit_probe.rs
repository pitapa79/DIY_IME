//! Native-only feasibility probe, not a product cache or package artifact loader.
//! Only exact bytes returned by this process's verified compiler can be loaded.
use super::{Api, Bytes, Limits, Runtime, compiled_jit};
use compiled_jit::Strategy as Compiler;
use std::{path::Path, ptr, time::Instant};

type Settings = (
    Option<u64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);
fn settings(limits: &Limits) -> Settings {
    (
        limits.fuel,
        limits.memory,
        limits.table_elements,
        limits.tables,
        limits.memories,
    )
}
fn hash(bytes: &[u8]) -> Result<String, String> {
    crate::isolated_worker::adapter::sha256(bytes).map_err(|_| "probe hash failed".into())
}

// No file/package/blob constructor. API ownership also pins this probe to the
// same verified DLL and CPU/process; this is not cross-process provenance yet.
struct Produced<'a> {
    api: &'a Api,
    compiler: Compiler,
    settings: Settings,
    source: String,
    digest: String,
    bytes: Vec<u8>,
}
impl<'a> Produced<'a> {
    fn compile(
        api: &'a Api,
        compiler: Compiler,
        limits: &Limits,
        wasm: &[u8],
    ) -> Result<Self, String> {
        let result = compiled_jit::compile_for(api, limits, wasm, compiler)?;
        Ok(Self {
            api,
            compiler,
            settings: settings(limits),
            source: hash(wasm)?,
            digest: hash(&result)?,
            bytes: result,
        })
    }

    fn prepare(
        &self,
        compiler: Compiler,
        limits: &Limits,
        wasm: &[u8],
    ) -> Result<Runtime<'a>, String> {
        if compiler != self.compiler
            || settings(limits) != self.settings
            || hash(wasm)? != self.source
        {
            return Err("probe source/compiler/settings mismatch".into());
        }
        // SAFETY: this private test owner can only be constructed by the
        // verified compiler above, in this same process/API/CPU and settings.
        unsafe { compiled_jit::prepare(self.api, limits, None, &self.bytes, &self.digest) }
    }
}

fn limits() -> Limits {
    Limits {
        fuel: Some(10_000),
        memory: Some(1024 * 1024),
        table_elements: None,
        tables: None,
        memories: None,
    }
}
fn api() -> Api {
    Api::load(Path::new(
        &std::env::var_os("DIY_RANGE_RUNTIME_DLL").expect("pinned DLL"),
    ))
    .unwrap()
}
fn wat(api: &Api, text: &str) -> Vec<u8> {
    let mut output = Bytes {
        size: 0,
        data: ptr::null_mut(),
    };
    unsafe {
        api.check(
            (api.wat)(text.as_ptr(), text.len(), &mut output),
            ptr::null_mut(),
        )
        .unwrap();
        let result = std::slice::from_raw_parts(output.data, output.size).to_vec();
        (api.bytes_delete)(&mut output);
        result
    }
}

pub(in crate::isolated_worker) fn worker_fixture(dll: &Path, mode: &str) -> Vec<u8> {
    let api = Api::load(dll).unwrap();
    match mode {
        "trap" => wat(&api, "(module (func $start unreachable) (start $start))"),
        "tail" => wat(&api, "(module (func $callee) (func return_call $callee))"),
        "state" | "different" => {
            let reply = r#"{"apiVersion":1,"ok":true,"data":{"value":0}}"#;
            let position = 32 + reply.find(":0").unwrap() + 1;
            let increment = if mode == "state" { 1 } else { 2 };
            wat(
                &api,
                &format!(
                    r#"(module
                (memory (export "memory") 1)
                (data (i32.const 32) "{}")
                (func (export "diyime_api_version") (result i32) i32.const 1)
                (func (export "diyime_alloc") (param i32) (result i32) i32.const 1024)
                (func (export "diyime_call") (param i32 i32) (result i64)
                    i32.const {position} i32.const {position} i32.load8_u
                    i32.const {increment} i32.add i32.store8
                    i64.const {})
                (func (export "diyime_release_reply")))"#,
                    reply.replace('"', "\\22"),
                    ((reply.len() as u64) << 32) | 32
                ),
            )
        }
        _ => panic!("unknown fixed compiler fixture"),
    }
}

#[test]
#[ignore = "requires pinned Windows Wasmtime; native feasibility only"]
fn real_serialized_jit_preserves_fresh_state_start_traps_and_exact_provenance() {
    let api = api();
    let limits = limits();
    let state = wat(
        &api,
        r#"(module
        (memory (export "memory") 1)
        (data (i32.const 32) "0")
        (func (export "diyime_api_version") (result i32) i32.const 1)
        (func (export "diyime_alloc") (param i32) (result i32) i32.const 16)
        (func (export "diyime_call") (param i32 i32) (result i64)
            i32.const 32 i32.const 32 i32.load8_u i32.const 1 i32.add i32.store8
            i64.const 4294967328)
        (func (export "diyime_release_reply")))"#,
    );
    let start_trap = wat(&api, "(module (func $start unreachable) (start $start))");
    let fuel = wat(
        &api,
        r#"(module
        (memory (export "memory") 1) (data (i32.const 32) "{}")
        (func (export "diyime_api_version") (result i32) i32.const 1)
        (func (export "diyime_alloc") (param i32) (result i32) i32.const 16)
        (func (export "diyime_call") (param i32 i32) (result i64) (local $i i32)
            i32.const 1000000 local.set $i
            (loop $again local.get $i i32.const 1 i32.sub local.tee $i br_if $again)
            i64.const 8589934624)
        (func (export "diyime_release_reply")))"#,
    );
    let memory = wat(
        &api,
        r#"(module
        (memory (export "memory") 1) (data (i32.const 32) "{}")
        (func (export "diyime_api_version") (result i32) i32.const 1)
        (func (export "diyime_alloc") (param i32) (result i32) i32.const 16)
        (func (export "diyime_call") (param i32 i32) (result i64)
            i32.const 16 memory.grow i32.const -1 i32.ne if unreachable end
            i64.const 8589934624)
        (func (export "diyime_release_reply")))"#,
    );
    for compiler in [Compiler::Winch, Compiler::Standard] {
        let mut produced = Produced::compile(&api, compiler, &limits, &state).unwrap();
        for _ in 0..6 {
            let mut runtime = produced.prepare(compiler, &limits, &state).unwrap();
            runtime.instantiate().unwrap();
            assert_eq!(runtime.invoke(b"{}").unwrap(), b"1");
            assert_eq!(runtime.invoke(b"{}").unwrap(), b"2");
        }
        let other = if compiler == Compiler::Winch {
            Compiler::Standard
        } else {
            Compiler::Winch
        };
        assert!(produced.prepare(other, &limits, &state).is_err());
        assert!(produced.prepare(compiler, &limits, &start_trap).is_err());
        for changed in [
            Limits {
                fuel: None,
                ..limits.clone()
            },
            Limits {
                memory: Some(65536),
                ..limits.clone()
            },
        ] {
            assert!(produced.prepare(compiler, &changed, &state).is_err());
        }
        produced.bytes[0] ^= 1;
        assert!(
            produced
                .prepare(compiler, &limits, &state)
                .err()
                .expect("changed artifact must be refused before deserialize")
                .contains("JIT bytes changed")
        );
        let trapped = Produced::compile(&api, compiler, &limits, &start_trap).unwrap();
        let mut runtime = trapped.prepare(compiler, &limits, &start_trap).unwrap();
        assert!(runtime.instantiate().unwrap_err().contains("unreachable"));
        let metered = Produced::compile(&api, compiler, &limits, &fuel).unwrap();
        let mut runtime = metered.prepare(compiler, &limits, &fuel).unwrap();
        runtime.instantiate().unwrap();
        assert!(runtime.invoke(b"{}").unwrap_err().contains("fuel"));
        let bounded = Produced::compile(&api, compiler, &limits, &memory).unwrap();
        let mut runtime = bounded.prepare(compiler, &limits, &memory).unwrap();
        runtime.instantiate().unwrap();
        assert_eq!(runtime.invoke(b"{}").unwrap(), b"{}");
        assert!(Produced::compile(&api, compiler, &limits, b"invalid").is_err());
    }
    let tail_call = wat(&api, "(module (func $callee) (func return_call $callee))");
    assert!(Produced::compile(&api, Compiler::Winch, &limits, &tail_call).is_err());
    let fallback = Produced::compile(&api, Compiler::Standard, &limits, &tail_call).unwrap();
    let automatic = compiled_jit::compile(&api, &limits, &tail_call).unwrap();
    assert_eq!(automatic[8], Compiler::Standard as u8);
    compiled_jit::validate_produced(
        &automatic,
        &hash(&tail_call).unwrap(),
        limits.fuel,
        limits.memory.map(|m| m as u64),
    )
    .unwrap();
    fallback
        .prepare(Compiler::Standard, &limits, &tail_call)
        .unwrap()
        .instantiate()
        .unwrap();
}

#[test]
#[ignore = "requires pinned DLL and real fixture; native feasibility timing, not Host performance"]
fn real_serialized_jit_refill_cost_probe() {
    let api = api();
    let limits = limits();
    let wasm =
        std::fs::read(std::env::var_os("DIY_RETAINED_HOST_FIXTURE_WASM").expect("real fixture"))
            .unwrap();
    let mut cases = Vec::new();
    for compiler in [Compiler::Winch, Compiler::Standard] {
        let artifact = Produced::compile(&api, compiler, &limits, &wasm).unwrap();
        let mut compile_us = Vec::new();
        let mut load_us = Vec::new();
        for _ in 0..20 {
            let started = Instant::now();
            drop(Produced::compile(&api, compiler, &limits, &wasm).unwrap());
            compile_us.push(started.elapsed().as_micros().to_string());
            let started = Instant::now();
            drop(artifact.prepare(compiler, &limits, &wasm).unwrap());
            load_us.push(started.elapsed().as_micros().to_string());
        }
        cases.push(serde_json::json!({"compiler":format!("{compiler:?}"), "artifactBytes":artifact.bytes.len(),
            "sourceSha256":artifact.source,"compileAndSerializeUs":compile_us,"freshEngineStoreDeserializeUs":load_us}));
    }
    let report = serde_json::json!({"scope":"native same-process feasibility; not ordinary Host or cross-process provenance", "cases":cases});
    if let Some(path) = std::env::var_os("DIY_COMPILED_JIT_PROBE_REPORT") {
        let file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        serde_json::to_writer(file, &report).unwrap();
    }
    eprintln!("{report}");
}
