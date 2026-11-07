//! Optional native import for the pinned Wasmtime 50.0.1 full C API.
use super::{Api, Extern, Handle, Limits, P, Recorder, Runtime, Val, compiled_module::symbol};
use crate::isolated_worker::resource_transport::{MAX_READ, ResourceReader};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};

type Callback = unsafe extern "C" fn(P, P, *const Val, usize, *mut Val, usize) -> P;
#[repr(C)]
#[derive(Default)]
struct TypeVector {
    size: usize,
    data: *mut P,
}

// Signatures/ownership: pinned crates/c-api/include/wasmtime/{linker,func,trap}.h
// and wasm.h. The optional exports are resolved only for this invocation mode.
#[derive(Clone, Copy)]
struct RangeApi {
    linker_new: unsafe extern "C" fn(P) -> P,
    linker_delete: unsafe extern "C" fn(P),
    define: unsafe extern "C" fn(
        P,
        *const u8,
        usize,
        *const u8,
        usize,
        P,
        Callback,
        P,
        Option<unsafe extern "C" fn(P)>,
    ) -> P,
    instantiate: unsafe extern "C" fn(P, P, P, *mut Handle, *mut P) -> P,
    valtype_new: unsafe extern "C" fn(u8) -> P,
    valtype_delete: unsafe extern "C" fn(P),
    vector_new: unsafe extern "C" fn(*mut TypeVector, usize, *const P),
    functype_new: unsafe extern "C" fn(*mut TypeVector, *mut TypeVector) -> P,
    functype_delete: unsafe extern "C" fn(P),
    caller_export: unsafe extern "C" fn(P, *const u8, usize, *mut Extern) -> bool,
    caller_context: unsafe extern "C" fn(P) -> P,
    trap: unsafe extern "C" fn(*const u8, usize) -> P,
}
impl RangeApi {
    fn load(api: &Api) -> Result<Self, String> {
        // SAFETY: exact signatures from the verified, pinned full x64 DLL.
        unsafe {
            Ok(Self {
                linker_new: symbol(api, c"wasmtime_linker_new")?,
                linker_delete: symbol(api, c"wasmtime_linker_delete")?,
                define: symbol(api, c"wasmtime_linker_define_func")?,
                instantiate: symbol(api, c"wasmtime_linker_instantiate")?,
                valtype_new: symbol(api, c"wasm_valtype_new")?,
                valtype_delete: symbol(api, c"wasm_valtype_delete")?,
                vector_new: symbol(api, c"wasm_valtype_vec_new")?,
                functype_new: symbol(api, c"wasm_functype_new")?,
                functype_delete: symbol(api, c"wasm_functype_delete")?,
                caller_export: symbol(api, c"wasmtime_caller_export_get")?,
                caller_context: symbol(api, c"wasmtime_caller_context")?,
                trap: symbol(api, c"wasmtime_trap_new")?,
            })
        }
    }
    unsafe fn function_type(self, emitting: bool) -> Result<P, String> {
        // i32 resource, i64 offset, i32 length, i32 destination -> i32 length.
        let kinds: &[u8] = if emitting {
            &[0, 0, 0]
        } else {
            &[0, 1, 0, 0, 0]
        };
        let types: Vec<P> = kinds
            .iter()
            .map(|kind| unsafe { (self.valtype_new)(*kind) })
            .collect();
        let nparams = types.len() - 1;
        if types.iter().any(|p| p.is_null()) {
            for p in types.into_iter().filter(|p| !p.is_null()) {
                unsafe { (self.valtype_delete)(p) };
            }
            return Err("resource function type unavailable".into());
        }
        let mut params = TypeVector::default();
        let mut results = TypeVector::default();
        // Vector construction takes ownership of the element handles;
        // functype_new consumes both vectors, including their contents.
        unsafe {
            (self.vector_new)(&mut params, nparams, types.as_ptr());
            (self.vector_new)(&mut results, 1, types.as_ptr().add(nparams));
            let result = (self.functype_new)(&mut params, &mut results);
            if result.is_null() {
                Err("resource function type unavailable".into())
            } else {
                Ok(result)
            }
        }
    }
}

pub(super) struct Bridge<'a> {
    api: &'a Api,
    range: RangeApi,
    reader: &'a mut dyn ResourceReader,
    failed: bool,
    emitting: bool,
}
impl Bridge<'_> {
    pub(super) fn failed(&self) -> bool {
        self.failed
    }
}

pub(super) fn instantiate<'a>(
    api: &'a Api,
    module: &[u8],
    limits: &Limits,
    diagnostics: Option<&'a Recorder>,
    reader: &'a mut dyn ResourceReader,
    emitting: bool,
) -> Result<Runtime<'a>, String> {
    let guest = Runtime::prepare_module(api, limits, diagnostics, |engine, result| {
        // SAFETY: same verified Engine, live module bytes and output pointer.
        unsafe {
            api.check(
                (api.module_new)(engine, module.as_ptr(), module.len(), result),
                ptr::null_mut(),
            )
        }
    })?;
    attach(guest, reader, emitting)
}

pub(super) fn attach<'a>(
    mut guest: Runtime<'a>,
    reader: &'a mut dyn ResourceReader,
    emitting: bool,
) -> Result<Runtime<'a>, String> {
    let api = guest.api;
    let range = RangeApi::load(api)?;
    guest.resource = Some(Box::new(Bridge {
        api,
        range,
        reader,
        failed: false,
        emitting,
    }));
    // The Box address stays stable until after Store deletion. The linker
    // clones type metadata; no C finalizer owns the Rust callback environment.
    let env = ptr::from_mut(guest.resource.as_deref_mut().unwrap()).cast();
    // SAFETY: all C handles belong to this Engine; cleanup covers every error.
    unsafe {
        let linker = (range.linker_new)(guest.engine);
        if linker.is_null() {
            return Err("resource linker unavailable".into());
        }
        let result = (|| {
            let ty = range.function_type(false)?;
            let error = (range.define)(
                linker,
                b"diyime_resource_v1".as_ptr(),
                18,
                b"read".as_ptr(),
                4,
                ty,
                read,
                env,
                None,
            );
            (range.functype_delete)(ty);
            api.check(error, ptr::null_mut())?;
            if emitting {
                let ty = range.function_type(true)?;
                let error = (range.define)(
                    linker,
                    b"diyime_prepared_v1".as_ptr(),
                    18,
                    b"emit".as_ptr(),
                    4,
                    ty,
                    emit,
                    env,
                    None,
                );
                (range.functype_delete)(ty);
                api.check(error, ptr::null_mut())?;
            }
            let mut trap = ptr::null_mut();
            let error = (range.instantiate)(
                linker,
                guest.context,
                guest.module,
                &mut guest.instance,
                &mut trap,
            );
            api.check(error, trap)
        })();
        (range.linker_delete)(linker);
        result?;
    }
    Ok(guest)
}

struct Export<'a> {
    api: &'a Api,
    value: Extern,
}

unsafe extern "C" fn emit(
    env: P,
    caller: P,
    args: *const Val,
    nargs: usize,
    results: *mut Val,
    nresults: usize,
) -> P {
    // SAFETY: this stable Box is installed only on construction-mode Stores.
    let bridge = unsafe { &mut *env.cast::<Bridge<'_>>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if bridge.failed
            || !bridge.emitting
            || nargs != 2
            || nresults != 1
            || args.is_null()
            || results.is_null()
        {
            return Err(());
        }
        let args = unsafe { std::slice::from_raw_parts(args, nargs) };
        if args.iter().any(|v| v.kind != 0) {
            return Err(());
        }
        let (source, length) = unsafe { (args[0].of.i32 as u32 as usize, args[1].of.i32) };
        if length <= 0 || length as usize > MAX_READ {
            return Err(());
        }
        let end = source.checked_add(length as usize).ok_or(())?;
        let mut export = Extern::default();
        if !unsafe { (bridge.range.caller_export)(caller, b"memory".as_ptr(), 6, &mut export) } {
            return Err(());
        }
        let export = Export {
            api: bridge.api,
            value: export,
        };
        if export.value.kind != 3 {
            return Err(());
        }
        let memory = unsafe { export.value.of.memory };
        let context = unsafe { (bridge.range.caller_context)(caller) };
        if context.is_null() || end > unsafe { (bridge.api.memory_size)(context, &memory) } {
            return Err(());
        }
        let data = unsafe { (bridge.api.memory_data)(context, &memory) };
        if data.is_null() {
            return Err(());
        }
        // Bound and copy before IO; a guest pointer never crosses the transport.
        let bytes =
            unsafe { std::slice::from_raw_parts(data.add(source), length as usize) }.to_vec();
        bridge.reader.emit_artifact(&bytes).map_err(|_| ())?;
        unsafe {
            results.write(Val::i32(length as u32));
        }
        Ok(())
    }));
    if matches!(result, Ok(Ok(()))) {
        ptr::null_mut()
    } else {
        bridge.failed = true;
        let message = b"artifact output failed";
        unsafe { (bridge.range.trap)(message.as_ptr(), message.len()) }
    }
}
impl Drop for Export<'_> {
    fn drop(&mut self) {
        // SAFETY: an owned caller_export_get result, released before caller ends.
        unsafe { (self.api.extern_delete)(&mut self.value) };
    }
}

unsafe extern "C" fn read(
    env: P,
    caller: P,
    args: *const Val,
    nargs: usize,
    results: *mut Val,
    nresults: usize,
) -> P {
    // SAFETY: Wasmtime passes exactly the stable Box environment installed above.
    let bridge = unsafe { &mut *env.cast::<Bridge<'_>>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if bridge.failed || nargs != 4 || nresults != 1 || args.is_null() || results.is_null() {
            return Err(());
        }
        // SAFETY: lengths checked; C API supplies initialized values matching the type.
        let args = unsafe { std::slice::from_raw_parts(args, nargs) };
        if args.iter().map(|v| v.kind).ne([0, 1, 0, 0]) {
            return Err(());
        }
        let (resource, offset, length, destination) = unsafe {
            (
                args[0].of.i32,
                args[1].of.i64,
                args[2].of.i32,
                args[3].of.i32 as u32 as usize,
            )
        };
        if resource <= 0 || offset < 0 || length <= 0 || length as usize > MAX_READ {
            return Err(());
        }
        let end = destination.checked_add(length as usize).ok_or(())?;
        let mut export = Extern::default();
        // SAFETY: the live caller owns this export; memory kind is checked before union access.
        if !unsafe { (bridge.range.caller_export)(caller, b"memory".as_ptr(), 6, &mut export) } {
            return Err(());
        }
        let export = Export {
            api: bridge.api,
            value: export,
        };
        if export.value.kind != 3 {
            return Err(());
        }
        let memory = unsafe { export.value.of.memory };
        let context = unsafe { (bridge.range.caller_context)(caller) };
        if context.is_null() || end > unsafe { (bridge.api.memory_size)(context, &memory) } {
            return Err(());
        }
        let bytes = bridge
            .reader
            .read_resource(resource as u32, offset as u64, length as u32)
            .map_err(|_| ())?;
        if bytes.len() != length as usize
            || end > unsafe { (bridge.api.memory_size)(context, &memory) }
        {
            return Err(());
        }
        // Reacquire the base after IO; no guest memory pointer survives a read.
        let data = unsafe { (bridge.api.memory_data)(context, &memory) };
        if data.is_null() {
            return Err(());
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), data.add(destination), bytes.len());
            results.write(Val::i32(length as u32));
        }
        Ok(())
    }));
    if matches!(result, Ok(Ok(()))) {
        ptr::null_mut()
    } else {
        bridge.failed = true;
        let message = b"resource read failed";
        // SAFETY: constant bounded message; trap ownership passes to Wasmtime.
        unsafe { (bridge.range.trap)(message.as_ptr(), message.len()) }
    }
}

#[test]
fn pinned_type_vector_layout() {
    assert_eq!(std::mem::size_of::<TypeVector>(), 16);
    assert_eq!(std::mem::align_of::<TypeVector>(), 8);
}
