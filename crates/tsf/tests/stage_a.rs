#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case)]

use std::{
    cell::Cell,
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::Command,
    ptr,
    rc::Rc,
    sync::{Mutex, MutexGuard},
};

use diy_ime2_ime_host::external::Observation;
use diy_ime2_tsf::{
    DllCanUnloadNow, DllGetClassObject, JAPANESE_LANGID, TEST_PROFILE_GUID, TEST_SERVICE_CLSID,
};
use windows::Win32::{
    Foundation::{
        CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_UNEXPECTED, FreeLibrary, HMODULE, HWND, POINT,
        S_FALSE, S_OK,
    },
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, IClassFactory,
        REGCLS_MULTIPLEUSE,
    },
    System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LoadLibraryExW,
    },
    UI::{
        Input::KeyboardAndMouse::{GetFocus, GetKeyboardLayout, HKL, SetFocus},
        TextServices::{
            CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITfDisplayAttributeProvider,
            ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInputProcessorProfileMgr,
            ITfKeystrokeMgr, ITfLangBarItemButton, ITfLangBarItemMgr, ITfTextInputProcessor,
            ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READWRITE, TF_ES_SYNC,
            TF_INPUTPROCESSORPROFILE, TF_IPPMF_ENABLEPROFILE, TF_IPPMF_FORPROCESS, TF_LBI_CLK_LEFT,
            TF_POPF_ALL, TF_PROFILETYPE_INPUTPROCESSOR, TF_RP_LOCALPROCESS, TF_URP_LOCALPROCESS,
        },
        WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP},
    },
};
use windows_core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, Result, implement, s};

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
    let display: ITfDisplayAttributeProvider = service.cast()?;
    let enum_attributes = unsafe { display.EnumDisplayAttributeInfo()? };
    let mut attributes = [None];
    let mut fetched = 0;
    unsafe { enum_attributes.Next(&mut attributes, &mut fetched)? };
    assert_eq!(fetched, 1);
    let [first] = attributes;
    let info = first.ok_or(E_FAIL)?;
    let mut style = windows::Win32::UI::TextServices::TF_DISPLAYATTRIBUTE::default();
    unsafe { info.GetAttributeInfo(&mut style)? };
    assert_eq!(style.lsStyle, windows::Win32::UI::TextServices::TF_LS_SOLID);
    drop((display, enum_attributes, info));

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

struct TestLibrary(HMODULE);

impl TestLibrary {
    fn open(path: &Path) -> Result<Self> {
        assert!(path.is_absolute(), "use an absolute test DLL path");
        let wide: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        let library = unsafe {
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )?
        };
        Ok(Self(library))
    }

    fn factory(&self) -> Result<IClassFactory> {
        type GetClass =
            unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
        let symbol = unsafe { GetProcAddress(self.0, s!("DllGetClassObject")) }.ok_or(E_FAIL)?;
        let get_class: GetClass = unsafe { std::mem::transmute(symbol) };
        let mut factory = ptr::null_mut();
        unsafe {
            get_class(&TEST_SERVICE_CLSID, &IClassFactory::IID, &mut factory).ok()?;
            Ok(IClassFactory::from_raw(factory))
        }
    }
}

impl Drop for TestLibrary {
    fn drop(&mut self) {
        // The DLL's COM objects may still exist when a test unwinds. Let process
        // exit unload it in that case, and never unload code that TSF holds.
        if library_idle(self.0) == Some(true) {
            let _ = unsafe { FreeLibrary(self.0) };
        }
    }
}

enum ProfileSource<'a> {
    ProcessLocal,
    ProcessLocalDll(&'a Path),
    Machine,
}

enum WindowChecks {
    Activation,
    Keys,
    Conversion,
    Protection(Option<Observation>),
    FailedHost(Observation),
}

#[test]
fn private_window_selects_and_deactivates_the_test_service_through_tsf() -> Result<()> {
    exercise_test_window(ProfileSource::ProcessLocal, WindowChecks::Activation)
}

#[test]
#[ignore = "requires a foreground window on an interactive x64 Windows desktop"]
fn interactive_app_routes_ordinary_and_protected_input() -> Result<()> {
    exercise_test_window(ProfileSource::ProcessLocal, WindowChecks::Keys)
}

#[test]
#[ignore = "requires the temporary machine profile enabled for the current user and desktop foreground"]
fn interactive_machine_registered_app_routes_input() -> Result<()> {
    exercise_test_window(ProfileSource::Machine, WindowChecks::Keys)
}

#[test]
#[ignore = "requires the built TSF DLL with an adjacent Stage B Host and an interactive Windows desktop"]
fn interactive_process_local_dll_converts() -> Result<()> {
    let dll = std::env::var_os("DIY_IME_TEST_DLL").expect("absolute path of staged test DLL");
    let dll = Path::new(&dll);
    eprintln!("process-local DLL: ordinary keys and document switch");
    exercise_test_window(ProfileSource::ProcessLocalDll(dll), WindowChecks::Keys)?;
    eprintln!("process-local DLL: conversion, candidates and finalization");
    exercise_test_window(
        ProfileSource::ProcessLocalDll(dll),
        WindowChecks::Conversion,
    )?;
    eprintln!("process-local DLL: protected input");
    exercise_test_window(
        ProfileSource::ProcessLocalDll(dll),
        WindowChecks::Protection(None),
    )
}

#[test]
#[ignore = "requires the approved isolated Host fixture and an interactive Windows desktop"]
fn interactive_app_converts_through_official_host() -> Result<()> {
    let exe = std::env::var_os("DIY_IME_TEST_HOST").expect("test Host executable");
    let config = std::env::var_os("DIY_IME_TEST_HOST_CONFIG").expect("test Host configuration");
    diy_ime2_tsf::set_test_host_configuration(exe.into(), config.into());
    exercise_test_window(ProfileSource::ProcessLocal, WindowChecks::Conversion)
}

#[test]
#[ignore = "requires the approved isolated Host fixture and an interactive Windows desktop"]
fn interactive_app_enforces_protection() -> Result<()> {
    let exe = std::env::var_os("DIY_IME_TEST_HOST").expect("test Host executable");
    let config = std::env::var_os("DIY_IME_TEST_HOST_CONFIG").expect("test Host configuration");
    let observation = diy_ime2_tsf::set_test_host_configuration(exe.into(), config.into());
    exercise_test_window(
        ProfileSource::ProcessLocal,
        WindowChecks::Protection(Some(observation)),
    )
}

#[test]
#[ignore = "requires the approved isolated Host fixture and an interactive Windows desktop"]
fn interactive_app_recovers_after_host_startup_failure() -> Result<()> {
    let exe = PathBuf::from(std::env::var_os("DIY_IME_TEST_HOST").expect("test Host executable"));
    let config = PathBuf::from(
        std::env::var_os("DIY_IME_TEST_HOST_CONFIG").expect("test Host configuration"),
    );
    assert!(config.is_file(), "the recovery configuration must exist");
    let unavailable = config.join("nonexistent");
    let failed_start = Command::new(&exe)
        .arg(&unavailable)
        .output()
        .expect("start the real Host with the invalid configuration");
    assert!(
        !failed_start.status.success(),
        "the Host must reject that path"
    );
    let failed = diy_ime2_tsf::set_test_host_configuration(exe.clone(), unavailable);
    exercise_test_window(
        ProfileSource::ProcessLocal,
        WindowChecks::FailedHost(failed),
    )?;
    diy_ime2_tsf::set_test_host_configuration(exe, config);
    exercise_test_window(ProfileSource::ProcessLocal, WindowChecks::Conversion)
}

#[test]
#[ignore = "requires the temporarily installed Stage B profile and approved adjacent Host configuration"]
fn interactive_machine_registered_stage_b() -> Result<()> {
    exercise_test_window(ProfileSource::Machine, WindowChecks::Conversion)
}

#[test]
#[ignore = "requires the temporarily installed Stage B profile and approved adjacent Host configuration"]
fn interactive_machine_registered_stage_b_protection() -> Result<()> {
    exercise_test_window(ProfileSource::Machine, WindowChecks::Protection(None))
}

fn exercise_test_window(source: ProfileSource<'_>, checks: WindowChecks) -> Result<()> {
    let interactive = !matches!(checks, WindowChecks::Activation);
    let library = match source {
        ProfileSource::ProcessLocalDll(path) => Some(TestLibrary::open(path)?),
        ProfileSource::ProcessLocal | ProfileSource::Machine => None,
    };
    let machine_registered = matches!(source, ProfileSource::Machine);
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
    let deny_writes = Rc::new(Cell::new(false));
    let owner: ITextStoreACP =
        text_store::Store::new(window.0, scope.clone(), deny_writes.clone()).into();
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
        let factory = if let Some(library) = library.as_ref() {
            library.factory()?
        } else {
            let mut factory_ptr = ptr::null_mut();
            unsafe {
                DllGetClassObject(&TEST_SERVICE_CLSID, &IClassFactory::IID, &mut factory_ptr)
                    .ok()?;
                IClassFactory::from_raw(factory_ptr)
            }
        };
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
    if interactive {
        wait_for_app_focus(&thread_manager)?;
        pump_until(|| {
            Ok(unsafe { keys.GetForeground() }.is_ok_and(|service| service == TEST_SERVICE_CLSID))
        })
        .inspect_err(|_| {
            eprintln!("waiting for the test profile to become the TSF foreground service")
        })?;
    }
    assert_eq!(
        unsafe { keys.GetForeground() }
            .expect("query TSF foreground after activating test profile"),
        TEST_SERVICE_CLSID
    );
    if interactive {
        let context = context.as_ref().expect("real TSF context");
        match &checks {
            WindowChecks::Protection(observation) => verify_protection(
                context,
                &thread_manager,
                &keys,
                client,
                &scope,
                observation.as_ref(),
            )?,
            WindowChecks::Conversion => verify_conversion(
                context,
                &document,
                &thread_manager,
                &keys,
                client,
                &deny_writes,
            )?,
            WindowChecks::Keys | WindowChecks::FailedHost(_) => {
                verify_key_edit(context, &keys, client, &scope, &deny_writes)?;
                verify_document_switch(context, &document, &thread_manager, &keys, client)?;
                if let WindowChecks::FailedHost(observation) = &checks {
                    assert_eq!(observation.dispatched(), 0);
                }
            }
            WindowChecks::Activation => unreachable!("headless activation has no input checks"),
        }
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
    if library.is_some() || !matches!(checks, WindowChecks::Activation | WindowChecks::Keys) {
        if machine_registered || library.is_some() {
            pump_until(installed_service_idle)?;
        } else {
            pump_until(|| Ok(DllCanUnloadNow() == S_OK))?;
        }
    }
    assert_eq!(DllCanUnloadNow(), S_OK);
    Ok(())
}

use windows::Win32::{
    Foundation::{E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, LPARAM, WPARAM},
    System::Com::CoTaskMemAlloc,
    UI::{
        Input::KeyboardAndMouse::{
            VK_A, VK_B, VK_BACK, VK_DOWN, VK_ESCAPE, VK_N, VK_RETURN, VK_SPACE, VK_TAB, VK_UP,
        },
        TextServices::{
            GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_ATTRIBUTE, IS_DEFAULT, IS_PASSWORD,
            ITextStoreACP, ITfCandidateListUIElement, ITfCompartmentMgr, ITfContext,
            ITfContextComposition, ITfContextOwnerCompositionServices, ITfInputScope,
            ITfInputScope_Impl, ITfRangeACP, InputScope, TF_ES_READ,
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
        if self.0 == InputScope(-1) {
            return Err(E_FAIL.into());
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
    caret: Rc<Cell<(i32, i32)>>,
    decorated: Rc<Cell<bool>>,
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
        let mut selections = [windows::Win32::UI::TextServices::TF_SELECTION::default()];
        let mut count = 0;
        let result = unsafe {
            self.context.GetSelection(
                cookie,
                windows::Win32::UI::TextServices::TF_DEFAULT_SELECTION,
                &mut selections,
                &mut count,
            )
        };
        let [selection] = selections;
        let range = std::mem::ManuallyDrop::into_inner(selection.range).ok_or(E_FAIL)?;
        result?;
        assert_eq!(count, 1);
        let acp: ITfRangeACP = range.cast()?;
        let mut position = 0;
        let mut length = 0;
        unsafe { acp.GetExtent(&mut position, &mut length)? };
        self.caret.set((position, length));
        let at_start = unsafe { self.context.GetStart(cookie)? };
        unsafe { at_start.ShiftEnd(cookie, 1, &mut shifted, ptr::null())? };
        let value = unsafe {
            self.context
                .GetProperty(&GUID_PROP_ATTRIBUTE)?
                .GetValue(cookie, &at_start)?
        };
        self.decorated.set(
            (unsafe { value.as_raw().Anonymous.Anonymous.vt })
                == windows::Win32::System::Variant::VT_I4.0,
        );
        Ok(())
    }
}

fn verify_key_edit(
    context: &ITfContext,
    keys: &ITfKeystrokeMgr,
    client: u32,
    scope: &Cell<InputScope>,
    deny_writes: &Cell<bool>,
) -> Result<()> {
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
    let contents = Rc::new(Cell::new((0, [0; 2])));
    let caret = Rc::new(Cell::new((0, 0)));
    let decorated = Rc::new(Cell::new(false));
    let reader: ITfEditSession = ReadDocument {
        context: context.clone(),
        contents: contents.clone(),
        caret: caret.clone(),
        decorated: decorated.clone(),
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
        assert_eq!(caret.get(), (1, 0));
        assert!(decorated.get());

        let second = WPARAM(usize::from(VK_B.0));
        deny_writes.set(true);
        assert!(keys.TestKeyDown(second, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(second, LPARAM(0))?.as_bool());
        deny_writes.set(false);
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'a'), 0]));
        assert!(keys.KeyDown(second, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (2, [u16::from(b'a'), u16::from(b'b')]));
        assert_eq!(caret.get(), (2, 0));

        let back = WPARAM(usize::from(VK_BACK.0));
        assert!(keys.KeyDown(back, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'a'), 0]));

        let compositions: ITfContextComposition = context.cast()?;
        let owner: ITfContextOwnerCompositionServices = context.cast()?;
        let mut views = [None];
        let mut fetched = 0;
        compositions
            .EnumCompositions()?
            .Next(&mut views, &mut fetched)?;
        assert_eq!(fetched, 1);
        owner.TerminateComposition(views[0].as_ref().expect("preedit composition"))?;
        assert!(!keys.KeyDown(back, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'a'), 0]));
        assert!(keys.KeyDown(second, LPARAM(0))?.as_bool());
        let escape = WPARAM(usize::from(VK_ESCAPE.0));
        assert!(keys.KeyDown(escape, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'a'), 0]));
        assert!(!keys.KeyDown(escape, LPARAM(0))?.as_bool());

        let write = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        change_selection(context, client, write, Some("𠮷"), 1, 1)?;
        assert!(
            !keys.TestKeyDown(key, LPARAM(0))?.as_bool(),
            "split surrogate"
        );
        assert!(!keys.KeyDown(key, LPARAM(0))?.as_bool());
        let original: Vec<_> = "𠮷".encode_utf16().collect();
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get().1.as_slice(), original);

        change_selection(context, client, write, None, 0, 2)?;
        assert!(keys.KeyDown(key, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'a'), 0]));
        assert!(keys.KeyDown(back, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get().1.as_slice(), original);
        assert_eq!(
            caret.get(),
            (0, 2),
            "cancel restores the original selection"
        );

        change_selection(context, client, write, Some(""), 0, 0)?;
        assert!(keys.KeyDown(key, LPARAM(0))?.as_bool());
        scope.set(IS_PASSWORD);
        assert!(!keys.TestKeyDown(back, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(back, LPARAM(0))?.as_bool());
        scope.set(IS_DEFAULT);
        assert!(
            !keys.KeyDown(back, LPARAM(0))?.as_bool(),
            "old preedit must stay invalid"
        );
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(
            contents.get(),
            (1, [u16::from(b'a'), 0]),
            "protected text must be unchanged"
        );

        change_selection(context, client, write, Some(""), 0, 0)?;
    }
    Ok(())
}

#[implement(ITfEditSession)]
struct ChangeSelection {
    context: ITfContext,
    replacement: Option<Vec<u16>>,
    start: i32,
    end: i32,
}

impl ITfEditSession_Impl for ChangeSelection_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        use windows::Win32::UI::TextServices::{
            TF_AE_END, TF_ANCHOR_START, TF_SELECTION, TF_SELECTIONSTYLE,
        };
        unsafe {
            if let Some(replacement) = self.replacement.as_ref() {
                let all = self.context.GetStart(cookie)?;
                let end = self.context.GetEnd(cookie)?;
                all.ShiftEndToRange(cookie, &end, TF_ANCHOR_START)?;
                all.SetText(cookie, 0, replacement)?;
            }
            let range = self.context.GetStart(cookie)?;
            let acp: ITfRangeACP = range.cast()?;
            acp.SetExtent(self.start, self.end - self.start)?;
            let selection = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(range)),
                style: TF_SELECTIONSTYLE {
                    ase: TF_AE_END,
                    fInterimChar: false.into(),
                },
            };
            let outcome = self
                .context
                .SetSelection(cookie, std::slice::from_ref(&selection));
            drop(std::mem::ManuallyDrop::into_inner(selection.range));
            outcome
        }
    }
}

fn change_selection(
    context: &ITfContext,
    client: u32,
    flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    replacement: Option<&str>,
    start: i32,
    end: i32,
) -> Result<()> {
    let session: ITfEditSession = ChangeSelection {
        context: context.clone(),
        replacement: replacement.map(|text| text.encode_utf16().collect()),
        start,
        end,
    }
    .into();
    unsafe { context.RequestEditSession(client, &session, flags)?.ok() }
}

fn second_document(
    thread: &ITfThreadMgr,
    original: &ITfContext,
    client: u32,
) -> Result<ITfDocumentMgr> {
    let window = unsafe { original.GetActiveView()?.GetWnd()? };
    let store: ITextStoreACP =
        text_store::Store::new(window, Rc::new(Cell::new(IS_DEFAULT)), Rc::default()).into();
    let owner: IUnknown = store.cast()?;
    let other = unsafe { thread.CreateDocumentMgr()? };
    let mut context = None;
    let mut cookie = 0;
    unsafe {
        other.CreateContext(client, 0, &owner, &mut context, &mut cookie)?;
        other.Push(&context.ok_or(E_FAIL)?)?;
    }
    Ok(other)
}

fn focus_document(thread: &ITfThreadMgr, document: &ITfDocumentMgr) -> Result<()> {
    unsafe { thread.SetFocus(document)? };
    pump_messages();
    assert_eq!(&unsafe { thread.GetFocus()? }, document);
    Ok(())
}

fn verify_document_switch(
    context: &ITfContext,
    document: &ITfDocumentMgr,
    thread: &ITfThreadMgr,
    keys: &ITfKeystrokeMgr,
    client: u32,
) -> Result<()> {
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
    let contents = Rc::new(Cell::new((0, [0; 2])));
    let reader: ITfEditSession = ReadDocument {
        context: context.clone(),
        contents: contents.clone(),
        caret: Rc::default(),
        decorated: Rc::default(),
    }
    .into();
    let key = WPARAM(usize::from(VK_A.0));
    unsafe {
        assert!(keys.KeyDown(key, LPARAM(0))?.as_bool());
        let other = second_document(thread, context, client)?;
        focus_document(thread, &other)?;
        focus_document(thread, document)?;
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(
            contents.get(),
            (0, [0; 2]),
            "focus loss must cancel old preedit"
        );
        other.Pop(TF_POPF_ALL)?;
        assert!(
            !keys
                .KeyDown(WPARAM(usize::from(VK_BACK.0)), LPARAM(0))?
                .as_bool()
        );
    }
    Ok(())
}

fn verify_conversion(
    context: &ITfContext,
    document: &ITfDocumentMgr,
    thread: &ITfThreadMgr,
    keys: &ITfKeystrokeMgr,
    client: u32,
    deny_writes: &Cell<bool>,
) -> Result<()> {
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
    let contents = Rc::new(Cell::new((0, [0; 2])));
    let reader: ITfEditSession = ReadDocument {
        context: context.clone(),
        contents: contents.clone(),
        caret: Rc::default(),
        decorated: Rc::default(),
    }
    .into();
    let compartment_manager: ITfCompartmentMgr = context.cast()?;
    let space = WPARAM(usize::from(VK_SPACE.0));
    unsafe {
        compartment_manager
            .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?
            .SetValue(client, &VARIANT::from(0_i32))?;
        for key in b"KOUSUI" {
            assert!(
                keys.KeyDown(WPARAM(usize::from(*key)), LPARAM(0))?
                    .as_bool()
            );
        }
        let waiting_for_host = std::time::Instant::now();
        pump_until(|| Ok(keys.TestKeyDown(space, LPARAM(0))?.as_bool()))
            .inspect_err(|_| eprintln!("waiting for the Stage B Host to accept conversion"))?;
        let host_ready_ms = waiting_for_host.elapsed().as_millis();
        let conversion_started = std::time::Instant::now();
        assert!(keys.KeyDown(space, LPARAM(0))?.as_bool());
        let conversion_callback_us = conversion_started.elapsed().as_micros();
        pump_until(|| Ok(candidate_list(thread, document)?.is_some()))
            .inspect_err(|_| eprintln!("starting candidate list"))?;
        eprintln!(
            "stage_b_host_ready_ms={host_ready_ms} space_callback_us={conversion_callback_us} space_to_candidates_ms={}",
            conversion_started.elapsed().as_millis()
        );
        let initial = candidate_list(thread, document)?.ok_or(E_FAIL)?;
        assert!(initial.IsShown()?.as_bool());
        assert!(initial.GetCount()? >= 3);
        let start = initial.GetSelection()?;
        assert_eq!(initial.GetString(start)?.to_string(), "降水");
        for (key, forward) in [(VK_DOWN, true), (VK_SPACE, true), (VK_UP, false)] {
            let previous = initial.GetSelection()?;
            let count = initial.GetCount()?;
            let expected = if forward {
                (previous + 1) % count
            } else {
                (previous + count - 1) % count
            };
            assert!(
                keys.TestKeyDown(WPARAM(usize::from(key.0)), LPARAM(0))?
                    .as_bool()
            );
            assert_eq!(
                initial.GetSelection()?,
                previous,
                "inspection does not move selection"
            );
            assert!(
                keys.KeyDown(WPARAM(usize::from(key.0)), LPARAM(0))?
                    .as_bool()
            );
            assert_eq!(initial.GetSelection()?, expected);
            let text = initial.GetString(expected)?.to_string();
            let rendered: Vec<_> = text.encode_utf16().collect();
            context.RequestEditSession(client, &reader, flags)?.ok()?;
            assert_eq!(contents.get().1.as_slice(), rendered);
        }

        let escape = WPARAM(usize::from(VK_ESCAPE.0));
        assert!(keys.KeyDown(escape, LPARAM(0))?.as_bool());
        assert!(candidate_list(thread, document)?.is_none());
        assert_eq!(initial.GetCount()?, 0);
        assert!(initial.GetSelection().is_err());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (2, [u16::from(b'k'), u16::from(b'o')]));

        assert!(keys.KeyDown(space, LPARAM(0))?.as_bool());
        pump_until(|| Ok(candidate_list(thread, document)?.is_some()))
            .inspect_err(|_| eprintln!("candidate list after returning to raw input"))?;
        let enter = WPARAM(usize::from(VK_RETURN.0));
        let list = candidate_list(thread, document)?.ok_or(E_FAIL)?;
        assert!(
            keys.KeyDown(WPARAM(usize::from(VK_DOWN.0)), LPARAM(0))?
                .as_bool()
        );
        let expected: Vec<_> = list
            .GetString(list.GetSelection()?)?
            .to_string()
            .encode_utf16()
            .collect();

        deny_writes.set(true);
        assert!(keys.TestKeyDown(enter, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(enter, LPARAM(0))?.as_bool());
        deny_writes.set(false);
        assert!(candidate_list(thread, document)?.is_some());
        assert!(keys.KeyDown(enter, LPARAM(0))?.as_bool());
        assert_eq!(list.GetCount()?, 0);
        assert!(!keys.KeyDown(enter, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(escape, LPARAM(0))?.as_bool());
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get().1.as_slice(), expected);

        let write = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        change_selection(context, client, write, Some(""), 0, 0)?;
        pump_messages();
        let n = WPARAM(usize::from(VK_N.0));
        assert!(keys.KeyDown(n, LPARAM(0))?.as_bool());
        let finalize_started = std::time::Instant::now();
        assert!(keys.KeyDown(enter, LPARAM(0))?.as_bool());
        let finalize_callback_us = finalize_started.elapsed().as_micros();
        assert!(keys.KeyDown(enter, LPARAM(0))?.as_bool());
        let nasal: Vec<_> = "ん".encode_utf16().collect();
        pump_until(|| {
            context.RequestEditSession(client, &reader, flags)?.ok()?;
            Ok(contents.get().0 == 1 && contents.get().1[..1] == nasal)
        })
        .inspect_err(|_| eprintln!("finalize result did not reach the active test document"))?;
        eprintln!(
            "stage_b_enter_callback_us={finalize_callback_us} enter_to_commit_ms={}",
            finalize_started.elapsed().as_millis()
        );
        assert!(!keys.KeyDown(enter, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(escape, LPARAM(0))?.as_bool());
        assert!(candidate_list(thread, document)?.is_none());

        change_selection(context, client, write, Some(""), 0, 0)?;
        pump_messages();
        for key in b"KOUSUI" {
            assert!(
                keys.KeyDown(WPARAM(usize::from(*key)), LPARAM(0))?
                    .as_bool()
            );
        }
        assert!(keys.KeyDown(space, LPARAM(0))?.as_bool());
        pump_until(|| Ok(candidate_list(thread, document)?.is_some()))?;
        let old = candidate_list(thread, document)?.ok_or(E_FAIL)?;
        let other = second_document(thread, context, client)?;
        focus_document(thread, &other)?;
        focus_document(thread, document)?;
        pump_messages();
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (0, [0; 2]));
        assert!(candidate_list(thread, document)?.is_none());
        assert_eq!(old.GetCount()?, 0);
        assert!(old.GetString(0).is_err());
        assert!(!keys.TestKeyDown(space, LPARAM(0))?.as_bool());

        change_selection(context, client, write, Some(""), 0, 0)?;
        pump_messages();
        assert!(keys.KeyDown(n, LPARAM(0))?.as_bool());
        assert!(keys.KeyDown(enter, LPARAM(0))?.as_bool());
        focus_document(thread, &other)?;
        focus_document(thread, document)?;
        pump_messages();
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(
            contents.get(),
            (0, [0; 2]),
            "pending finalize must not survive focus change"
        );
        other.Pop(TF_POPF_ALL)?;
        assert!(!keys.TestKeyDown(enter, LPARAM(0))?.as_bool());
    }
    Ok(())
}

fn installed_service_idle() -> Result<bool> {
    use windows::Win32::{
        Foundation::ERROR_MOD_NOT_FOUND, System::LibraryLoader::GetModuleHandleExW,
    };
    use windows_core::w;
    let mut module = HMODULE::default();
    if let Err(error) = unsafe { GetModuleHandleExW(0, w!("diy_ime2_tsf.dll"), &mut module) } {
        return if error.code() == ERROR_MOD_NOT_FOUND.to_hresult() {
            Ok(true)
        } else {
            Err(error)
        };
    }
    let idle = library_idle(module);
    unsafe { FreeLibrary(module)? };
    idle.ok_or(E_FAIL.into())
}

fn library_idle(module: HMODULE) -> Option<bool> {
    unsafe { GetProcAddress(module, s!("DllCanUnloadNow")) }.map(|symbol| {
        let can_unload: unsafe extern "system" fn() -> HRESULT =
            unsafe { std::mem::transmute(symbol) };
        unsafe { can_unload() == S_OK }
    })
}

fn candidate_list(
    thread: &ITfThreadMgr,
    target: &ITfDocumentMgr,
) -> Result<Option<ITfCandidateListUIElement>> {
    use windows::Win32::UI::TextServices::ITfUIElementMgr;
    let manager: ITfUIElementMgr = thread.cast()?;
    unsafe {
        let elements = manager.EnumUIElements()?;
        loop {
            let mut item = [None];
            let mut fetched = 0;
            elements.Next(&mut item, &mut fetched)?;
            if fetched == 0 {
                return Ok(None);
            }
            let [element] = item;
            if let Some(element) = element
                && let Ok(list) = element.cast::<ITfCandidateListUIElement>()
                && list
                    .GetDocumentMgr()
                    .is_ok_and(|current| &current == target)
            {
                return Ok(Some(list));
            }
        }
    }
}

fn wait_for_app_focus(manager: &ITfThreadMgr) -> Result<()> {
    pump_until(|| Ok(unsafe { manager.IsThreadFocus()? }.as_bool()))
        .inspect_err(|_| eprintln!("waiting for the TSF test window to gain foreground focus"))
}

fn pump_messages() {
    let mut message = MSG::default();
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn pump_until(mut ready: impl FnMut() -> Result<bool>) -> Result<()> {
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        pump_messages();
        if ready()? {
            return Ok(());
        }
        if std::time::Instant::now() >= limit {
            eprintln!("timed out waiting for the TSF test condition");
            return Err(E_FAIL.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[path = "support/text_store.rs"]
mod text_store;

struct ProtectionReset(ITfLangBarItemButton);

impl Drop for ProtectionReset {
    fn drop(&mut self) {
        unsafe {
            if self.0.GetText().is_ok_and(|text| text == "保護入力 オン") {
                let _ = self
                    .0
                    .OnClick(TF_LBI_CLK_LEFT, POINT::default(), ptr::null());
            }
        }
    }
}

fn verify_protection(
    context: &ITfContext,
    thread: &ITfThreadMgr,
    keys: &ITfKeystrokeMgr,
    client: u32,
    scope: &Cell<InputScope>,
    observation: Option<&Observation>,
) -> Result<()> {
    let items: ITfLangBarItemMgr = thread.cast()?;
    let button = ProtectionReset(unsafe { items.GetItem(&diy_ime2_tsf::PROTECTION_ITEM)? }.cast()?);
    let compartment_manager: ITfCompartmentMgr = context.cast()?;
    let disable =
        unsafe { compartment_manager.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)? };
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
    let write = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
    let contents = Rc::new(Cell::new((0, [0; 2])));
    let reader: ITfEditSession = ReadDocument {
        context: context.clone(),
        contents: contents.clone(),
        caret: Rc::default(),
        decorated: Rc::default(),
    }
    .into();
    let space = WPARAM(usize::from(VK_SPACE.0));
    let x = WPARAM(usize::from(b'X'));
    let letter = WPARAM(usize::from(VK_A.0));
    let before = observation.map(Observation::dispatched);

    unsafe {
        assert_eq!(button.0.GetText()?.to_string(), "保護入力 オフ");
        disable.SetValue(client, &VARIANT::from(0_i32))?;
        assert!(keys.KeyDown(letter, LPARAM(0))?.as_bool());
        pump_until(|| Ok(keys.TestKeyDown(space, LPARAM(0))?.as_bool()))
            .inspect_err(|_| eprintln!("waiting for the Stage B Host to accept conversion"))?;
        assert!(keys.KeyDown(space, LPARAM(0))?.as_bool());
        let document = context.GetDocumentMgr()?;
        if let Some(observation) = observation {
            pump_until(|| Ok(observation.dispatched() > before.unwrap_or(0)))?;
        } else {
            pump_until(|| Ok(candidate_list(thread, &document)?.is_some()))?;
        }

        let manual_started = std::time::Instant::now();
        button
            .0
            .OnClick(TF_LBI_CLK_LEFT, POINT::default(), ptr::null())?;
        assert_eq!(button.0.GetText()?.to_string(), "保護入力 オン");
        assert!(!keys.TestKeyDown(x, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(x, LPARAM(0))?.as_bool());
        if let Some(observation) = observation {
            pump_until(|| Ok(observation.reaped() >= 1))?;
            eprintln!(
                "stage_b_manual_protection_to_host_reaped_ms={}",
                manual_started.elapsed().as_millis()
            );
        }
        let protected_count = observation.map(Observation::dispatched);
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (0, [0; 2]));
        assert!(candidate_list(thread, &document)?.is_none());

        scope.set(IS_PASSWORD);
        button
            .0
            .OnClick(TF_LBI_CLK_LEFT, POINT::default(), ptr::null())?;
        assert_eq!(button.0.GetText()?.to_string(), "保護入力 オフ");
        assert!(!keys.TestKeyDown(x, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(x, LPARAM(0))?.as_bool());
        // The application applies its ordinary fallback after TSF passes the key.
        change_selection(context, client, write, Some("x"), 1, 1)?;
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (1, [u16::from(b'x'), 0]));

        scope.set(InputScope(-1));
        assert!(!keys.TestKeyDown(x, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(x, LPARAM(0))?.as_bool());
        scope.set(IS_DEFAULT);
        disable.SetValue(client, &VARIANT::from(1_i32))?;
        assert!(!keys.KeyDown(x, LPARAM(0))?.as_bool());
        pump_messages();
        assert_eq!(observation.map(Observation::dispatched), protected_count);
        assert!(candidate_list(thread, &document)?.is_none());

        disable.SetValue(client, &VARIANT::from(0_i32))?;
        change_selection(context, client, write, Some(""), 0, 0)?;
        pump_messages();
        for key in b"KOUSUI" {
            assert!(
                keys.KeyDown(WPARAM(usize::from(*key)), LPARAM(0))?
                    .as_bool()
            );
        }
        pump_until(|| Ok(keys.TestKeyDown(space, LPARAM(0))?.as_bool()))
            .inspect_err(|_| eprintln!("waiting for the Stage B Host to accept conversion"))?;
        assert_eq!(
            observation.map(Observation::dispatched),
            protected_count,
            "old input was replayed"
        );
        assert!(keys.KeyDown(space, LPARAM(0))?.as_bool());
        pump_until(|| {
            context.RequestEditSession(client, &reader, flags)?.ok()?;
            Ok(contents.get() == (2, [0x964d, 0x6c34]))
        })?;
        assert_eq!(
            observation.map(Observation::dispatched),
            protected_count.map(|count| count + 1)
        );

        let previous = candidate_list(thread, &document)?.ok_or(E_FAIL)?;
        let reaped = observation.map(Observation::reaped);
        let automatic_started = std::time::Instant::now();
        scope.set(IS_PASSWORD);
        if let Some(observation) = observation {
            pump_until(|| Ok(observation.reaped() > reaped.unwrap_or(0)))?;
            eprintln!(
                "stage_b_automatic_protection_to_host_reaped_ms={}",
                automatic_started.elapsed().as_millis()
            );
        } else {
            pump_until(|| Ok(candidate_list(thread, &document)?.is_none()))?;
        }
        assert_eq!(button.0.GetText()?.to_string(), "保護入力 オフ");
        assert!(!keys.TestKeyDown(x, LPARAM(0))?.as_bool());
        assert!(!keys.KeyDown(x, LPARAM(0))?.as_bool());
        assert!(candidate_list(thread, &document)?.is_none());
        assert_eq!(previous.GetCount()?, 0);
        assert_eq!(
            observation.map(Observation::dispatched),
            protected_count.map(|count| count + 1)
        );
    }
    Ok(())
}
