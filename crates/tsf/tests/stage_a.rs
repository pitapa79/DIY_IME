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
            TF_ES_SYNC, TF_INPUTPROCESSORPROFILE, TF_IPPMF_ENABLEPROFILE, TF_IPPMF_FORPROCESS,
            TF_POPF_ALL, TF_PROFILETYPE_INPUTPROCESSOR, TF_RP_LOCALPROCESS, TF_URP_LOCALPROCESS,
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
struct EditReceipt {
    context: windows::Win32::UI::TextServices::ITfContext,
    inserted: Rc<Cell<bool>>,
}

impl ITfEditSession_Impl for EditReceipt_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        use windows::Win32::UI::TextServices::{
            INSERT_TEXT_AT_SELECTION_FLAGS, ITfInsertAtSelection,
        };
        let writer: ITfInsertAtSelection = self.context.cast()?;
        let text = [u16::from(b'a')];
        let range = unsafe {
            writer.InsertTextAtSelection(cookie, INSERT_TEXT_AT_SELECTION_FLAGS(0), &text)?
        };
        let mut buffer = [0; 2];
        let mut read = 0;
        unsafe { range.GetText(cookie, 0, &mut buffer, &mut read)? };
        self.inserted.set(read == 1 && buffer[0] == text[0]);
        Ok(())
    }
}

#[test]
fn real_tsf_document_round_trips_text_inside_the_edit_session() -> Result<()> {
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
    let inserted = Rc::new(Cell::new(false));
    let session: ITfEditSession = EditReceipt {
        context: context.clone(),
        inserted: inserted.clone(),
    }
    .into();
    unsafe {
        document.Push(&context)?;
        thread_manager.SetFocus(&document)?;
        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        context.RequestEditSession(client, &session, flags)?.ok()?;
        assert!(inserted.get());
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

    fn activate(&mut self, enable_for_user: bool) -> Result<()> {
        let flags = TF_IPPMF_FORPROCESS
            | if enable_for_user {
                TF_IPPMF_ENABLEPROFILE
            } else {
                0
            };
        let result = unsafe {
            (self.manager.vtable().ActivateProfile)(
                self.manager.as_raw(),
                TF_PROFILETYPE_INPUTPROCESSOR,
                self.language,
                &TEST_SERVICE_CLSID,
                &TEST_PROFILE_GUID,
                HKL::default(),
                flags,
            )
        };
        if result == S_FALSE {
            return Err(windows_core::Error::new(
                E_FAIL,
                "ActivateProfile returned S_FALSE for the test profile",
            ));
        }
        result.ok().map_err(|error| {
            windows_core::Error::new(error.code(), "ActivateProfile failed for the test profile")
        })?;
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
    exercise_test_window(false, false)
}

#[test]
#[ignore = "requires a foreground window on an interactive x64 Windows desktop"]
fn interactive_app_routes_ordinary_and_protected_input() -> Result<()> {
    exercise_test_window(true, false)
}

#[test]
#[ignore = "requires the temporary machine profile enabled for the current user and desktop foreground"]
fn interactive_machine_registered_app_routes_input() -> Result<()> {
    exercise_test_window(true, true)
}

fn exercise_test_window(interactive: bool, machine_registered: bool) -> Result<()> {
    let _apartment = Apartment::open()?;
    let window = Window(unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows_core::w!("STATIC"),
            windows_core::w!("DIY IME TSF input verification"),
            if interactive {
                WS_OVERLAPPEDWINDOW
            } else {
                WS_POPUP
            },
            0,
            0,
            if interactive { 540 } else { 1 },
            if interactive { 180 } else { 1 },
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
    let scope = Rc::new(Cell::new(IS_DEFAULT));
    let owner: ITextStoreACP = text_store::Store::new(window.0, scope.clone()).into();
    let owner: IUnknown = owner.cast()?;
    unsafe {
        document.CreateContext(
            client,
            0,
            interactive.then_some(&owner),
            &mut context,
            &mut cookie,
        )?;
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
        if interactive {
            let _ = ShowWindow(window.0, SW_SHOW);
            let _ = SetForegroundWindow(window.0);
        }
        let _ = SetFocus(window.0);
        assert_eq!(GetFocus(), window.0);
        thread_manager.SetFocus(&document)?;
    }

    let profiles: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)? };
    let language = (unsafe { GetKeyboardLayout(0) }.0 as usize & 0xffff) as u16;
    let (mut profile, registration) = if machine_registered {
        assert_eq!(
            language, JAPANESE_LANGID,
            "select the Japanese keyboard language first"
        );
        assert!(
            profile_exists(&profiles),
            "the temporary machine registration is required"
        );
        (
            LocalProfile {
                manager: profiles,
                language,
                live: false,
                active: false,
            },
            None,
        )
    } else {
        // Discover the calling process's active layout without changing desktop input settings.
        let mut factory_ptr = ptr::null_mut();
        unsafe {
            DllGetClassObject(&TEST_SERVICE_CLSID, &IClassFactory::IID, &mut factory_ptr).ok()?
        };
        let factory = unsafe { IClassFactory::from_raw(factory_ptr) };
        let registration = ClassRegistration(unsafe {
            CoRegisterClassObject(
                &TEST_SERVICE_CLSID,
                &factory,
                CLSCTX_INPROC_SERVER,
                REGCLS_MULTIPLEUSE,
            )?
        });
        (
            LocalProfile::register(profiles, language, true)?,
            Some(registration),
        )
    };
    profile.activate(machine_registered)?;
    let keys: ITfKeystrokeMgr = thread_manager.cast()?;
    assert_eq!(
        unsafe { keys.GetForeground() }
            .expect("query TSF foreground after activating test profile"),
        TEST_SERVICE_CLSID
    );
    if interactive {
        let context = context.as_ref().expect("real TSF context");
        wait_for_app_focus(&thread_manager)?;
        verify_key_edit(context, &keys, client, &scope)?;
    }
    profile.remove()?;
    assert_ne!(
        unsafe { keys.GetForeground() }
            .expect("query TSF foreground after deactivating test profile"),
        TEST_SERVICE_CLSID
    );
    unsafe {
        document.Pop(TF_POPF_ALL)?;
        thread_manager.Deactivate()?;
    }
    drop(registration);
    assert_eq!(DllCanUnloadNow(), S_OK);
    Ok(())
}

use windows::Win32::{
    Foundation::{E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, LPARAM, WPARAM},
    System::Com::CoTaskMemAlloc,
    UI::{
        Input::KeyboardAndMouse::{VK_A, VK_TAB},
        TextServices::{
            GUID_COMPARTMENT_KEYBOARD_DISABLED, IS_DEFAULT, IS_PASSWORD, ITextStoreACP,
            ITfCompartmentMgr, ITfContext, ITfInputScope, ITfInputScope_Impl, InputScope,
            TF_ES_READ,
        },
        WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOW, SetForegroundWindow,
            ShowWindow, TranslateMessage, WS_OVERLAPPEDWINDOW,
        },
    },
};
use windows_core::{BSTR, VARIANT};

#[implement(ITfInputScope)]
struct AppScope(InputScope);

impl ITfInputScope_Impl for AppScope_Impl {
    fn GetInputScopes(&self, scopes: *mut *mut InputScope, count: *mut u32) -> Result<()> {
        if scopes.is_null() || count.is_null() {
            return Err(E_POINTER.into());
        }
        let value = unsafe { CoTaskMemAlloc(size_of::<InputScope>()) }.cast::<InputScope>();
        if value.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        unsafe {
            value.write(self.0);
            scopes.write(value);
            count.write(1);
        }
        Ok(())
    }
    fn GetPhrase(&self, _: *mut *mut BSTR, _: *mut u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetRegularExpression(&self) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetSRGS(&self) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetXML(&self) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
}

#[implement(ITfEditSession)]
struct ReadDocument {
    context: ITfContext,
    contents: Rc<Cell<(u32, [u16; 2])>>,
}

impl ITfEditSession_Impl for ReadDocument_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        let start = unsafe { self.context.GetStart(cookie)? };
        let mut shifted = 0;
        let mut read = 0;
        let mut text = [0; 2];
        unsafe {
            start.ShiftEnd(cookie, 2, &mut shifted, ptr::null())?;
            start.GetText(cookie, 0, &mut text, &mut read)?;
        }
        self.contents.set((read, text));
        Ok(())
    }
}

fn verify_key_edit(
    context: &ITfContext,
    keys: &ITfKeystrokeMgr,
    client: u32,
    scope: &Cell<InputScope>,
) -> Result<()> {
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
    let contents = Rc::new(Cell::new((0, [0; 2])));
    let reader: ITfEditSession = ReadDocument {
        context: context.clone(),
        contents: contents.clone(),
    }
    .into();
    let compartments: ITfCompartmentMgr = context.cast()?;
    unsafe {
        let disabled = compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?;
        let key = WPARAM(usize::from(VK_A.0));
        scope.set(IS_DEFAULT);
        disabled.SetValue(client, &VARIANT::from(1_i32))?;
        assert!(!keys.TestKeyDown(key, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(key, LPARAM(0))?.as_bool());
        disabled.SetValue(client, &VARIANT::from(0_i32))?;
        let tab = WPARAM(usize::from(VK_TAB.0));
        assert!(!keys.TestKeyDown(tab, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(tab, LPARAM(0))?.as_bool());
        scope.set(IS_PASSWORD);
        assert!(!keys.TestKeyDown(key, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(key, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(
            contents.get(),
            (0, [0; 2]),
            "protected input must not edit the TSF document"
        );
        scope.set(IS_DEFAULT);
        assert!(
            keys.TestKeyDown(key, LPARAM(0))?.as_bool(),
            "ordinary input recognition"
        );
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (0, [0; 2]), "key inspection must not edit");
        assert!(
            keys.KeyDown(key, LPARAM(0))?.as_bool(),
            "key must be consumed by the TIP"
        );
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'a'), 0]));
    }
    Ok(())
}

fn wait_for_app_focus(manager: &ITfThreadMgr) -> Result<()> {
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if manager.IsThreadFocus()?.as_bool() {
                return Ok(());
            }
        }
        if std::time::Instant::now() >= limit {
            eprintln!("the test window did not obtain desktop foreground");
            return Err(E_FAIL.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[path = "support/text_store.rs"]
mod text_store;
