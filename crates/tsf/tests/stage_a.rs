#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case)]

use std::{
    cell::Cell,
    ptr,
    rc::Rc,
    sync::{Mutex, MutexGuard},
};

use diy_ime2_tsf::{
    DllCanUnloadNow, DllGetClassObject, JAPANESE_LANGID, TEST_PROFILE_GUID, TEST_SERVICE_CLSID,
};
use windows::Win32::{
    Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_UNEXPECTED, HWND, S_FALSE, S_OK},
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, IClassFactory,
        REGCLS_MULTIPLEUSE,
    },
    UI::{
        Input::KeyboardAndMouse::{GetFocus, GetKeyboardLayout, HKL, SetFocus},
        TextServices::{
            CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITfDocumentMgr, ITfEditSession,
            ITfEditSession_Impl, ITfInputProcessorProfileMgr, ITfKeystrokeMgr,
            ITfTextInputProcessor, ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READWRITE,
            TF_ES_SYNC, TF_INPUTPROCESSORPROFILE, TF_IPPMF_FORPROCESS, TF_POPF_ALL,
            TF_PROFILETYPE_INPUTPROCESSOR, TF_RP_LOCALPROCESS, TF_URP_LOCALPROCESS,
        },
        WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP},
    },
};
use windows_core::{GUID, IUnknown, Interface, Result, implement};

static PROCESS_PROFILE_TEST: Mutex<()> = Mutex::new(());

struct Apartment {
    _gate: MutexGuard<'static, ()>,
}

impl Apartment {
    fn open() -> Result<Self> {
        let gate = PROCESS_PROFILE_TEST.lock().unwrap();
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self { _gate: gate })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[test]
fn class_factory_rejects_foreign_clsid_and_releases_failed_activation() -> Result<()> {
    let _apartment = Apartment::open()?;
    assert_eq!(DllCanUnloadNow(), S_OK);
    let mut raw = ptr::null_mut();
    let rejected = unsafe { DllGetClassObject(&GUID::zeroed(), &IClassFactory::IID, &mut raw) };
    assert_eq!(rejected, CLASS_E_CLASSNOTAVAILABLE);
    assert!(raw.is_null());
    unsafe { DllGetClassObject(&TEST_SERVICE_CLSID, &IClassFactory::IID, &mut raw).ok()? };
    let factory = unsafe { IClassFactory::from_raw(raw) };
    assert_eq!(DllCanUnloadNow(), S_FALSE);
    unsafe { factory.LockServer(true)? };
    let service: ITfTextInputProcessor = unsafe { factory.CreateInstance(None::<&IUnknown>)? };
    unsafe { factory.LockServer(false)? };
    drop(factory);
    assert_eq!(DllCanUnloadNow(), S_FALSE);

    let thread_manager: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)? };
    let error = unsafe { service.Activate(&thread_manager, 0) }.unwrap_err();
    assert_eq!(error.code(), E_UNEXPECTED);
    unsafe {
        service.Deactivate()?;
        service.Deactivate()?;
    }
    drop(service);
    assert_eq!(DllCanUnloadNow(), S_OK);
    Ok(())
}

#[implement(ITfEditSession)]
struct EditReceipt(Rc<Cell<Option<u32>>>);

impl ITfEditSession_Impl for EditReceipt_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        self.0.set(Some(cookie));
        Ok(())
    }
}

#[test]
fn real_tsf_document_grants_a_synchronous_edit_cookie() -> Result<()> {
    let _apartment = Apartment::open()?;
    let thread_manager: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)? };
    let client = unsafe { thread_manager.Activate()? };
    let document = unsafe { thread_manager.CreateDocumentMgr()? };
    let mut context = None;
    let mut owner_cookie = 0;
    unsafe {
        document.CreateContext(
            client,
            0,
            None::<&IUnknown>,
            &mut context,
            &mut owner_cookie,
        )?;
    }
    let context = context.expect("real TSF context");
    let received = Rc::new(Cell::new(None));
    let session: ITfEditSession = EditReceipt(received.clone()).into();
    unsafe {
        document.Push(&context)?;
        thread_manager.SetFocus(&document)?;
        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        context.RequestEditSession(client, &session, flags)?.ok()?;
        assert!(received.get().is_some());
        document.Pop(TF_POPF_ALL)?;
        thread_manager.Deactivate()?;
    }
    Ok(())
}

struct LocalProfile {
    manager: ITfInputProcessorProfileMgr,
    language: u16,
    live: bool,
    active: bool,
}

impl LocalProfile {
    fn register(
        manager: ITfInputProcessorProfileMgr,
        language: u16,
        enabled: bool,
    ) -> Result<Self> {
        let description: Vec<u16> = "DIY IME stage A process-local test"
            .encode_utf16()
            .collect();
        unsafe {
            manager.RegisterProfile(
                &TEST_SERVICE_CLSID,
                language,
                &TEST_PROFILE_GUID,
                &description,
                &[],
                0,
                HKL::default(),
                0,
                enabled,
                TF_RP_LOCALPROCESS,
            )?;
        }
        Ok(Self {
            manager,
            language,
            live: true,
            active: false,
        })
    }

    fn activate(&mut self) -> Result<()> {
        let result = unsafe {
            (self.manager.vtable().ActivateProfile)(
                self.manager.as_raw(),
                TF_PROFILETYPE_INPUTPROCESSOR,
                self.language,
                &TEST_SERVICE_CLSID,
                &TEST_PROFILE_GUID,
                HKL::default(),
                TF_IPPMF_FORPROCESS,
            )
        };
        if result == S_FALSE {
            return Err(E_FAIL.into());
        }
        result.ok()?;
        self.active = true;
        Ok(())
    }

    fn remove(&mut self) -> Result<()> {
        if self.active {
            unsafe {
                self.manager.DeactivateProfile(
                    TF_PROFILETYPE_INPUTPROCESSOR,
                    self.language,
                    &TEST_SERVICE_CLSID,
                    &TEST_PROFILE_GUID,
                    HKL::default(),
                    TF_IPPMF_FORPROCESS,
                )?;
            }
            self.active = false;
        }
        if self.live {
            unsafe {
                self.manager.UnregisterProfile(
                    &TEST_SERVICE_CLSID,
                    self.language,
                    &TEST_PROFILE_GUID,
                    TF_URP_LOCALPROCESS,
                )?;
            }
            self.live = false;
        }
        Ok(())
    }
}

impl Drop for LocalProfile {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

fn profile_exists(manager: &ITfInputProcessorProfileMgr) -> bool {
    let mut profile = TF_INPUTPROCESSORPROFILE::default();
    (unsafe {
        manager
            .GetProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                JAPANESE_LANGID,
                &TEST_SERVICE_CLSID,
                &TEST_PROFILE_GUID,
                HKL::default(),
                &mut profile,
            )
            .is_ok()
    }) && profile.clsid == TEST_SERVICE_CLSID
        && profile.guidProfile == TEST_PROFILE_GUID
}

#[test]
fn process_local_profile_registration_and_removal() -> Result<()> {
    let _apartment = Apartment::open()?;
    let manager: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)? };
    assert!(!profile_exists(&manager));
    let mut profile = LocalProfile::register(manager.clone(), JAPANESE_LANGID, false)?;
    assert!(profile_exists(&manager));
    profile.remove()?;
    assert!(!profile_exists(&manager));
    Ok(())
}

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

struct ClassRegistration(u32);

impl Drop for ClassRegistration {
    fn drop(&mut self) {
        let _ = unsafe { CoRevokeClassObject(self.0) };
    }
}

#[test]
fn private_window_selects_and_deactivates_the_test_service_through_tsf() -> Result<()> {
    let _apartment = Apartment::open()?;
    let window = Window(unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows_core::w!("STATIC"),
            windows_core::w!("DIY IME private test"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )?
    });
    let thread_manager: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)? };
    let client = unsafe { thread_manager.Activate()? };
    let document = unsafe { thread_manager.CreateDocumentMgr()? };
    let mut context = None;
    let mut cookie = 0;
    unsafe {
        document.CreateContext(client, 0, None::<&IUnknown>, &mut context, &mut cookie)?;
        document.Push(context.as_ref().expect("real TSF context"))?;
        let mut previous = ptr::null_mut();
        (thread_manager.vtable().AssociateFocus)(
            thread_manager.as_raw(),
            window.0,
            document.as_raw(),
            &mut previous,
        )
        .ok()?;
        if !previous.is_null() {
            drop(ITfDocumentMgr::from_raw(previous));
        }
        // A hidden test-owned HWND is enough for the local registration path.
        let _ = SetFocus(window.0);
        assert_eq!(GetFocus(), window.0);
        thread_manager.SetFocus(&document)?;
    }

    let mut factory_ptr = ptr::null_mut();
    unsafe { DllGetClassObject(&TEST_SERVICE_CLSID, &IClassFactory::IID, &mut factory_ptr).ok()? };
    let factory = unsafe { IClassFactory::from_raw(factory_ptr) };
    let registration = ClassRegistration(unsafe {
        CoRegisterClassObject(
            &TEST_SERVICE_CLSID,
            &factory,
            CLSCTX_INPROC_SERVER,
            REGCLS_MULTIPLEUSE,
        )?
    });
    let profiles: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)? };
    // Match only the test process's active layout. The separately tested Japanese
    // profile stays fixed, and this test never changes the desktop's input language.
    let language = (unsafe { GetKeyboardLayout(0) }.0 as usize & 0xffff) as u16;
    let mut profile = LocalProfile::register(profiles, language, true)?;
    profile.activate()?;
    let keys: ITfKeystrokeMgr = thread_manager.cast()?;
    assert_eq!(unsafe { keys.GetForeground()? }, TEST_SERVICE_CLSID);
    profile.remove()?;
    assert_ne!(unsafe { keys.GetForeground()? }, TEST_SERVICE_CLSID);
    unsafe {
        document.Pop(TF_POPF_ALL)?;
        thread_manager.Deactivate()?;
    }
    drop(registration);
    drop(factory);
    assert_eq!(DllCanUnloadNow(), S_OK);
    Ok(())
}
