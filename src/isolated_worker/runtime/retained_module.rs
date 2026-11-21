//! Native-only retained protocol. Product selection/ownership is a separate gate.
use super::*;
use crate::isolated_worker::resource_transport::{Client, ResourceReader};
use crate::isolated_worker::retained_startup;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
};

const MAGIC: &[u8; 8] = b"DIYRET01";
const TIMED_MAGIC: &[u8; 8] = b"DIYRET02";
const MAX_MODULES: usize = 4;
const MAX_MODULE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_CALLS: u64 = 4096;

/// The owner has no Store/Instance. Each borrowed invocation owns its Store and
/// callback environment, and cannot outlive or mutably alias its code owner.
struct Compiled<'a> {
    code: Runtime<'a>,
    limits: Limits,
}
impl<'a> Compiled<'a> {
    /// # Safety
    /// Parent must supply its own settled compiler output, source/policy bound
    /// and pinned through this original process's exit and asset cleanup.
    unsafe fn from_captured(
        api: &'a Api,
        bytes: &[u8],
        limits: Limits,
        digest: &str,
        source: &str,
    ) -> Result<Self, String> {
        super::compiled_jit::validate_produced(
            bytes,
            source,
            limits.fuel,
            limits.memory.map(|m| m as u64),
        )?;
        // Compilation has no Instance or guest execution. The compiler format
        // binds fuel/memory; table/count bounds are installed on EACH actual
        // invocation below, not removed from that invocation's Store policy.
        let preparation = Limits {
            table_elements: None,
            tables: None,
            memories: None,
            ..limits.clone()
        };
        let mut code =
            unsafe { super::compiled_jit::prepare(api, &preparation, None, bytes, digest)? };
        code.discard_store();
        Ok(Self { code, limits })
    }

    fn new(api: &'a Api, bytes: &[u8], limits: Limits) -> Result<Self, String> {
        let mut code = Runtime::prepare_module(api, &limits, None, |engine, result| unsafe {
            // SAFETY: verified API, live bytes, and output belongs to this Engine.
            api.check(
                (api.module_new)(engine, bytes.as_ptr(), bytes.len(), result),
                ptr::null_mut(),
            )
        })?;
        code.discard_store();
        Ok(Self { code, limits })
    }
    fn invoke(&mut self, input: &[u8], reader: &mut dyn ResourceReader) -> Result<Vec<u8>, String> {
        self.invoke_measured(input, reader, false)
            .map(|(reply, _)| reply)
    }
    fn invoke_measured(
        &mut self,
        input: &[u8],
        reader: &mut dyn ResourceReader,
        timed: bool,
    ) -> Result<(Vec<u8>, Option<super::super::retained_metrics::Metrics>), String> {
        let capture = timed.then(super::super::retained_metrics::Capture::new);
        let mut timed_reader;
        let reader = if let Some(capture) = &capture {
            timed_reader = super::super::retained_metrics::TimedReader { reader, capture };
            &mut timed_reader as &mut dyn ResourceReader
        } else {
            reader
        };
        let mut call = Runtime {
            api: self.code.api,
            engine: self.code.engine,
            module: self.code.module,
            store: ptr::null_mut(),
            context: ptr::null_mut(),
            instance: Handle::default(),
            fuel: self.limits.fuel,
            diagnostics: None,
            resource: None,
            owns_code: false,
        };
        call.install_store(&self.limits)?;
        let mut call = resource_import::attach(call, reader, false)?;
        let reply = call.invoke_timed(input, capture.as_ref())?;
        if call.resource.as_ref().is_some_and(|bridge| bridge.failed()) {
            return Err("retained resource callback failed".into());
        }
        // Store and callback are destroyed before the caller can write RESULT.
        drop(call);
        Ok((
            reply,
            capture
                .as_ref()
                .and_then(super::super::retained_metrics::Capture::finish),
        ))
    }
}

fn positive(value: &std::ffi::OsStr, cap: u64) -> Result<u64, String> {
    value
        .to_str()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= cap)
        .ok_or("invalid retained limit".into())
}
fn next(input: &mut impl Read, expected: u64, modules: usize) -> io::Result<Option<(u32, bool)>> {
    let mut magic = [0; 8];
    if input.read(&mut magic[..1])? == 0 {
        return Ok(None);
    }
    input.read_exact(&mut magic[1..])?;
    let mut sequence = [0; 8];
    input.read_exact(&mut sequence)?;
    let mut slot = [0; 4];
    input.read_exact(&mut slot)?;
    let slot = u32::from_le_bytes(slot);
    if (&magic != MAGIC && &magic != TIMED_MAGIC)
        || u64::from_le_bytes(sequence) != expected
        || slot as usize >= modules
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid retained call",
        ));
    }
    Ok(Some((slot, &magic == TIMED_MAGIC)))
}
fn acknowledge(output: &mut impl Write, sequence: u64, slot: u32, timed: bool) -> io::Result<()> {
    output.write_all(if timed { TIMED_MAGIC } else { MAGIC })?;
    output.write_all(&sequence.to_le_bytes())?;
    output.write_all(&slot.to_le_bytes())?;
    output.flush()
}

pub(super) fn run(args: &[OsString]) -> Result<(), String> {
    run_modules(args, false, false)
}

pub(super) fn run_compiled(args: &[OsString]) -> Result<(), String> {
    run_modules(args, true, false)
}

pub(super) fn run_compiled_timed(args: &[OsString]) -> Result<(), String> {
    run_modules(args, true, true)
}

fn micros(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn run_modules(args: &[OsString], compiled: bool, timed_startup: bool) -> Result<(), String> {
    // runtime calls fuel memory, then 1..4 (module, exact length) pairs.
    // The new compile-cache experiment is restricted to one Node/module.
    let stride = if compiled { 4 } else { 2 };
    if (compiled && args.len() != 8)
        || args.len() < 4 + stride
        || args.len() > 4 + stride * MAX_MODULES
        || !(args.len() - 4).is_multiple_of(stride)
    {
        return Err("expected retained runtime calls fuel memory module length pairs".into());
    }
    let calls = positive(&args[1], MAX_CALLS)?;
    let fuel = positive(&args[2], 5_000_000_000)?;
    let memory = positive(&args[3], 128 * 1024 * 1024)?;
    let path = Path::new(&args[0]);
    if !path.is_absolute() {
        return Err("retained runtime path must be absolute".into());
    }
    let (api, mut startup_timing) = if timed_startup {
        use std::time::Instant;
        let begin = Instant::now();
        let mut verify_us = 0;
        let mut load_begin = begin;
        let api = Api::load_observed(path, || {
            verify_us = micros(begin);
            load_begin = Instant::now();
        })?;
        (
            api,
            Some(retained_startup::Timing {
                dll_verify_us: verify_us,
                dll_load_us: micros(load_begin),
                module_read_us: 0,
                module_restore_us: 0,
            }),
        )
    } else {
        (Api::load(path)?, None)
    };
    let limits = Limits {
        fuel: Some(fuel),
        memory: Some(memory as i64),
        table_elements: Some(10000),
        tables: Some(1),
        memories: Some(1),
    };
    let mut modules = Vec::new();
    let mut total = 0usize;
    for pair in args[4..].chunks_exact(stride) {
        let length = positive(&pair[1], MAX_MODULE_BYTES as u64)? as usize;
        total = total
            .checked_add(length)
            .filter(|value| *value <= MAX_TOTAL_BYTES)
            .ok_or("retained modules exceed total bound")?;
        if !Path::new(&pair[0]).is_absolute() {
            return Err("retained module path must be absolute".into());
        }
        let read_started = timed_startup.then(std::time::Instant::now);
        let mut bytes = Vec::new();
        std::fs::File::open(&pair[0])
            .map_err(|_| "retained module open failed")?
            .take(length as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "retained module read failed")?;
        if bytes.len() != length {
            return Err("retained module length mismatch".into());
        }
        if let (Some(start), Some(timing)) = (read_started, &mut startup_timing) {
            timing.module_read_us = micros(start);
        }
        let restore_started = timed_startup.then(std::time::Instant::now);
        let module = if compiled {
            let digest = pair[2].to_str().ok_or("invalid retained code digest")?;
            let source = pair[3].to_str().ok_or("invalid retained source digest")?;
            // SAFETY: private native entry selected by the trusted Parent from
            // its own compile-only producer. Frame/hash checks alone do NOT
            // authorize a package to supply serialized executable bytes.
            unsafe { Compiled::from_captured(&api, &bytes, limits.clone(), digest, source)? }
        } else {
            Compiled::new(&api, &bytes, limits.clone())?
        };
        if let (Some(start), Some(timing)) = (restore_started, &mut startup_timing) {
            timing.module_restore_us = micros(start);
        }
        modules.push(module);
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    retained_startup::write_ready(&mut output, modules.len() as u32, startup_timing)
        .map_err(|_| "retained readiness failed")?;
    for sequence in 1..=calls {
        let Some((slot, timed)) =
            next(&mut input, sequence, modules.len()).map_err(|_| "retained header rejected")?
        else {
            return Ok(());
        };
        acknowledge(&mut output, sequence, slot, timed)
            .map_err(|_| "retained acknowledgement failed")?;
        let (mut client, request) =
            Client::start(&mut input, &mut output).map_err(|_| "retained call start failed")?;
        let (reply, metrics) = if timed {
            modules[slot as usize].invoke_measured(&request, &mut client, true)?
        } else {
            (modules[slot as usize].invoke(&request, &mut client)?, None)
        };
        if let Some(metrics) = metrics {
            client.finish_retained_measured(&reply, metrics.encode(sequence, slot))
        } else {
            client.finish_retained(&reply)
        }
        .map_err(|_| "retained reply failed")?;
    }
    // Give Parent time to publish the final valid reply. Budget exhaustion is
    // not an early EOF/exit that can race that publication. No further call runs.
    match next(&mut input, calls + 1, modules.len())
        .map_err(|_| "retained limit header rejected")?
    {
        None => Ok(()),
        Some(_) => Err("retained call budget exhausted".into()),
    }
}

#[cfg(test)]
pub(in crate::isolated_worker) fn state_fixture(dll: &Path) -> Vec<u8> {
    let api = Api::load(dll).unwrap();
    let reply = r#"{"apiVersion":1,"ok":true,"data":{"counter":0,"memory":0}}"#;
    let position = 16 + reply.find(":0").unwrap() + 1;
    let memory_position = 16 + reply.rfind('0').unwrap();
    let data = reply.replace('"', "\\22");
    let bad = r#"{"apiVersion":1,"apiVersion":1,"ok":true,"data":{}}"#;
    let bad_data = bad.replace('"', "\\22");
    let wat = format!(
        r#"(module
        (import "diyime_resource_v1" "read" (func $read (param i32 i64 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (global $counter (mut i32) (i32.const 0))
        (data (i32.const 16) "{data}")
        (data (i32.const 256) "{bad_data}")
        (func (export "diyime_api_version") (result i32) i32.const 1)
        (func (export "diyime_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "diyime_release_reply"))
        (func (export "diyime_call") (param i32 i32) (result i64)
            local.get 0 i32.load8_u i32.const 64 i32.eq
            if
                i32.const 1 i64.const 0 i32.const 1 i32.const 8192 call $read drop
            end
            local.get 0 i32.load8_u i32.const 33 i32.eq
            if
                i32.const 1 i64.const 0 i32.const 1 i32.const 8192 call $read drop
                (loop $spin br $spin)
            end
            local.get 0 i32.load8_u i32.const 126 i32.eq if unreachable end
            local.get 0 i32.load8_u i32.const 63 i32.eq if i64.const {bad_descriptor} return end
            global.get $counter i32.const 1 i32.add global.set $counter
            i32.const {position} global.get $counter i32.const 48 i32.add i32.store8
            i32.const 4096 i32.const 4096 i32.load8_u i32.const 1 i32.add i32.store8
            i32.const {memory_position} i32.const 4096 i32.load8_u i32.const 48 i32.add i32.store8
            i64.const {descriptor}))"#,
        descriptor = ((reply.len() as u64) << 32) | 16,
        bad_descriptor = ((bad.len() as u64) << 32) | 256
    );
    let mut bytes = Bytes {
        size: 0,
        data: ptr::null_mut(),
    };
    // SAFETY: live WAT input and pinned C API's owned output byte vector.
    unsafe {
        api.check(
            (api.wat)(wat.as_ptr(), wat.len(), &mut bytes),
            ptr::null_mut(),
        )
        .unwrap();
        let output = std::slice::from_raw_parts(bytes.data, bytes.size).to_vec();
        (api.bytes_delete)(&mut bytes);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct FixtureResources;
    impl ResourceReader for FixtureResources {
        fn read_resource(&mut self, _: u32, _: u64, length: u32) -> io::Result<Vec<u8>> {
            assert_eq!(length, 1);
            Ok(vec![0])
        }
        fn emit_artifact(&mut self, _: &[u8]) -> io::Result<()> {
            panic!("execution cannot emit a prepared artifact")
        }
    }

    #[test]
    #[ignore = "requires explicit pinned Wasmtime DLL"]
    fn real_captured_retained_code_keeps_fresh_state_and_execution_limits() {
        let dll = std::path::PathBuf::from(
            std::env::var_os("DIY_RANGE_RUNTIME_DLL").expect("explicit pinned DLL"),
        );
        let api = Api::load(&dll).unwrap();
        let limits = Limits {
            fuel: Some(100_000),
            memory: Some(65_536),
            table_elements: Some(10_000),
            tables: Some(1),
            memories: Some(1),
        };
        let preparation = Limits {
            table_elements: None,
            tables: None,
            memories: None,
            ..limits.clone()
        };
        let state = state_fixture(&dll);
        for strategy in [
            compiled_jit::Strategy::Standard,
            compiled_jit::Strategy::Winch,
        ] {
            // Test-only trusted native producer. No package/blob constructor.
            let bytes = compiled_jit::compile_for(&api, &preparation, &state, strategy).unwrap();
            let source = crate::isolated_worker::adapter::sha256(&state).unwrap();
            let digest = crate::isolated_worker::adapter::sha256(&bytes).unwrap();
            let mut code = unsafe {
                Compiled::from_captured(&api, &bytes, limits.clone(), &digest, &source).unwrap()
            };
            for _ in 0..6 {
                let reply = code.invoke(b"{}", &mut FixtureResources).unwrap();
                let value: serde_json::Value = serde_json::from_slice(&reply).unwrap();
                assert_eq!(value["data"], serde_json::json!({"counter":1,"memory":1}));
            }
            assert!(
                code.invoke(b"!", &mut FixtureResources)
                    .unwrap_err()
                    .contains("fuel")
            );
            assert!(code.invoke(b"{}", &mut FixtureResources).is_ok());
            for (settings, hash, wasm_hash) in [
                (
                    Limits {
                        fuel: Some(99_999),
                        ..limits.clone()
                    },
                    digest.clone(),
                    source.clone(),
                ),
                (
                    Limits {
                        memory: Some(131_072),
                        ..limits.clone()
                    },
                    digest.clone(),
                    source.clone(),
                ),
                (limits.clone(), "0".repeat(64), source.clone()),
                (limits.clone(), digest.clone(), "0".repeat(64)),
            ] {
                assert!(
                    unsafe { Compiled::from_captured(&api, &bytes, settings, &hash, &wasm_hash) }
                        .is_err()
                );
            }
        }

        // A real grow attempt must see the retained Store's strict limits,
        // despite the compile-only cache format not binding Store table counts.
        let wat = br#"(module
            (memory (export "memory") 1 4)
            (table 1 20002 funcref)
            (data (i32.const 32) "{}")
            (func (export "diyime_api_version") (result i32) i32.const 1)
            (func (export "diyime_alloc") (param i32) (result i32) i32.const 1024)
            (func (export "diyime_release_reply"))
            (func (export "diyime_call") (param i32 i32) (result i64)
                i32.const 1 memory.grow i32.const -1 i32.ne if unreachable end
                ref.null func i32.const 10000 table.grow i32.const -1 i32.ne if unreachable end
                i64.const 8589934624))"#;
        let mut wasm = Bytes {
            size: 0,
            data: ptr::null_mut(),
        };
        let bytes = unsafe {
            api.check(
                (api.wat)(wat.as_ptr(), wat.len(), &mut wasm),
                ptr::null_mut(),
            )
            .unwrap();
            let owned = std::slice::from_raw_parts(wasm.data, wasm.size).to_vec();
            (api.bytes_delete)(&mut wasm);
            owned
        };
        for strategy in [
            compiled_jit::Strategy::Standard,
            compiled_jit::Strategy::Winch,
        ] {
            let produced = compiled_jit::compile_for(&api, &preparation, &bytes, strategy).unwrap();
            let source = crate::isolated_worker::adapter::sha256(&bytes).unwrap();
            let digest = crate::isolated_worker::adapter::sha256(&produced).unwrap();
            let mut code = unsafe {
                Compiled::from_captured(&api, &produced, limits.clone(), &digest, &source).unwrap()
            };
            for _ in 0..3 {
                assert_eq!(code.invoke(b"{}", &mut FixtureResources).unwrap(), b"{}");
            }
        }
    }

    #[test]
    fn compiled_retained_entry_rejects_wrong_cardinality_before_runtime_load() {
        for count in 0..=16 {
            if count == 8 {
                continue;
            }
            let args = vec![OsString::from("not-a-runtime"); count];
            assert!(
                run_compiled(&args)
                    .unwrap_err()
                    .contains("expected retained")
            );
        }
    }
    #[test]
    fn native_sequence_slot_and_every_partial_header_are_checked() {
        let mut header = Vec::new();
        acknowledge(&mut header, 1, 0, false).unwrap();
        assert_eq!(
            next(&mut Cursor::new(&header), 1, 1).unwrap(),
            Some((0, false))
        );
        assert!(next(&mut Cursor::new(&header), 2, 1).is_err());
        assert!(next(&mut Cursor::new(&header), 1, 0).is_err());
        assert_eq!(next(&mut Cursor::new([]), 1, 1).unwrap(), None);
        for length in 1..header.len() {
            assert!(next(&mut Cursor::new(&header[..length]), 1, 1).is_err());
        }
        header[0] ^= 1;
        assert!(next(&mut Cursor::new(header), 1, 1).is_err());
    }
    #[test]
    fn limits_do_not_allow_unbounded_or_zero_execution() {
        for value in ["none", "0", "4097", "-1", "18446744073709551616"] {
            assert!(positive(std::ffi::OsStr::new(value), MAX_CALLS).is_err());
        }
        assert_eq!(
            positive(std::ffi::OsStr::new("4096"), MAX_CALLS).unwrap(),
            MAX_CALLS
        );
    }
}
