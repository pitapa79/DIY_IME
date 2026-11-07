//! Native-only retained protocol. Product selection/ownership is a separate gate.
use super::*;
use crate::isolated_worker::resource_transport::{Client, ResourceReader};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
};

const MAGIC: &[u8; 8] = b"DIYRET01";
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
        let reply = call.invoke(input)?;
        if call.resource.as_ref().is_some_and(|bridge| bridge.failed()) {
            return Err("retained resource callback failed".into());
        }
        // Store and callback are destroyed before the caller can write RESULT.
        drop(call);
        Ok(reply)
    }
}

fn positive(value: &std::ffi::OsStr, cap: u64) -> Result<u64, String> {
    value
        .to_str()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= cap)
        .ok_or("invalid retained limit".into())
}
fn next(input: &mut impl Read, expected: u64, modules: usize) -> io::Result<Option<u32>> {
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
    if &magic != MAGIC || u64::from_le_bytes(sequence) != expected || slot as usize >= modules {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid retained call",
        ));
    }
    Ok(Some(slot))
}
fn acknowledge(output: &mut impl Write, sequence: u64, slot: u32) -> io::Result<()> {
    output.write_all(MAGIC)?;
    output.write_all(&sequence.to_le_bytes())?;
    output.write_all(&slot.to_le_bytes())?;
    output.flush()
}

pub(super) fn run(args: &[OsString]) -> Result<(), String> {
    // runtime calls fuel memory, then 1..4 (module, exact length) pairs.
    if args.len() < 6 || args.len() > 4 + 2 * MAX_MODULES || !(args.len() - 4).is_multiple_of(2) {
        return Err("expected retained runtime calls fuel memory module length pairs".into());
    }
    let calls = positive(&args[1], MAX_CALLS)?;
    let fuel = positive(&args[2], 5_000_000_000)?;
    let memory = positive(&args[3], 128 * 1024 * 1024)?;
    let path = Path::new(&args[0]);
    if !path.is_absolute() {
        return Err("retained runtime path must be absolute".into());
    }
    let api = Api::load(path)?;
    let limits = Limits {
        fuel: Some(fuel),
        memory: Some(memory as i64),
        table_elements: Some(10000),
        tables: Some(1),
        memories: Some(1),
    };
    let mut modules = Vec::new();
    let mut total = 0usize;
    for pair in args[4..].chunks_exact(2) {
        let length = positive(&pair[1], MAX_MODULE_BYTES as u64)? as usize;
        total = total
            .checked_add(length)
            .filter(|value| *value <= MAX_TOTAL_BYTES)
            .ok_or("retained modules exceed total bound")?;
        if !Path::new(&pair[0]).is_absolute() {
            return Err("retained module path must be absolute".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&pair[0])
            .map_err(|_| "retained module open failed")?
            .take(length as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "retained module read failed")?;
        if bytes.len() != length {
            return Err("retained module length mismatch".into());
        }
        modules.push(Compiled::new(&api, &bytes, limits.clone())?);
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output
        .write_all(b"DIYRDY01")
        .and_then(|()| output.write_all(&(modules.len() as u32).to_le_bytes()))
        .and_then(|()| output.flush())
        .map_err(|_| "retained readiness failed")?;
    for sequence in 1..=calls {
        let Some(slot) =
            next(&mut input, sequence, modules.len()).map_err(|_| "retained header rejected")?
        else {
            return Ok(());
        };
        acknowledge(&mut output, sequence, slot).map_err(|_| "retained acknowledgement failed")?;
        let (mut client, request) =
            Client::start(&mut input, &mut output).map_err(|_| "retained call start failed")?;
        let reply = modules[slot as usize].invoke(&request, &mut client)?;
        client
            .finish_retained(&reply)
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
    #[test]
    fn native_sequence_slot_and_every_partial_header_are_checked() {
        let mut header = Vec::new();
        acknowledge(&mut header, 1, 0).unwrap();
        assert_eq!(next(&mut Cursor::new(&header), 1, 1).unwrap(), Some(0));
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
