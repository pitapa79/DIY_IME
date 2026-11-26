use std::{
    cell::RefCell,
    collections::BTreeMap,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    rc::{Rc, Weak},
};

use diy_ime2_ime_host::{
    Conversion, Delivery, DocumentStamp, InputScope, InputState, Invalidation, RequestId,
    external::{Background, Configuration, Event},
};
use windows::Win32::{
    Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    },
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, HWND_MESSAGE, KillTimer, SetTimer, WINDOW_EX_STYLE,
            WM_TIMER, WS_CHILD,
        },
    },
};
use windows_core::PCWSTR;

use crate::{ModuleRef, service::Activation};

thread_local! {
    static TEST_CONFIGURATION: RefCell<Option<Configuration>> = const { RefCell::new(None) };
    static TIMERS: RefCell<BTreeMap<usize, Weak<Activation>>> = const { RefCell::new(BTreeMap::new()) };
}

/// Rust test harness entry. COM exposes no configuration or permission grant.
pub fn set_test_host_configuration(executable: PathBuf, file: PathBuf) {
    TEST_CONFIGURATION.with(|setting| setting.replace(Some(Configuration { executable, file })));
}

fn installed_configuration() -> Option<Configuration> {
    let mut module = HMODULE::default();
    let mut path = [0; 1024];
    let count = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(installed_configuration as *const () as *const u16),
            &mut module,
        )
        .ok()?;
        GetModuleFileNameW(module, &mut path) as usize
    };
    if count == 0 || count >= path.len() {
        return None;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&path[..count]));
    let name = path.file_name()?.to_str()?;
    if !name.eq_ignore_ascii_case("diy_ime2_tsf.dll") {
        return None;
    }
    let directory = path.parent()?;
    Some(Configuration {
        executable: directory.join("diy-ime2-external-host.exe"),
        file: directory.join("ime-execution.json"),
    })
}

pub(crate) struct Timer(HWND);

impl Timer {
    pub(crate) fn open(owner: &Rc<Activation>) -> Option<Self> {
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows_core::w!("STATIC"),
                PCWSTR::null(),
                WS_CHILD,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                None,
                None,
            )
        }
        .ok()?;
        let timer = Self(window);
        if !unsafe { SetWindowSubclass(window, Some(tick), 1, 0).as_bool() }
            || unsafe { SetTimer(window, 1, 20, None) } == 0
        {
            return None;
        }
        TIMERS.with(|timers| {
            timers
                .borrow_mut()
                .insert(window.0 as usize, Rc::downgrade(owner))
        });
        Some(timer)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        TIMERS.with(|timers| timers.borrow_mut().remove(&(self.0.0 as usize)));
        unsafe {
            let _ = KillTimer(self.0, 1);
            let _ = RemoveWindowSubclass(self.0, Some(tick), 1);
            let _ = DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn tick(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == WM_TIMER && wparam.0 == 1 {
        let owner = TIMERS.with(|timers| {
            timers
                .try_borrow()
                .ok()?
                .get(&(window.0 as usize))
                .and_then(Weak::upgrade)
        });
        if let Some(owner) = owner {
            owner.poll_host();
        }
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

pub(crate) struct Bridge {
    worker: Background,
    input: Option<InputState>,
    invalidation: Option<Invalidation>,
    queued: Option<(DocumentStamp, String)>,
    submitted: Option<(DocumentStamp, RequestId)>,
    disconnected: bool,
}

pub(crate) struct Preview {
    pub(crate) stamp: DocumentStamp,
    pub(crate) text: String,
    pub(crate) candidates: Vec<String>,
}

impl Bridge {
    pub(crate) fn open() -> Option<Self> {
        let config = TEST_CONFIGURATION
            .with(|setting| setting.take())
            .or_else(installed_configuration)?;
        let worker = Background::spawn(config, ModuleRef::new()).ok()?;
        Some(Self {
            worker,
            input: None,
            invalidation: None,
            queued: None,
            submitted: None,
            disconnected: false,
        })
    }

    pub(crate) fn available(&self) -> bool {
        !self.disconnected && self.input.is_some()
    }

    pub(crate) fn has_work(&self) -> bool {
        self.queued.is_some() || self.submitted.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        self.queued = None;
        self.submitted = None;
        if let Some(input) = self.input.as_mut()
            && let Some(invalidation) = input.observe_document(None, InputScope::Unknown)
        {
            self.invalidate(invalidation);
        }
    }

    pub(crate) fn convert(&mut self, stamp: DocumentStamp, text: String) {
        if let Some(input) = self.input.as_mut() {
            if let Some(invalidation) = input.observe_document(Some(stamp), InputScope::Editable) {
                self.invalidate(invalidation);
            }
            self.queued = Some((stamp, text));
        }
    }

    fn invalidate(&mut self, invalidation: Invalidation) {
        if let Some(revision) = invalidation.revision {
            self.worker.invalidate(revision);
        }
        self.invalidation = Some(invalidation);
    }

    pub(crate) fn poll(&mut self) -> Option<Preview> {
        match self.worker.poll() {
            Ok(Some(Event::Ready(plan))) => self.input = Some(InputState::new(plan)),
            Ok(Some(Event::Started(key))) => {
                if let Some(input) = self.input.as_mut()
                    && let Some((_, request)) = self.submitted.as_ref()
                {
                    let _ = input.accept_run(request, key);
                }
            }
            Ok(Some(Event::Converted(key, result))) => {
                if let Some(input) = self.input.as_mut()
                    && let Some((stamp, request)) = self.submitted.as_ref()
                {
                    let text = match &result.conversion {
                        Conversion::Text(text) => Some(text.clone()),
                        _ => None,
                    };
                    let stamp = *stamp;
                    let delivery = input.receive(request, &key, result.conversion);
                    if delivery.is_ok() {
                        self.submitted = None;
                    }
                    if delivery == Ok(Delivery::Ready) {
                        return text.map(|text| Preview {
                            stamp,
                            text,
                            candidates: result.candidates,
                        });
                    }
                }
            }
            Ok(Some(Event::Unavailable)) | Err(_) => {
                self.cancel();
                self.disconnected = true;
            }
            Ok(None) => {}
        }
        let input = self.input.as_mut()?;
        if let Some(invalidation) = self.invalidation.as_ref()
            && invalidation
                .revision
                .is_some_and(|revision| self.worker.acknowledged() >= revision)
        {
            input.acknowledge_invalidation(invalidation);
            self.invalidation = None;
        }
        if input.accepts_input()
            && let Some((stamp, text)) = self.queued.take()
            && let Ok(request) = input.replace_input(text)
            && input.dispatch(&request, |_, input| self.worker.submit(input.clone())) == Ok(true)
        {
            self.submitted = Some((stamp, request));
        }
        None
    }
}
