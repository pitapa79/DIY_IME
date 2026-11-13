//! Private compile-only Worker format. The format is not an artifact trust gate:
//! Parent must prove successful exit of its own captured compiler process first.
use super::{
    Api, Bytes, Limits, P, Recorder, Runtime, compiled_module, create_engine, create_winch_engine,
};
use std::ptr;

const MAGIC: &[u8; 8] = b"DIYJIT01";
const HEADER: usize = 8 + 1 + 8 + 8 + 64 + 4;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Strategy {
    Standard = 1,
    Winch = 2,
}
impl Strategy {
    fn engine(self, api: &Api, limits: &Limits) -> Result<P, String> {
        match self {
            Self::Standard => create_engine(api, limits),
            Self::Winch => create_winch_engine(api, limits),
        }
    }
}
struct Frame<'a> {
    strategy: Strategy,
    fuel: Option<u64>,
    memory: Option<i64>,
    source: &'a str,
    code: &'a [u8],
}
fn hash(bytes: &[u8]) -> Result<String, String> {
    crate::isolated_worker::adapter::sha256(bytes).map_err(|_| "compiled JIT hash failed".into())
}
fn parse(bytes: &[u8]) -> Result<Frame<'_>, String> {
    if !(HEADER + 1..=MAX_BYTES).contains(&bytes.len()) || &bytes[..8] != MAGIC {
        return Err("invalid compiled JIT frame".into());
    }
    let strategy = match bytes[8] {
        1 => Strategy::Standard,
        2 => Strategy::Winch,
        _ => return Err("invalid compiled JIT strategy".into()),
    };
    let fuel = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
    let memory = u64::from_le_bytes(bytes[17..25].try_into().unwrap());
    let memory = i64::try_from(memory).map_err(|_| "compiled JIT memory overflow")?;
    let source = std::str::from_utf8(&bytes[25..89]).map_err(|_| "invalid compiled JIT source")?;
    if !source
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid compiled JIT source digest".into());
    }
    let length = u32::from_le_bytes(bytes[89..93].try_into().unwrap()) as usize;
    if length == 0 || length != bytes.len() - HEADER {
        return Err("compiled JIT length mismatch".into());
    }
    Ok(Frame {
        strategy,
        fuel: (fuel != 0).then_some(fuel),
        memory: (memory != 0).then_some(memory),
        source,
        code: &bytes[HEADER..],
    })
}

/// Safe syntax/binding check, never proof that arbitrary bytes are executable.
pub(in crate::isolated_worker) fn validate_produced(
    bytes: &[u8],
    source: &str,
    fuel: Option<u64>,
    memory: Option<u64>,
) -> Result<(), String> {
    let frame = parse(bytes)?;
    if frame.source != source || frame.fuel != fuel || frame.memory.map(|m| m as u64) != memory {
        return Err("compiled JIT source or settings mismatch".into());
    }
    Ok(())
}

pub(super) fn compile(api: &Api, limits: &Limits, wasm: &[u8]) -> Result<Vec<u8>, String> {
    // Both attempts compile only. Neither can run a Wasm start function.
    compile_for(api, limits, wasm, Strategy::Winch)
        .or_else(|_| compile_for(api, limits, wasm, Strategy::Standard))
}
pub(super) fn compile_for(
    api: &Api,
    limits: &Limits,
    wasm: &[u8],
    strategy: Strategy,
) -> Result<Vec<u8>, String> {
    if !wasm.starts_with(b"\0asm\x01\0\0\0")
        || limits.fuel == Some(0)
        || limits.memory.is_some_and(|m| m <= 0)
        || limits.table_elements.is_some()
        || limits.tables.is_some()
        || limits.memories.is_some()
    {
        return Err("unsupported compiled JIT source/settings".into());
    }
    let serialize: unsafe extern "C" fn(P, *mut Bytes) -> P =
        unsafe { compiled_module::symbol(api, c"wasmtime_module_serialize")? };
    let engine = strategy.engine(api, limits)?;
    let mut module = ptr::null_mut();
    let mut output = Bytes {
        size: 0,
        data: ptr::null_mut(),
    };
    // SAFETY: pinned compiler C API, live source and initialized outputs.
    // There is no Store/Instance/input/resource broker on this path.
    let code = unsafe {
        let result: Result<Vec<u8>, String> = (|| {
            api.check(
                (api.module_new)(engine, wasm.as_ptr(), wasm.len(), &mut module),
                ptr::null_mut(),
            )?;
            api.check(serialize(module, &mut output), ptr::null_mut())?;
            if output.data.is_null() || !(1..=MAX_BYTES - HEADER).contains(&output.size) {
                return Err("compiled JIT exceeds bound".into());
            }
            Ok(std::slice::from_raw_parts(output.data, output.size).to_vec())
        })();
        if !output.data.is_null() {
            (api.bytes_delete)(&mut output);
        }
        if !module.is_null() {
            (api.module_delete)(module);
        }
        (api.engine_delete)(engine);
        result
    }?;
    let mut frame = Vec::with_capacity(HEADER + code.len());
    frame.extend_from_slice(MAGIC);
    frame.push(strategy as u8);
    frame.extend_from_slice(&limits.fuel.unwrap_or(0).to_le_bytes());
    frame.extend_from_slice(&(limits.memory.unwrap_or(0) as u64).to_le_bytes());
    frame.extend_from_slice(hash(wasm)?.as_bytes());
    frame.extend_from_slice(&(code.len() as u32).to_le_bytes());
    frame.extend_from_slice(&code);
    Ok(frame)
}

/// # Safety
/// `bytes` must be exact unmodified output of the trusted compile-only Worker,
/// captured by Parent only after original-process exit/IO/ACL/profile cleanup,
/// bound to its captured source, Worker/DLL and immutable policy. Hash/syntax
/// alone never establish this origin. Parent owns a locked, bounded code asset.
pub(super) unsafe fn prepare<'a>(
    api: &'a Api,
    limits: &Limits,
    diagnostics: Option<&'a Recorder>,
    bytes: &[u8],
    expected: &str,
) -> Result<Runtime<'a>, String> {
    unsafe {
        prepare_using(api, limits, diagnostics, bytes, expected, |s| {
            s.engine(api, limits)
        })
    }
}

/// Only public runtime state: no Module, Store, Instance or guest input. A
/// consumed owner selects exactly one compiler engine and drops the other.
pub(super) struct PublicRuntime<'a> {
    api: &'a Api,
    limits: Limits,
    standard: P,
    winch: P,
}
impl Drop for PublicRuntime<'_> {
    fn drop(&mut self) {
        unsafe {
            for engine in [self.standard, self.winch] {
                if !engine.is_null() {
                    (self.api.engine_delete)(engine);
                }
            }
        }
    }
}
impl<'a> PublicRuntime<'a> {
    pub(super) fn new(api: &'a Api, limits: &Limits) -> Result<Self, String> {
        let standard = create_engine(api, limits)?;
        Ok(Self {
            api,
            limits: limits.clone(),
            standard,
            winch: create_winch_engine(api, limits).unwrap_or(ptr::null_mut()),
        })
    }

    /// # Safety
    /// Same compiler-origin requirements as `prepare`. Neither digest nor the
    /// private transport establishes authority to deserialize external bytes.
    pub(super) unsafe fn prepare(
        mut self,
        diagnostics: Option<&'a Recorder>,
        bytes: &[u8],
        expected: &str,
    ) -> Result<Runtime<'a>, String> {
        let limits = self.limits.clone();
        unsafe {
            prepare_using(
                self.api,
                &limits,
                diagnostics,
                bytes,
                expected,
                |strategy| {
                    let selected = match strategy {
                        Strategy::Standard => &mut self.standard,
                        Strategy::Winch => &mut self.winch,
                    };
                    if selected.is_null() {
                        return Err("prepared compiler engine unavailable".into());
                    }
                    // Runtime takes ownership even if Store/load subsequently fails.
                    Ok(std::mem::replace(selected, ptr::null_mut()))
                },
            )
        }
    }
}

unsafe fn prepare_using<'a>(
    api: &'a Api,
    limits: &Limits,
    diagnostics: Option<&'a Recorder>,
    bytes: &[u8],
    expected: &str,
    engine: impl FnOnce(Strategy) -> Result<P, String>,
) -> Result<Runtime<'a>, String> {
    if hash(bytes)? != expected {
        return Err("compiled JIT bytes changed".into());
    }
    let frame = parse(bytes)?;
    if frame.fuel != limits.fuel
        || frame.memory != limits.memory
        || limits.table_elements.is_some()
        || limits.tables.is_some()
        || limits.memories.is_some()
    {
        return Err("compiled JIT execution settings mismatch".into());
    }
    Runtime::prepare_module_using(
        api,
        limits,
        diagnostics,
        || engine(frame.strategy),
        |engine, module| {
            // This interval measures code preparation, including cached loading.
            let _phase =
                diagnostics.map(|d| d.span(crate::execution_control::WorkerTracePhase::Compile));
            unsafe {
                let deserialize: unsafe extern "C" fn(P, *const u8, usize, *mut P) -> P =
                    compiled_module::symbol(api, c"wasmtime_module_deserialize")?;
                api.check(
                    deserialize(engine, frame.code.as_ptr(), frame.code.len(), module),
                    ptr::null_mut(),
                )
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_frames_and_binding_changes_are_rejected_without_loading_code() {
        // This synthetic frame tests syntax only and is never deserialized.
        let mut bytes = Vec::from(MAGIC.as_slice());
        bytes.push(2);
        bytes.extend_from_slice(&10u64.to_le_bytes());
        bytes.extend_from_slice(&65536u64.to_le_bytes());
        bytes.extend_from_slice("a".repeat(64).as_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.push(0);
        let source = "a".repeat(64);
        validate_produced(&bytes, &source, Some(10), Some(65536)).unwrap();
        assert!(validate_produced(&bytes, &"b".repeat(64), Some(10), Some(65536)).is_err());
        assert!(validate_produced(&bytes, &source, None, Some(65536)).is_err());
        assert!(validate_produced(&bytes, &source, Some(10), None).is_err());
        for n in 0..bytes.len() {
            assert!(parse(&bytes[..n]).is_err());
        }
        for offset in [0, 8, 25, 89] {
            let mut changed = bytes.clone();
            changed[offset] = 255;
            assert!(parse(&changed).is_err());
        }
        let mut overflow = bytes.clone();
        overflow[17..25].fill(255);
        assert!(parse(&overflow).is_err());
        bytes.push(0);
        assert!(parse(&bytes).is_err());
    }
}
