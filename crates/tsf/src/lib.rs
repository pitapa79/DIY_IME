#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case)]

mod composition;
mod display;
mod host;
pub use host::set_test_host_configuration;
mod document;
pub mod keymap;
mod service;
mod windows_keys;

use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use windows::Win32::{
    Foundation::{
        BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, E_UNEXPECTED, S_FALSE,
        S_OK,
    },
    System::Com::{IClassFactory, IClassFactory_Impl},
    UI::TextServices::ITfTextInputProcessor,
};
use windows_core::{GUID, HRESULT, IUnknown, Interface, Result, implement};

/// Unique to the opt-in, x64 stage-A test service.
pub const TEST_SERVICE_CLSID: GUID = GUID::from_u128(0xeecf857d_82eb_4c57_9592_257f6ecf62f1);
pub const TEST_PROFILE_GUID: GUID = GUID::from_u128(0x8c73cb26_1ccf_45e1_a259_f29b674219e0);
pub const JAPANESE_LANGID: u16 = 0x0411;

static LIVE_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static SERVER_LOCKS: AtomicUsize = AtomicUsize::new(0);

struct ModuleRef;

impl ModuleRef {
    fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ModuleRef {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[implement(IClassFactory)]
struct Factory {
    _module: ModuleRef,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        output: *mut *mut c_void,
    ) -> Result<()> {
        if output.is_null() {
            return Err(E_POINTER.into());
        }
        // COM guarantees writable output storage when it is non-null.
        unsafe { output.write(std::ptr::null_mut()) };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if iid.is_null() {
            return Err(E_POINTER.into());
        }
        let service: ITfTextInputProcessor = service::TextService::new().into();
        unsafe { service.query(iid, output).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            SERVER_LOCKS
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_add(1)
                })
                .map_err(|_| E_UNEXPECTED)?;
        } else {
            SERVER_LOCKS
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_sub(1)
                })
                .map_err(|_| E_UNEXPECTED)?;
        }
        Ok(())
    }
}

/// # Safety
/// Called by COM with readable GUIDs and writable space for one interface pointer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if output.is_null() {
        return E_POINTER;
    }
    unsafe { output.write(std::ptr::null_mut()) };
    if clsid.is_null() || iid.is_null() {
        return E_POINTER;
    }
    if unsafe { *clsid } != TEST_SERVICE_CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory {
        _module: ModuleRef::new(),
    }
    .into();
    unsafe { factory.query(iid, output) }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE_OBJECTS.load(Ordering::SeqCst) == 0 && SERVER_LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}
