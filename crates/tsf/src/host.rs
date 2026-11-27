use std::{
    cell::RefCell,
    collections::BTreeMap,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    rc::{Rc, Weak},
};

use diy_ime2_ime_host::{
    Conversion, Delivery, DocumentStamp, InputScope, InputState, Invalidation, RequestId,
    external::{Background, Configuration, Event, Observation, Stopped},
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
    static TEST_CONFIGURATION: RefCell<Option<(Configuration, Observation)>> = const { RefCell::new(None) };
    static TIMERS: RefCell<BTreeMap<usize, Weak<Activation>>> = const { RefCell::new(BTreeMap::new()) };
}

/// Rust test harness entry. COM exposes no configuration or permission grant.
pub fn set_test_host_configuration(executable: PathBuf, file: PathBuf) -> Observation {
    let observation = Observation::default();
    TEST_CONFIGURATION.with(|setting| {
        setting.replace(Some((
            Configuration { executable, file },
            observation.clone(),
        )))
    });
    observation
}

fn installed_configuration() -> Option<Configuration> {
    let mut module = HMODULE::default();
    let mut path = [0; 1024];
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(installed_configuration as *const () as *const u16),
            &mut module,
        )
    }
    .inspect_err(|_| {
        crate::diagnostics::trace!("DIY IME Stage B: Host module lookup failed");
    })
    .ok()?;
    let count = unsafe { GetModuleFileNameW(module, &mut path) as usize };
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
        .inspect_err(|_| {
            crate::diagnostics::trace!("DIY IME Stage B: Host timer window creation failed");
        })
        .ok()?;
        let timer = Self(window);
        if !unsafe { SetWindowSubclass(window, Some(tick), 1, 0).as_bool() } {
            crate::diagnostics::trace!("DIY IME Stage B: Host timer subclass installation failed");
            return None;
        }
        if unsafe { SetTimer(window, 1, 20, None) } == 0 {
            crate::diagnostics::trace!("DIY IME Stage B: Host timer scheduling failed");
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
    config: Configuration,
    observer: Observation,
    worker: Option<Background>,
    stopping: Option<Stopped>,
    protected: bool,
    input: Option<InputState>,
    invalidation: Option<Invalidation>,
    queued: Option<PendingInput>,
    submitted: Option<Submission>,
    disconnected: bool,
}

struct PendingInput {
    stamp: DocumentStamp,
    text: String,
    finalize: bool,
}

struct Submission {
    stamp: DocumentStamp,
    request: RequestId,
    finalize: bool,
}

pub(crate) struct Preview {
    pub(crate) finalize: bool,
    pub(crate) stamp: DocumentStamp,
    pub(crate) text: String,
    pub(crate) candidates: Vec<String>,
}

impl Bridge {
    pub(crate) fn open(protected: bool) -> Option<Self> {
        let (config, observer) = TEST_CONFIGURATION
            .with(|setting| setting.take())
            .or_else(|| installed_configuration().map(|config| (config, Observation::default())))?;
        Some(Self {
            config,
            observer,
            worker: None,
            stopping: None,
            protected,
            input: None,
            invalidation: None,
            queued: None,
            submitted: None,
            disconnected: false,
        })
    }

    pub(crate) fn accepts_request(&self) -> bool {
        !self.protected && !self.disconnected
    }

    pub(crate) fn has_work(&self) -> bool {
        self.queued.is_some() || self.submitted.is_some()
    }

    pub(crate) fn set_protection(&mut self, enabled: bool) {
        if self.protected == enabled {
            return;
        }
        self.protected = enabled;
        if enabled {
            if let Some(worker) = self.worker.take() {
                self.stopping = Some(worker.request_stop());
            }
            self.queued = None;
            self.submitted = None;
            self.input = None;
            self.invalidation = None;
        }
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

    pub(crate) fn convert(&mut self, stamp: DocumentStamp, text: String, finalize: bool) {
        if !self.accepts_request() {
            return;
        }
        if let Some(input) = self.input.as_mut()
            && let Some(invalidation) = input.observe_document(Some(stamp), InputScope::Editable)
        {
            self.invalidate(invalidation);
        }
        self.queued = Some(PendingInput {
            stamp,
            text,
            finalize,
        });
    }

    fn invalidate(&mut self, invalidation: Invalidation) {
        if let Some(revision) = invalidation.revision
            && let Some(worker) = self.worker.as_ref()
        {
            worker.invalidate(revision);
        }
        self.invalidation = Some(invalidation);
    }

    pub(crate) fn poll(&mut self) -> Option<Preview> {
        if self.disconnected || self.protected {
            return None;
        }
        if self.worker.is_none() {
            if self
                .stopping
                .as_ref()
                .is_some_and(|stopping| !stopping.confirmed())
            {
                return None;
            }
            self.stopping = None;
            self.worker = Background::spawn_observed(
                self.config.clone(),
                self.observer.clone(),
                ModuleRef::new(),
            )
            .ok();
            if self.worker.is_none() {
                crate::diagnostics::trace!("DIY IME Stage B: Host background creation failed");
                self.disconnected = true;
            }
        }
        let worker = self.worker.as_ref()?;
        let acknowledged = worker.acknowledged();
        match worker.poll() {
            Ok(Some(Event::Ready(plan))) => {
                crate::diagnostics::trace!("DIY IME Stage B: Host bridge received ready event");
                let pending = self.queued.take();
                self.input = Some(InputState::new(plan));
                if let Some(pending) = pending {
                    self.convert(pending.stamp, pending.text, pending.finalize);
                }
            }
            Ok(Some(Event::Started(key))) => {
                if let Some(input) = self.input.as_mut()
                    && let Some(submission) = self.submitted.as_ref()
                {
                    let _ = input.accept_run(&submission.request, key);
                }
            }
            Ok(Some(Event::Converted(key, result))) => {
                if let Some(input) = self.input.as_mut()
                    && let Some(submission) = self.submitted.as_ref()
                {
                    let text = match &result.conversion {
                        Conversion::Text(text) => Some(text.clone()),
                        _ => None,
                    };
                    let stamp = submission.stamp;
                    let finalize = submission.finalize;
                    let delivery = input.receive(&submission.request, &key, result.conversion);
                    if delivery.is_ok() {
                        self.submitted = None;
                    }
                    if delivery == Ok(Delivery::Ready) {
                        return text.map(|text| Preview {
                            stamp,
                            finalize,
                            text,
                            candidates: result.candidates,
                        });
                    }
                }
            }
            Ok(Some(Event::Unavailable)) | Err(_) => {
                crate::diagnostics::trace!("DIY IME Stage B: Host bridge disconnected");
                self.cancel();
                self.disconnected = true;
                return None;
            }
            Ok(None) => {}
        }
        let input = self.input.as_mut()?;
        if let Some(invalidation) = self.invalidation.as_ref()
            && invalidation
                .revision
                .is_some_and(|revision| acknowledged >= revision)
        {
            input.acknowledge_invalidation(invalidation);
            self.invalidation = None;
        }
        if input.accepts_input()
            && let Some(pending) = self.queued.take()
        {
            let request = input.replace_input(pending.text).and_then(|request| {
                if pending.finalize {
                    input.commit()
                } else {
                    Ok(request)
                }
            });
            if let Ok(request) = request
                && input.dispatch(&request, |_, input| {
                    self.worker
                        .as_ref()
                        .is_some_and(|worker| worker.submit(input.clone()))
                }) == Ok(true)
            {
                self.submitted = Some(Submission {
                    stamp: pending.stamp,
                    request,
                    finalize: pending.finalize,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starting_bridge() -> Bridge {
        set_test_host_configuration(
            PathBuf::from("synthetic-host.exe"),
            PathBuf::from("synthetic-config.json"),
        );
        Bridge::open(false).expect("synthetic test configuration")
    }

    fn stamp() -> DocumentStamp {
        DocumentStamp {
            focus: 1,
            context: 1,
            revision: 1,
            selection: 1,
        }
    }

    #[test]
    fn conversion_before_host_ready_is_retained_until_cancelled() {
        let mut bridge = starting_bridge();
        assert!(bridge.input.is_none());
        assert!(bridge.accepts_request());
        bridge.convert(stamp(), "kousui".into(), false);
        let pending = bridge
            .queued
            .as_ref()
            .expect("first Space must retain conversion input");
        assert_eq!(pending.text, "kousui");
        assert_eq!(pending.stamp, stamp());
        assert!(bridge.has_work());
        bridge.cancel();
        assert!(
            !bridge.has_work(),
            "cancel before readiness must discard the pending input"
        );
    }

    #[test]
    fn protection_and_disconnect_discard_startup_requests() {
        let mut bridge = starting_bridge();
        bridge.convert(stamp(), "kousui".into(), true);
        assert!(
            bridge
                .queued
                .as_ref()
                .is_some_and(|pending| pending.finalize)
        );
        bridge.set_protection(true);
        assert!(!bridge.has_work());
        bridge.convert(stamp(), "synthetic-protected".into(), false);
        assert!(!bridge.has_work(), "protected input must never be queued");
        bridge.set_protection(false);
        assert!(
            !bridge.has_work(),
            "unblocking must not restore the old request"
        );
        bridge.disconnected = true;
        bridge.convert(stamp(), "synthetic-disconnected".into(), false);
        assert!(!bridge.has_work());
    }
}
