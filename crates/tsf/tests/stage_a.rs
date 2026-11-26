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
            CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITfDisplayAttributeProvider,
            ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInputProcessorProfileMgr,
            ITfKeystrokeMgr, ITfTextInputProcessor, ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS,
            TF_ES_READWRITE, TF_ES_SYNC, TF_INPUTPROCESSORPROFILE, TF_IPPMF_ENABLEPROFILE,
            TF_IPPMF_FORPROCESS, TF_POPF_ALL, TF_PROFILETYPE_INPUTPROCESSOR, TF_RP_LOCALPROCESS,
            TF_URP_LOCALPROCESS,
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

#[test]
fn private_window_selects_and_deactivates_the_test_service_through_tsf() -> Result<()> {
    exercise_test_window(false, false, false)
}

#[test]
#[ignore = "requires a foreground window on an interactive x64 Windows desktop"]
fn interactive_app_routes_ordinary_and_protected_input() -> Result<()> {
    exercise_test_window(true, false, false)
}

#[test]
#[ignore = "requires the temporary machine profile enabled for the current user and desktop foreground"]
fn interactive_machine_registered_app_routes_input() -> Result<()> {
    exercise_test_window(true, true, false)
}

#[test]
#[ignore = "requires the approved isolated Host fixture and an interactive Windows desktop"]
fn interactive_app_converts_through_official_host() -> Result<()> {
    let exe = std::env::var_os("DIY_IME_TEST_HOST").expect("test Host executable");
    let config = std::env::var_os("DIY_IME_TEST_HOST_CONFIG").expect("test Host configuration");
    diy_ime2_tsf::set_test_host_configuration(exe.into(), config.into());
    exercise_test_window(true, false, true)
}

fn exercise_test_window(
    interactive: bool,
    machine_registered: bool,
    conversion: bool,
) -> Result<()> {
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
        if conversion {
            verify_conversion(context, &document, &thread_manager, &keys, client)?;
        } else {
            verify_key_edit(context, &keys, client, &scope, &deny_writes)?;
            verify_document_switch(context, &document, &thread_manager, &keys, client)?;
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
    if conversion {
        pump_until(|| Ok(DllCanUnloadNow() == S_OK))?;
    }
    assert_eq!(DllCanUnloadNow(), S_OK);
    Ok(())
}

use windows::Win32::{
    Foundation::{E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, LPARAM, WPARAM},
    System::Com::CoTaskMemAlloc,
    UI::{
        Input::KeyboardAndMouse::{
            VK_A, VK_B, VK_BACK, VK_DOWN, VK_ESCAPE, VK_SPACE, VK_TAB, VK_UP,
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
        let other = thread.CreateDocumentMgr()?;
        thread.SetFocus(&other)?;
        thread.SetFocus(document)?;
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(
            contents.get(),
            (0, [0; 2]),
            "focus loss must cancel old preedit"
        );
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
        pump_until(|| Ok(keys.TestKeyDown(space, LPARAM(0))?.as_bool()))?;
        assert!(keys.KeyDown(space, LPARAM(0))?.as_bool());
        pump_until(|| Ok(candidate_list(thread, document)?.is_some()))?;
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
        pump_until(|| Ok(candidate_list(thread, document)?.is_some()))?;
        let old = candidate_list(thread, document)?.ok_or(E_FAIL)?;
        let other = thread.CreateDocumentMgr()?;
        thread.SetFocus(&other)?;
        thread.SetFocus(document)?;
        context.RequestEditSession(client, &reader, flags)?.ok()?;
        assert_eq!(contents.get(), (0, [0; 2]));
        assert!(candidate_list(thread, document)?.is_none());
        assert_eq!(old.GetCount()?, 0);
        assert!(old.GetString(0).is_err());
        assert!(!keys.TestKeyDown(space, LPARAM(0))?.as_bool());
    }
    Ok(())
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
}

fn pump_until(mut ready: impl FnMut() -> Result<bool>) -> Result<()> {
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if ready()? {
                return Ok(());
            }
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
