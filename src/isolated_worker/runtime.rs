//! Pinned Wasmtime50.0.1 full x64 C API. Host worker and developer probe entrypoints.
use super::worker_trace::Recorder;
use crate::execution_control::WorkerTracePhase;
mod runtime_image;
pub(super) use runtime_image::VerifiedRuntimeFile;
pub(super) const MAX_STREAMED_MODULE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const JIT_READY: &[u8] = b"DIY-JIT-READY-v1\0";
#[derive(Clone)]
struct Limits {
    fuel: Option<u64>,
    memory: Option<i64>,
    table_elements: Option<i64>,
    tables: Option<i64>,
    memories: Option<i64>,
}

/// Fixed developer probe compatibility. Product workers use explicit Host limits.
pub fn run_probe() -> Result<(), String> {
    run(
        std::env::args_os().skip(1).collect(),
        Limits {
            fuel: Some(50_000_000),
            memory: Some(128 * 1024 * 1024),
            table_elements: Some(10000),
            tables: Some(1),
            memories: Some(1),
        },
        None,
        None,
        None,
        ModuleMode::Jit {
            acknowledge_preparation: false,
        },
    )
}
/// stdin-only worker, refusing ordinary-privilege invocation.
pub fn run_worker() -> Result<(), String> {
    require_appcontainer()?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    worker_with_args(args)
}
fn require_appcontainer() -> Result<(), String> {
    let mut token = std::ptr::null_mut();
    super::ok(
        unsafe {
            super::OpenProcessToken(super::GetCurrentProcess(), super::TOKEN_QUERY, &mut token)
        },
        "open Worker own token",
    )?;
    let token = super::OwnedHandle::new(token, "Worker own token")?;
    let mut container = 0u32;
    let mut size = 0;
    super::ok(
        unsafe {
            super::GetTokenInformation(
                token.0,
                super::TokenIsAppContainer,
                std::ptr::from_mut(&mut container).cast(),
                4,
                &mut size,
            )
        },
        "Worker own AppContainer query",
    )?;
    if container != 1 {
        return Err("OS isolation absent; refusing Worker invocation".into());
    }
    Ok(())
}
#[test]
fn private_runtime_bootstrap_rejects_invalid_and_aliased_handles_before_loading() {
    for handles in [
        "0:0",
        "1:1",
        "18446744073709551615:0",
        "text:0",
        "1:text",
        "1:2:3",
    ] {
        let args = ["runtime", "module", "1", "none", "none"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .chain([format!("--verified-runtime-file={handles}").into()])
            .collect();
        let error = worker_with_args(args).unwrap_err();
        assert!(error.contains("invalid"), "unexpected rejection: {error}");
    }
}

fn worker_with_args(args: Vec<std::ffi::OsString>) -> Result<(), String> {
    if args
        .first()
        .is_some_and(|arg| arg == "--retained-resources-v1")
    {
        return retained_module::run(&args[1..]);
    }
    let (args, mode) = {
        let mut args = args;
        let flag = args.first().and_then(|value| value.to_str()).unwrap_or("");
        let mode = if flag == "--compile-official" {
            ModuleMode::Compile
        } else if flag == "--compile-jit-v1" {
            ModuleMode::CompileJit
        } else if let Some(digest) = flag.strip_prefix("--prepare-compiled-jit-v1=") {
            ModuleMode::PreparedCompiled(digest.to_owned())
        } else if let Some(digest) = flag.strip_prefix("--prepare-compiled-jit-pipe-v1=") {
            ModuleMode::PreparedCompiledPipe(digest.to_owned())
        } else if flag == "--prepare-jit-pipe-v1" {
            ModuleMode::JitPipe
        } else if flag == "--resource-ranges-v1" {
            ModuleMode::ResourceRanges
        } else if flag == "--build-resources-v1" {
            ModuleMode::ResourceBuild
        } else if let Some(digest) = flag.strip_prefix("--compiled-official=") {
            ModuleMode::Reuse(digest.to_owned())
        } else {
            ModuleMode::Jit {
                acknowledge_preparation: flag == "--prepare-jit-v1",
            }
        };
        if !matches!(
            mode,
            ModuleMode::Jit {
                acknowledge_preparation: false
            }
        ) {
            args.remove(0);
        }
        (args, mode)
    };
    if args.len() != 5 && args.len() != 6 {
        return Err("expected runtime module module-length fuel memory".into());
    }
    let length = args[2]
        .to_str()
        .ok_or("invalid module length")?
        .parse::<usize>()
        .map_err(|_| "invalid module length")?;
    let parse = |v: &std::ffi::OsStr| -> Result<Option<u64>, String> {
        if v == "none" {
            Ok(None)
        } else {
            v.to_str()
                .ok_or("invalid runtime limit")?
                .parse::<u64>()
                .ok()
                .filter(|v| *v > 0)
                .map(Some)
                .ok_or("invalid runtime limit".into())
        }
    };
    let fuel = parse(&args[3])?;
    let memory = parse(&args[4])?
        .map(i64::try_from)
        .transpose()
        .map_err(|_| "runtime memory exceeds i64")?;
    // Only the trusted Parent supplies this inherited, allowlisted private pipe.
    // Failure to obtain diagnostics cannot change the guest result.
    let extension = args.get(5).and_then(|arg| arg.to_str());
    let mut verified_runtime = None;
    let diagnostic_handle = if let Some(value) =
        extension.and_then(|arg| arg.strip_prefix("--verified-runtime-file="))
    {
        let (file, diagnostic) = value.split_once(':').ok_or("invalid runtime file proof")?;
        let file = file
            .parse::<usize>()
            .map_err(|_| "invalid runtime file handle")?;
        let diagnostic = diagnostic
            .parse::<usize>()
            .map_err(|_| "invalid diagnostic handle")?;
        if file == 0 || file == usize::MAX || diagnostic == usize::MAX || file == diagnostic {
            return Err("invalid or aliased runtime file handles".into());
        }
        use std::os::windows::io::FromRawHandle;
        // SAFETY: private Native bootstrap from the trusted Parent. This file
        // is explicitly allowlisted, read-only and already pinned/fully hashed;
        // no guest/package protocol can choose it. Keep it until run returns.
        verified_runtime =
            Some(unsafe { std::fs::File::from_raw_handle(file as *mut std::ffi::c_void) });
        Some(diagnostic)
    } else {
        extension
            .and_then(|arg| arg.strip_prefix("--diagnostics="))
            .and_then(|value| value.parse::<usize>().ok())
    };
    let diagnostics = diagnostic_handle
        .filter(|handle| *handle != 0 && *handle != usize::MAX)
        .map(|handle| {
            use std::os::windows::io::FromRawHandle;
            Recorder::new(unsafe {
                std::fs::File::from_raw_handle(handle as *mut std::ffi::c_void)
            })
        });
    run(
        vec![args[0].clone(), args[1].clone(), "-".into()],
        Limits {
            fuel,
            memory,
            table_elements: None,
            tables: None,
            memories: None,
        },
        Some(length),
        diagnostics.as_ref(),
        verified_runtime.as_ref(),
        mode,
    )
}
use std::{
    ffi::{CStr, OsStr, c_void},
    io::Read,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr,
};
type P = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Handle {
    store: u64,
    private: usize,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Func {
    store: u64,
    private: P,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StoredMemory {
    store: u64,
    private1: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Memory {
    stored: StoredMemory,
    private2: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
union ExternPayload {
    function: Func,
    memory: Memory,
    storage: [u64; 3],
}
impl Default for ExternPayload {
    fn default() -> Self {
        Self { storage: [0; 3] }
    }
}
#[repr(C)]
#[derive(Default)]
struct Extern {
    kind: u8,
    of: ExternPayload,
}
// Full distribution GC payload is24bytes. Inactive fields may be uninitialized.
#[repr(C)]
#[derive(Clone, Copy)]
union ValPayload {
    i32: i32,
    i64: i64,
    storage: [u64; 3],
}
impl Default for ValPayload {
    fn default() -> Self {
        Self { storage: [0; 3] }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Val {
    kind: u8,
    of: ValPayload,
}
impl Val {
    fn i32(value: u32) -> Self {
        Self {
            kind: 0,
            of: ValPayload { i32: value as i32 },
        }
    }
}
#[repr(C)]
struct Bytes {
    size: usize,
    data: *mut u8,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(file: *const u16, reserved: P, flags: u32) -> P;
    fn GetProcAddress(module: P, name: *const u8) -> P;
}
#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptOpenAlgorithmProvider(
        handle: *mut P,
        algorithm: *const u16,
        implementation: *const u16,
        flags: u32,
    ) -> i32;
    fn BCryptHash(
        handle: P,
        secret: *const u8,
        secret_size: u32,
        input: *const u8,
        input_size: u32,
        output: *mut u8,
        output_size: u32,
    ) -> i32;
    fn BCryptCloseAlgorithmProvider(handle: P, flags: u32) -> i32;
}
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
fn verify_runtime(path: &Path) -> Result<Option<runtime_image::ReadOnlyImage>, String> {
    if cfg!(feature = "copied-runtime-verification-baseline") {
        let bytes = std::fs::read(path).map_err(|e| format!("runtime read: {e}"))?;
        verify_runtime_bytes(&bytes)?;
        Ok(None)
    } else {
        let image = runtime_image::ReadOnlyImage::open(path)?;
        verify_runtime_bytes(image.bytes())?;
        Ok(Some(image))
    }
}
fn verify_runtime_bytes(bytes: &[u8]) -> Result<(), String> {
    let size = u32::try_from(bytes.len()).map_err(|_| "runtime file too large")?;
    let mut algorithm = ptr::null_mut();
    let mut digest = [0u8; 32];
    let name = wide(OsStr::new("SHA256"));
    // SAFETY: initialized handles/buffers, system CNG SHA256 provider, exact sizes.
    unsafe {
        if BCryptOpenAlgorithmProvider(&mut algorithm, name.as_ptr(), ptr::null(), 0) != 0 {
            return Err("SHA256 provider unavailable".into());
        }
        let status = BCryptHash(
            algorithm,
            ptr::null(),
            0,
            bytes.as_ptr(),
            size,
            digest.as_mut_ptr(),
            32,
        );
        BCryptCloseAlgorithmProvider(algorithm, 0);
        if status != 0 {
            return Err("SHA256 calculation failed".into());
        }
    }
    let hash = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if hash != "fa7fdafc1dcb98ff20acd6ea480f305b90b7a8f518cb35110904ef345517e4ef" {
        return Err("runtime does not match pinned official 50.0.1 full x64 DLL".into());
    }
    Ok(())
}
struct Library(P);
// Process-owned: runtime global handlers/threads may outlive an Engine.
// Keep the DLL resident; the OS releases it at genuine process exit.
macro_rules! api {
  ($($field:ident:[$ty:ty] => $name:literal),+ $(,)?)=>{
   struct Api {_library:Library,$($field:$ty),+}
   impl Api {fn load(path:&Path)->Result<Self,String>{
    // Retain the read-only mapping and its deny-write/delete file handle
    // through LoadLibrary and symbol resolution, not just through hashing.
    let _verified_image = verify_runtime(path)?;
    // SAFETY: this is the verified pinned runtime in a Host-owned code snapshot.
    unsafe { Self::load_verified(path) }
   }
   unsafe fn load_verified(path:&Path)->Result<Self,String>{
    let name=wide(path.as_os_str());
    // SAFETY: verified full official DLL; explicit absolute path and restricted dependency search.
    let library=Library(unsafe{LoadLibraryExW(name.as_ptr(),ptr::null_mut(),0x1100)});
    if library.0.is_null(){return Err("cannot load verified Wasmtime DLL".into());}
    $(let symbol=unsafe{GetProcAddress(library.0,concat!($name,"\0").as_ptr())};if symbol.is_null(){return Err(format!("runtime API missing: {}",$name));}
    // SAFETY: exact function signature from pinned release headers; library outlives every call.
    let $field=unsafe{std::mem::transmute::<P,$ty>(symbol)};)+
    Ok(Self{_library:library,$($field),+})
   }}
  }
 }
api! {
 config_new:[unsafe extern "C" fn()->P] =>"wasm_config_new",
 fuel_enable:[unsafe extern "C" fn(P,bool)] =>"wasmtime_config_consume_fuel_set",
 opt_level:[unsafe extern "C" fn(P,u8)] =>"wasmtime_config_cranelift_opt_level_set",
 engine_new:[unsafe extern "C" fn(P)->P] =>"wasm_engine_new_with_config",
 engine_delete:[unsafe extern "C" fn(P)] =>"wasm_engine_delete",
 store_new:[unsafe extern "C" fn(P,P,Option<unsafe extern "C" fn(P)>)->P] =>"wasmtime_store_new",
 store_context:[unsafe extern "C" fn(P)->P] =>"wasmtime_store_context",
 limiter:[unsafe extern "C" fn(P,i64,i64,i64,i64,i64)] =>"wasmtime_store_limiter",
 store_delete:[unsafe extern "C" fn(P)] =>"wasmtime_store_delete",
 fuel_set:[unsafe extern "C" fn(P,u64)->P] =>"wasmtime_context_set_fuel",
 module_new:[unsafe extern "C" fn(P,*const u8,usize,*mut P)->P] =>"wasmtime_module_new",
 module_delete:[unsafe extern "C" fn(P)] =>"wasmtime_module_delete",
 instantiate:[unsafe extern "C" fn(P,P,*const Extern,usize,*mut Handle,*mut P)->P] =>"wasmtime_instance_new",
 export:[unsafe extern "C" fn(P,*const Handle,*const u8,usize,*mut Extern)->bool] =>"wasmtime_instance_export_get",
 extern_delete:[unsafe extern "C" fn(*mut Extern)] =>"wasmtime_extern_delete",
 call:[unsafe extern "C" fn(P,*const Func,*const Val,usize,*mut Val,usize,*mut P)->P] =>"wasmtime_func_call",
 unroot:[unsafe extern "C" fn(*mut Val)] =>"wasmtime_val_unroot",
 memory_data:[unsafe extern "C" fn(P,*const Memory)->*mut u8] =>"wasmtime_memory_data",
 memory_size:[unsafe extern "C" fn(P,*const Memory)->usize] =>"wasmtime_memory_data_size",
 error_message:[unsafe extern "C" fn(P,*mut Bytes)] =>"wasmtime_error_message",
 error_delete:[unsafe extern "C" fn(P)] =>"wasmtime_error_delete",
 trap_message:[unsafe extern "C" fn(P,*mut Bytes)] =>"wasm_trap_message",
 trap_delete:[unsafe extern "C" fn(P)] =>"wasm_trap_delete",
 bytes_delete:[unsafe extern "C" fn(*mut Bytes)] =>"wasm_byte_vec_delete",
 wat:[unsafe extern "C" fn(*const u8,usize,*mut Bytes)->P] =>"wasmtime_wat2wasm",
}
impl Api {
    fn check(&self, error: P, trap: P) -> Result<(), String> {
        if error.is_null() && trap.is_null() {
            return Ok(());
        }
        let mut bytes = Bytes {
            size: 0,
            data: ptr::null_mut(),
        };
        // SAFETY: error/trap and owned byte-vector lifetimes follow pinned C API ownership.
        let message = unsafe {
            if !error.is_null() {
                (self.error_message)(error, &mut bytes);
            } else {
                (self.trap_message)(trap, &mut bytes);
            }
            let message = if bytes.data.is_null() {
                "runtime failure".to_string()
            } else {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    bytes.data,
                    bytes.size.min(4096),
                ))
                .into_owned()
            };
            (self.bytes_delete)(&mut bytes);
            if !error.is_null() {
                (self.error_delete)(error);
            }
            if !trap.is_null() {
                (self.trap_delete)(trap);
            }
            message
        };
        Err(message)
    }
}
struct Runtime<'a> {
    api: &'a Api,
    engine: P,
    store: P,
    module: P,
    context: P,
    instance: Handle,
    fuel: Option<u64>,
    diagnostics: Option<&'a Recorder>,
    resource: Option<Box<resource_import::Bridge<'a>>>,
    owns_code: bool,
}
impl Drop for Runtime<'_> {
    fn drop(&mut self) {
        let _phase = self.diagnostics.map(|d| d.span(WorkerTracePhase::Cleanup));
        unsafe {
            if !self.store.is_null() {
                (self.api.store_delete)(self.store);
            }
            if self.owns_code && !self.module.is_null() {
                (self.api.module_delete)(self.module);
            }
            if self.owns_code && !self.engine.is_null() {
                (self.api.engine_delete)(self.engine);
            }
        }
    }
}
impl<'a> Runtime<'a> {
    fn prepare_jit(
        api: &'a Api,
        module: &[u8],
        limits: &Limits,
        diagnostics: Option<&'a Recorder>,
    ) -> Result<Self, String> {
        let load = |engine, result: &mut P| {
            let _compile = diagnostics.map(|d| d.span(WorkerTracePhase::Compile));
            // SAFETY: the module bytes remain live until compilation returns.
            unsafe {
                api.check(
                    (api.module_new)(engine, module.as_ptr(), module.len(), result),
                    ptr::null_mut(),
                )
            }
        };
        Self::prepare_module_using(
            api,
            limits,
            diagnostics,
            || create_winch_engine(api, limits),
            load,
        )
        .or_else(|_| Self::prepare_module(api, limits, diagnostics, load))
    }

    #[cfg(test)]
    fn new(
        api: &'a Api,
        module: &[u8],
        limits: &Limits,
        diagnostics: Option<&'a Recorder>,
    ) -> Result<Self, String> {
        let mut result = Self::prepare_jit(api, module, limits, diagnostics)?;
        // Instantiation can run a start function. Keep it outside the retry.
        result.instantiate()?;
        Ok(result)
    }

    fn with_module(
        api: &'a Api,
        limits: &Limits,
        diagnostics: Option<&'a Recorder>,
        load: impl FnOnce(P, &mut P) -> Result<(), String>,
    ) -> Result<Self, String> {
        let mut result = Self::prepare_module(api, limits, diagnostics, load)?;
        result.instantiate()?;
        Ok(result)
    }
    fn prepare_module(
        api: &'a Api,
        limits: &Limits,
        diagnostics: Option<&'a Recorder>,
        load: impl FnOnce(P, &mut P) -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::prepare_module_using(
            api,
            limits,
            diagnostics,
            || create_engine(api, limits),
            load,
        )
    }

    fn prepare_module_using(
        api: &'a Api,
        limits: &Limits,
        diagnostics: Option<&'a Recorder>,
        engine: impl FnOnce() -> Result<P, String>,
        load: impl FnOnce(P, &mut P) -> Result<(), String>,
    ) -> Result<Self, String> {
        let engine_phase = diagnostics.map(|d| d.span(WorkerTracePhase::EngineSetup));
        let engine = engine()?;
        let mut result = Self {
            api,
            engine,
            store: ptr::null_mut(),
            module: ptr::null_mut(),
            context: ptr::null_mut(),
            instance: Handle::default(),
            fuel: limits.fuel,
            // A failed preparation is already covered by its preparation phases.
            diagnostics: None,
            resource: None,
            owns_code: true,
        };
        result.install_store(limits)?;
        drop(engine_phase);
        load(engine, &mut result.module)?;
        result.diagnostics = diagnostics;
        Ok(result)
    }
    fn install_store(&mut self, limits: &Limits) -> Result<(), String> {
        debug_assert!(self.store.is_null());
        // SAFETY: the Store is created on this Runtime's Engine.
        unsafe {
            self.store = (self.api.store_new)(self.engine, ptr::null_mut(), None);
            if self.store.is_null() {
                return Err("store unavailable".into());
            }
            (self.api.limiter)(
                self.store,
                limits.memory.unwrap_or(-1),
                limits.table_elements.unwrap_or(-1),
                1,
                limits.tables.unwrap_or(-1),
                limits.memories.unwrap_or(-1),
            );
            self.context = (self.api.store_context)(self.store);
            if let Some(fuel) = limits.fuel {
                self.api
                    .check((self.api.fuel_set)(self.context, fuel), ptr::null_mut())?;
            }
        }
        Ok(())
    }
    fn instantiate(&mut self) -> Result<(), String> {
        let _instantiate = self
            .diagnostics
            .map(|d| d.span(WorkerTracePhase::Instantiate));
        // SAFETY: the Module and Store belong to the same Engine.
        unsafe {
            let mut trap = ptr::null_mut();
            let error = (self.api.instantiate)(
                self.context,
                self.module,
                ptr::null(),
                0,
                &mut self.instance,
                &mut trap,
            );
            self.api.check(error, trap)
        }
    }
    fn discard_store(&mut self) {
        // SAFETY: this is the Store owned by self. No guest handles survive it.
        unsafe { (self.api.store_delete)(self.store) };
        self.store = ptr::null_mut();
        self.context = ptr::null_mut();
        self.instance = Handle::default();
    }
    fn export(&self, name: &CStr, kind: u8) -> Result<ExternPayload, String> {
        let mut export = Extern::default();
        // SAFETY: live instance/context in the same Store; output uses pinned header layout.
        let found = unsafe {
            (self.api.export)(
                self.context,
                &self.instance,
                name.as_ptr().cast(),
                name.to_bytes().len(),
                &mut export,
            )
        };
        if !found {
            return Err("guest export missing".into());
        }
        let result = if export.kind == kind {
            Ok(export.of)
        } else {
            Err("guest export has wrong type".into())
        };
        unsafe {
            (self.api.extern_delete)(&mut export);
        }
        result
    }
    fn call(&mut self, name: &CStr, args: &[Val], result_kind: Option<u8>) -> Result<u64, String> {
        // SAFETY: export verified the function discriminant.
        let function = unsafe { self.export(name, 0)?.function };
        let mut result = Val::default();
        let mut trap = ptr::null_mut();
        // SAFETY: checked call verifies arity/types; buffers have their exact lengths.
        unsafe {
            if let Some(fuel) = self.fuel {
                self.api
                    .check((self.api.fuel_set)(self.context, fuel), ptr::null_mut())?;
            }
            let error = (self.api.call)(
                self.context,
                &function,
                args.as_ptr(),
                args.len(),
                &mut result,
                usize::from(result_kind.is_some()),
                &mut trap,
            );
            self.api.check(error, trap)?;
            let valid = result_kind.is_none_or(|kind| kind == result.kind);
            // SAFETY: read only the field selected by the checked result kind.
            // In particular, never read an i32 result as a wider integer.
            let value = match (valid, result_kind) {
                (true, Some(0)) => u64::from(result.of.i32 as u32),
                (true, Some(1)) => result.of.i64 as u64,
                _ => 0,
            };
            if result_kind.is_some() {
                (self.api.unroot)(&mut result);
            }
            if valid {
                Ok(value)
            } else {
                Err("guest result has wrong type".into())
            }
        }
    }
    fn range(&self, memory: &Memory, offset: u32, length: usize) -> Result<*mut u8, String> {
        if length > 8 * 1024 * 1024 {
            return Err("guest buffer exceeds wire limit".into());
        }
        // Reacquire the base after each guest call, which may grow or relocate memory.
        let size = unsafe { (self.api.memory_size)(self.context, memory) };
        let offset = offset as usize;
        let end = offset.checked_add(length).ok_or("guest range overflow")?;
        if end > size {
            return Err("guest buffer out of bounds".into());
        }
        let data = unsafe { (self.api.memory_data)(self.context, memory) };
        if data.is_null() {
            return Err("guest memory unavailable".into());
        }
        Ok(unsafe { data.add(offset) })
    }
    fn invoke(&mut self, input: &[u8]) -> Result<Vec<u8>, String> {
        let setup_phase = self
            .diagnostics
            .map(|d| d.span(WorkerTracePhase::GuestSetup));
        if input.is_empty() || input.len() > 8 * 1024 * 1024 {
            return Err("invalid request size".into());
        }
        if self.call(c"diyime_api_version", &[], Some(0))? != 1 {
            return Err("guest version unsupported".into());
        }
        // SAFETY: export verified the memory discriminant.
        let memory = unsafe { self.export(c"memory", 3)?.memory };
        let arg = Val::i32(input.len() as u32);
        let pointer = self.call(c"diyime_alloc", &[arg], Some(0))? as u32;
        if pointer == 0 {
            return Err("guest allocation rejected".into());
        }
        let target = self.range(&memory, pointer, input.len())?;
        // SAFETY: checked guest memory range, no concurrent guest calls, source is independent Host bytes.
        unsafe {
            ptr::copy_nonoverlapping(input.as_ptr(), target, input.len());
        }
        drop(setup_phase);
        let call_phase = self
            .diagnostics
            .map(|d| d.span(WorkerTracePhase::GuestCall));
        let reply = self.call(c"diyime_call", &[Val::i32(pointer), arg], Some(1))?;
        drop(call_phase);
        let _reply = self
            .diagnostics
            .map(|d| d.span(WorkerTracePhase::ReplyRead));
        let length = (reply >> 32) as usize;
        let pointer = reply as u32;
        if pointer == 0 || length == 0 {
            return Err("guest returned empty descriptor".into());
        }
        let data = self.range(&memory, pointer, length)?;
        let bytes = unsafe { std::slice::from_raw_parts(data, length) }.to_vec();
        self.call(c"diyime_release_reply", &[], None)?;
        Ok(bytes)
    }
}
fn create_engine(api: &Api, limits: &Limits) -> Result<P, String> {
    create_engine_configured(api, limits, |_| {})
}

#[cfg(target_arch = "x86_64")]
fn create_winch_engine(api: &Api, limits: &Limits) -> Result<P, String> {
    if !std::is_x86_feature_detected!("avx") {
        return Err("Winch CPU unsupported".into());
    }
    // SAFETY: the symbol is looked up in the verified, process-owned DLL.
    let symbol = unsafe {
        GetProcAddress(
            api._library.0,
            c"wasmtime_config_strategy_set".as_ptr().cast(),
        )
    };
    if symbol.is_null() {
        return Err("Winch strategy API unavailable".into());
    }
    // SAFETY: the pinned C header declares wasm_config_t* and uint8_t.
    let set_strategy = unsafe { std::mem::transmute::<P, unsafe extern "C" fn(P, u8)>(symbol) };
    create_engine_configured(api, limits, |config| unsafe { set_strategy(config, 2) })
}

#[cfg(not(target_arch = "x86_64"))]
fn create_winch_engine(_: &Api, _: &Limits) -> Result<P, String> {
    Err("Winch architecture unsupported".into())
}

fn create_engine_configured(
    api: &Api,
    limits: &Limits,
    configure: impl FnOnce(P),
) -> Result<P, String> {
    // SAFETY: the fresh config is owned by this function until engine creation consumes it.
    unsafe {
        let config = (api.config_new)();
        if config.is_null() {
            return Err("config unavailable".into());
        }
        (api.fuel_enable)(config, limits.fuel.is_some());
        // WASMTIME_OPT_LEVEL_NONE (pinned config.h).
        (api.opt_level)(config, 0);
        configure(config);
        let engine = (api.engine_new)(config);
        if engine.is_null() {
            return Err("engine unavailable".into());
        }
        Ok(engine)
    }
}

#[cfg(test)]
pub(super) mod compiled_jit_probe;
#[cfg(test)]
mod compiled_module_probe;
enum ModuleMode {
    Jit { acknowledge_preparation: bool },
    JitPipe,
    ResourceRanges,
    ResourceBuild,
    Compile,
    CompileJit,
    PreparedCompiled(String),
    PreparedCompiledPipe(String),
    Reuse(String),
}

pub(super) mod compiled_jit;
mod compiled_module;
mod resource_import;
mod retained_module;
pub(super) use compiled_module::OFFICIAL_WASM_HASH;
#[cfg(test)]
pub(in crate::isolated_worker) use retained_module::state_fixture;

fn run(
    args: Vec<std::ffi::OsString>,
    limits: Limits,
    expected_module: Option<usize>,
    diagnostics: Option<&Recorder>,
    verified_runtime: Option<&std::fs::File>,
    mode: ModuleMode,
) -> Result<(), String> {
    if args.len() != 3 {
        return Err(
            "usage: wasm_c_api_probe <verified-runtime-dll> <module.wasm> <request.json|->".into(),
        );
    }
    // Parent selected the absolute cache asset; do not query inaccessible ancestor metadata here.
    let runtime = Path::new(&args[0]);
    if !runtime.is_absolute() {
        return Err("runtime path must be absolute".into());
    }
    let dll_phase = diagnostics.map(|d| d.span(WorkerTracePhase::DllLoad));
    let api = if let Some(verified) = verified_runtime {
        let _same_file = runtime_image::lock_matching_file(runtime, verified)?;
        // SAFETY: the trusted Parent hashed and continuously pinned this exact
        // immutable file. A different path target or mutable file was refused.
        // The original inherited file remains owned throughout this run.
        unsafe { Api::load_verified(runtime)? }
    } else {
        Api::load(runtime)?
    };
    drop(dll_phase);
    let module_phase = diagnostics.map(|d| d.span(WorkerTracePhase::ModuleRead));
    let limit = expected_module.unwrap_or(64 * 1024 * 1024);
    let streamed = matches!(
        mode,
        ModuleMode::JitPipe | ModuleMode::PreparedCompiledPipe(_)
    );
    let mut module = if streamed {
        let length = expected_module
            .filter(|&n| (1..=MAX_STREAMED_MODULE_BYTES).contains(&n))
            .ok_or("invalid streamed module length")?;
        let mut bytes = vec![0; length];
        std::io::stdin()
            .lock()
            .read_exact(&mut bytes)
            .map_err(|_| "module pipe closed before full payload")?;
        bytes
    } else {
        let mut bytes = Vec::new();
        std::fs::File::open(&args[1])
            .map_err(|e| format!("module open: {e}"))?
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| "module read failed")?;
        bytes
    };
    if module.len() > limit || expected_module.is_some_and(|n| n != module.len()) {
        return Err("module size mismatch".into());
    }
    if !streamed && Path::new(&args[1]).extension().is_some_and(|e| e == "wat") {
        let mut bytes = Bytes {
            size: 0,
            data: ptr::null_mut(),
        };
        // SAFETY: pinned runtime owns the output byte-vector; input bytes remain live.
        unsafe {
            api.check(
                (api.wat)(module.as_ptr(), module.len(), &mut bytes),
                ptr::null_mut(),
            )?;
            module = if bytes.size == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(bytes.data, bytes.size).to_vec()
            };
            (api.bytes_delete)(&mut bytes);
        }
    }
    if expected_module.is_none() && module.len() > 64 * 1024 * 1024 {
        return Err("module exceeds probe limit".into());
    }
    drop(module_phase);
    if matches!(mode, ModuleMode::ResourceRanges | ModuleMode::ResourceBuild) {
        if expected_module.is_none() || args[2] != "-" {
            return Err("resource transport requires a Host-owned invocation".into());
        }
        let building = matches!(mode, ModuleMode::ResourceBuild);
        let stdin = std::io::stdin().lock();
        let stdout = std::io::stdout().lock();
        let (mut client, input) = if building {
            super::resource_transport::Client::start_build(stdin, stdout)
        } else {
            super::resource_transport::Client::start(stdin, stdout)
        }
        .map_err(|_| "resource start failed")?;
        let reply = {
            let mut guest = resource_import::instantiate(
                &api,
                &module,
                &limits,
                diagnostics,
                &mut client,
                building,
            )?;
            let reply = guest.invoke(&input)?;
            if guest
                .resource
                .as_ref()
                .is_some_and(|bridge| bridge.failed())
            {
                return Err("resource read failed".into());
            }
            reply
        };
        return client
            .finish(&reply)
            .map_err(|_| "resource reply failed".into());
    }
    if matches!(mode, ModuleMode::Compile) {
        use std::io::Write;
        let artifact = compiled_module::compile(&api, &limits, &module)?;
        return std::io::stdout()
            .lock()
            .write_all(&artifact)
            .map_err(|_| "compiler pipe failed".into());
    }
    if matches!(mode, ModuleMode::CompileJit) {
        use std::io::Write;
        let artifact = compiled_jit::compile(&api, &limits, &module)?;
        return std::io::stdout()
            .lock()
            .write_all(&artifact)
            .map_err(|_| "JIT compiler pipe failed".into());
    }
    // Prepare only the Module and Store. Instantiation can run guest start code
    // and must wait for the Host-owned input.
    let prepared_jit = if matches!(
        mode,
        ModuleMode::Jit { .. }
            | ModuleMode::JitPipe
            | ModuleMode::PreparedCompiled(_)
            | ModuleMode::PreparedCompiledPipe(_)
    ) {
        let prepared = if let ModuleMode::PreparedCompiled(digest)
        | ModuleMode::PreparedCompiledPipe(digest) = &mode
        {
            // SAFETY: only the trusted Parent chooses this private Native mode
            // and supplies its own compiler-produced immutable file and digest.
            unsafe { compiled_jit::prepare(&api, &limits, diagnostics, &module, digest)? }
        } else {
            Runtime::prepare_jit(&api, &module, &limits, diagnostics)?
        };
        if matches!(
            mode,
            ModuleMode::Jit {
                acknowledge_preparation: true
            } | ModuleMode::JitPipe
                | ModuleMode::PreparedCompiled(_)
                | ModuleMode::PreparedCompiledPipe(_)
        ) {
            use std::io::Write;
            let mut output = std::io::stdout().lock();
            output
                .write_all(JIT_READY)
                .and_then(|()| output.flush())
                .map_err(|_| "JIT readiness pipe failed")?;
        }
        Some(prepared)
    } else {
        None
    };
    let input_phase = diagnostics.map(|d| d.span(WorkerTracePhase::InputRead));
    let input = if args[2] == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "request pipe failed")?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("request exceeds wire limit".into());
        }
        bytes
    } else {
        std::fs::read(&args[2]).map_err(|e| format!("request read: {e}"))?
    };
    drop(input_phase);
    let mut guest = match mode {
        ModuleMode::Jit { .. }
        | ModuleMode::JitPipe
        | ModuleMode::PreparedCompiled(_)
        | ModuleMode::PreparedCompiledPipe(_) => {
            if input.is_empty() {
                return Err("empty request before guest instantiation".into());
            }
            let mut guest = prepared_jit.expect("JIT module prepared before reading input");
            guest.instantiate()?;
            guest
        }
        ModuleMode::Reuse(digest) => {
            compiled_module::load(&api, &limits, diagnostics, &module, &digest)?
        }
        ModuleMode::Compile | ModuleMode::CompileJit => {
            unreachable!("compile returns before guest input")
        }
        ModuleMode::ResourceRanges => unreachable!("resource transport returns before raw input"),
        ModuleMode::ResourceBuild => unreachable!("resource build returns before raw input"),
    };
    if expected_module.is_some() {
        // Production carries the exact bounded guest wire to Parent validation.
        // Parsing/re-encoding here would erase duplicate object keys before the
        // trusted Host can reject them. Developer batch display stays below.
        use std::io::Write;
        let reply = guest.invoke(&input)?;
        let _write = diagnostics.map(|d| d.span(WorkerTracePhase::ReplyWrite));
        let mut output = std::io::stdout().lock();
        output.write_all(&reply).map_err(|_| "reply pipe failed")?;
        output.flush().map_err(|_| "reply pipe failed")?;
        return Ok(());
    }
    let requests = if input.iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'[') {
        let items: Vec<serde_json::Value> =
            serde_json::from_slice(&input).map_err(|_| "invalid probe request batch")?;
        if items.len() > 1000 {
            return Err("probe batch too large".into());
        }
        Some(items)
    } else {
        None
    };
    let value = if let Some(items) = requests {
        let mut output = Vec::new();
        for item in items {
            let input = serde_json::to_vec(&item).map_err(|e| e.to_string())?;
            let reply = guest.invoke(&input)?;
            output.push(
                serde_json::from_slice::<serde_json::Value>(&reply)
                    .map_err(|_| "guest response is not JSON")?,
            );
        }
        serde_json::Value::Array(output)
    } else {
        serde_json::from_slice::<serde_json::Value>(&guest.invoke(&input)?)
            .map_err(|_| "guest response is not JSON")?
    };
    println!(
        "{}",
        serde_json::to_string(&value).map_err(|e| e.to_string())?
    );
    Ok(())
}
#[test]
fn pinned_full_x64_c_layout() {
    assert_eq!(std::mem::size_of::<Handle>(), 16);
    assert_eq!(std::mem::size_of::<Func>(), 16);
    assert_eq!(std::mem::size_of::<Memory>(), 24);
    assert_eq!(std::mem::size_of::<Extern>(), 32);
    assert_eq!(std::mem::size_of::<Val>(), 32);
    assert_eq!(std::mem::align_of::<Val>(), 8);
}

#[test]
#[ignore = "requires the pinned Windows Wasmtime DLL"]
fn real_jit_compiler_fallback() {
    let dll = std::env::var_os("DIY_RANGE_RUNTIME_DLL").expect("pinned Wasmtime DLL");
    let api = Api::load(Path::new(&dll)).expect("load pinned Wasmtime");
    let limits = Limits {
        fuel: Some(10_000),
        memory: Some(1024 * 1024),
        table_elements: None,
        tables: None,
        memories: None,
    };
    // (module (func $callee) (func return_call $callee))
    let tail_call = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x03\x02\0\0\x0a\x09\x02\x02\0\x0b\x04\0\x12\0\x0b";
    let load = |engine, result: &mut P| unsafe {
        api.check(
            (api.module_new)(engine, tail_call.as_ptr(), tail_call.len(), result),
            ptr::null_mut(),
        )
    };
    assert!(Runtime::prepare_module(&api, &limits, None, load).is_ok());
    let winch = Runtime::prepare_module_using(
        &api,
        &limits,
        None,
        || create_winch_engine(&api, &limits),
        load,
    );
    let error = winch
        .err()
        .expect("Winch must reject the tail-call fixture");
    assert!(
        error.contains("tail calls support is not enabled")
            || matches!(
                error.as_str(),
                "Winch CPU unsupported" | "Winch architecture unsupported"
            ),
        "{error}"
    );
    let path =
        std::env::temp_dir().join(format!("diyime-jit-fallback-{}.trace", std::process::id()));
    let trace = Recorder::new(std::fs::File::create(&path).unwrap());
    let result = Runtime::new(&api, tail_call, &limits, Some(&trace));
    assert!(result.is_ok());
    drop(result);
    drop(trace);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let intervals =
        super::worker_trace::decode(&bytes).expect("fallback trace must remain readable");
    for phase in [WorkerTracePhase::Compile, WorkerTracePhase::Instantiate] {
        assert!(
            intervals
                .iter()
                .any(|interval| interval.phase == phase && interval.end.is_some())
        );
    }
    assert!(Runtime::new(&api, b"invalid", &limits, None).is_err());
}

#[test]
#[ignore = "requires a real Windows AppContainer and pinned Wasmtime DLL"]
fn real_jit_preparation_acknowledges_before_input() {
    use super::pipe::{WorkerScope, execute_observed};
    use super::{Cache, EndReason, Job, Outcome, Profile, RunControl};
    use crate::execution_control::{ExecutionHost, ResourcePolicy, ResourceUsage};
    use std::ffi::OsString;
    use std::time::Duration;

    let worker = std::env::var_os("DIY_RANGE_WORKER_BINARY").unwrap();
    let dll = std::env::var_os("DIY_RANGE_RUNTIME_DLL").unwrap();
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/contracts/execution_control.json")).unwrap();
    let policy =
        ResourcePolicy::from_value(contract["resourceProfiles"]["bounded"].clone()).unwrap();
    let host = ExecutionHost::new("prepared-jit-test", policy).unwrap();
    let mut naming_profile = Profile::new().unwrap();
    let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &naming_profile).unwrap();
    let executable = cache.copy(Path::new(&worker), "worker.exe").unwrap();
    let runtime = cache.copy(Path::new(&dll), "wasmtime.dll").unwrap();
    let wat = br#"(module
        (memory (export "memory") 1)
        (data (i32.const 32) "{}")
        (func (export "diyime_api_version") (result i32) i32.const 1)
        (func (export "diyime_alloc") (param i32) (result i32) i32.const 16)
        (func (export "diyime_call") (param i32 i32) (result i64) i64.const 8589934624)
        (func (export "diyime_release_reply")))"#;
    let module = cache.write("fixture.wat", wat).unwrap();
    naming_profile.delete().unwrap();
    let args = [
        OsString::from("--prepare-jit-v1"),
        runtime.into_os_string(),
        module.into_os_string(),
        wat.len().to_string().into(),
        "10000".into(),
        "1048576".into(),
    ];
    let repeats = if std::env::var_os("DIY_MEASURE_PREPARED_JIT").is_some() {
        101
    } else {
        1
    };
    let mut samples = Vec::new();
    for deliver in std::iter::once(false).chain(std::iter::repeat_n(true, repeats)) {
        let slot = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..ResourceUsage::default()
            })
            .unwrap();
        let mut profile = Profile::new().unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let job = Job::new().unwrap();
        let (ready, prepared) = std::sync::mpsc::sync_channel(1);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let observed_host = host.clone();
        let dispatch = std::thread::spawn(move || {
            prepared.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(observed_host.usage().physical_jobs, 1);
            let submitted = std::time::Instant::now();
            send.send(b"{}".to_vec()).unwrap();
            submitted
        });
        let control = RunControl {
            walltime: Some(Duration::from_secs(10)),
            ..RunControl::default()
        };
        let scope = WorkerScope::prepared(&profile, &job, &cache).with_jit_delivery(
            None,
            move || {
                ready.send(()).map_err(|error| error.to_string())?;
                receive
                    .recv_timeout(Duration::from_secs(5))
                    .map(|bytes| super::pipe::JitDelivery {
                        bytes,
                        run_cancel: None,
                        check: Some(Box::new(move || {
                            deliver
                                .then_some(())
                                .ok_or_else(|| "test delivery denied".into())
                        })),
                    })
                    .map_err(|error| error.to_string())
            },
            control.cancel.clone(),
        );
        let Outcome::Exited(receipt) =
            execute_observed(scope, &executable, &args, vec![], &control).unwrap()
        else {
            panic!("the original JIT worker did not exit");
        };
        assert_eq!(receipt.reason(), EndReason::Exited);
        let submitted = dispatch.join().unwrap();
        if deliver {
            assert_eq!(
                receipt.exit_code(),
                Some(0),
                "{:?}",
                receipt.output().map(String::from_utf8_lossy)
            );
            assert_eq!(receipt.output().unwrap(), b"{}");
        } else {
            assert_ne!(receipt.exit_code(), Some(0));
            assert!(!receipt.input_delivered());
            assert_eq!(receipt.output(), Err("test delivery denied"));
        }
        drop(job);
        cache.revoke_read_execute(&profile).unwrap();
        profile.delete().unwrap();
        if deliver {
            samples.push([
                receipt.signalled_at.duration_since(submitted).as_micros(),
                submitted.elapsed().as_micros(),
            ]);
        }
        assert_eq!(host.usage().physical_jobs, 1);
        drop(slot);
        assert_eq!(host.usage().physical_jobs, 0);
    }
    // These prefixes cannot reach READY or guest instantiation.
    let malformed: &[(usize, &[u8])] = &[
        (9, b"\0asm\x01\0\0\0"),
        (8, b"bad wasm"),
        (MAX_STREAMED_MODULE_BYTES + 1, b""),
    ];
    for &(declared, prefix) in malformed {
        let slot = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..ResourceUsage::default()
            })
            .unwrap();
        let mut profile = Profile::new().unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let job = Job::new().unwrap();
        let args = [
            OsString::from("--prepare-jit-pipe-v1"),
            args[1].clone(),
            "-".into(),
            declared.to_string().into(),
            "10000".into(),
            "1048576".into(),
        ];
        let scope = WorkerScope::observed(&profile, &job, &cache, Default::default());
        let control = RunControl {
            walltime: Some(Duration::from_secs(10)),
            ..RunControl::default()
        };
        let Outcome::Exited(receipt) =
            execute_observed(scope, &executable, &args, prefix.to_vec(), &control).unwrap()
        else {
            panic!("Worker receiving an invalid module did not exit");
        };
        assert_eq!(receipt.reason(), EndReason::Exited);
        assert!(receipt.exit_code().is_some_and(|code| code != 0));
        let output = receipt.output().unwrap();
        assert!(output.starts_with(b"wasm-worker: "), "{output:?}");
        assert!(
            !output
                .windows(JIT_READY.len())
                .any(|frame| frame == JIT_READY)
        );
        drop(job);
        cache.revoke_read_execute(&profile).unwrap();
        profile.delete().unwrap();
        drop(slot);
        assert_eq!(host.usage().physical_jobs, 0);
    }
    for file in ["worker.exe", "wasmtime.dll", "fixture.wat"] {
        super::adapter::remove_asset(&cache.path().join(file)).unwrap();
    }
    std::fs::remove_dir(cache.path()).unwrap();
    if repeats > 1 {
        for (index, phase) in ["dispatch-to-exit-confirmed", "dispatch-to-recovered"]
            .into_iter()
            .enumerate()
        {
            let mut times: Vec<_> = samples[1..].iter().map(|sample| sample[index]).collect();
            times.sort_unstable();
            println!(
                "{phase}: p50={}us p95={}us p99={}us n={}",
                times[49],
                times[94],
                times[98],
                times.len()
            );
        }
    }
}

#[test]
#[ignore = "requires the pinned Windows Wasmtime DLL"]
fn real_jit_traps_are_not_retried() {
    let dll = std::env::var_os("DIY_RANGE_RUNTIME_DLL").expect("pinned Wasmtime DLL");
    let api = Api::load(Path::new(&dll)).expect("load pinned Wasmtime");
    let limits = Limits {
        fuel: Some(10_000),
        memory: Some(1024 * 1024),
        table_elements: None,
        tables: None,
        memories: None,
    };
    let cases = [
        ("(module (func $start unreachable) (start $start))", false),
        (
            r#"(module
            (memory (export "memory") 1)
            (func (export "diyime_api_version") (result i32) i32.const 1)
            (func (export "diyime_alloc") (param i32) (result i32) i32.const 16)
            (func (export "diyime_call") (param i32 i32) (result i64) unreachable))"#,
            true,
        ),
    ];
    for (wat, traps_on_call) in cases {
        let mut binary = Bytes {
            size: 0,
            data: ptr::null_mut(),
        };
        // SAFETY: WAT lives through the conversion and Wasmtime owns the returned buffer.
        let wasm = unsafe {
            api.check(
                (api.wat)(wat.as_ptr(), wat.len(), &mut binary),
                ptr::null_mut(),
            )
            .unwrap();
            let wasm = std::slice::from_raw_parts(binary.data, binary.size).to_vec();
            (api.bytes_delete)(&mut binary);
            wasm
        };
        let path =
            std::env::temp_dir().join(format!("diyime-jit-trap-{}.trace", std::process::id()));
        let trace = Recorder::new(std::fs::File::create(&path).unwrap());
        let error = if traps_on_call {
            Runtime::new(&api, &wasm, &limits, Some(&trace))
                .expect("guest instantiates")
                .invoke(b"{}")
                .expect_err("guest call traps")
        } else {
            Runtime::new(&api, &wasm, &limits, Some(&trace))
                .err()
                .expect("guest start traps")
        };
        drop(trace);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(error.contains("unreachable"), "{error}");
        let intervals =
            super::worker_trace::decode(&bytes).expect("each phase occurs at most once");
        for phase in [WorkerTracePhase::Compile, WorkerTracePhase::Instantiate] {
            assert!(
                intervals
                    .iter()
                    .any(|interval| interval.phase == phase && interval.end.is_some())
            );
        }
        assert_eq!(
            intervals
                .iter()
                .any(|interval| interval.phase == WorkerTracePhase::GuestCall),
            traps_on_call
        );
    }
}
