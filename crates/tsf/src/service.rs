use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use windows::Win32::{
    Foundation::{BOOL, E_FAIL, E_POINTER, E_UNEXPECTED, LPARAM, WPARAM},
    UI::TextServices::{
        IEnumTfDisplayAttributeInfo, ITfContext, ITfDisplayAttributeInfo,
        ITfDisplayAttributeProvider, ITfDisplayAttributeProvider_Impl, ITfDocumentMgr,
        ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfSource, ITfTextInputProcessor,
        ITfTextInputProcessor_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
        ITfThreadMgrEventSink_Impl, ITfUIElementMgr,
    },
};
use windows_core::{GUID, IUnknown, Interface, Result, implement};

use crate::{
    ModuleRef,
    composition::Preedit,
    display,
    host::{Bridge, Timer},
    windows_keys::key_from_message,
};

pub(crate) struct Activation {
    thread_manager: ITfThreadMgr,
    keystroke_manager: ITfKeystrokeMgr,
    pub(crate) client_id: u32,
    active: Cell<bool>,
    advised: Cell<bool>,
    foreground: Cell<bool>,
    document_subscription: Cell<Option<u32>>,
    pub(crate) preedit: Preedit,
    pub(crate) host: RefCell<Option<Bridge>>,
    timer: RefCell<Option<Timer>>,
}

impl Activation {
    fn unadvise(&self) -> Result<()> {
        self.active.set(false);
        self.timer.take();
        self.host.take();
        self.preedit.invalidate(None, self.client_id);
        let document = self.document_subscription.take().map_or(Ok(()), |cookie| {
            let source: ITfSource = self.thread_manager.cast()?;
            unsafe { source.UnadviseSink(cookie) }
        });
        let keys = if self.advised.replace(false) {
            unsafe { self.keystroke_manager.UnadviseKeyEventSink(self.client_id) }
        } else {
            Ok(())
        };
        document.and(keys)
    }

    fn observe_context(&self, context: Option<&ITfContext>) {
        if !self.preedit.matches(context) {
            self.cancel_host();
        }
        self.preedit.invalidate(context, self.client_id);
    }

    pub(crate) fn ui_manager(&self) -> Result<ITfUIElementMgr> {
        self.thread_manager.cast()
    }

    pub(crate) fn cancel_host(&self) {
        if let Ok(mut host) = self.host.try_borrow_mut()
            && let Some(host) = host.as_mut()
        {
            host.cancel();
        }
    }

    pub(crate) fn poll_host(self: &Rc<Self>) {
        if !self.active.get() {
            return;
        }
        let has_work = self
            .host
            .try_borrow()
            .is_ok_and(|host| host.as_ref().is_some_and(Bridge::has_work));
        let showing_candidates = self.preedit.showing_candidates();
        if (has_work || showing_candidates) && !self.preedit.can_deliver(self) {
            if showing_candidates {
                self.preedit.invalidate(None, self.client_id);
            }
            self.cancel_host();
        }
        let preview = {
            let Ok(mut host) = self.host.try_borrow_mut() else {
                return;
            };
            host.as_mut().and_then(Bridge::poll)
        };
        if let Some(preview) = preview {
            self.preedit.preview(self, preview);
        }
    }

    pub(crate) fn owns_focus(&self, context: &ITfContext) -> bool {
        let matching = self.active.get()
            && self.foreground.get()
            && unsafe {
                self.thread_manager
                    .IsThreadFocus()
                    .is_ok_and(|focus| focus.as_bool())
            }
            && unsafe {
                self.thread_manager
                    .GetFocus()
                    .and_then(|document| document.GetTop())
                    .is_ok_and(|current| current == *context)
            };
        matching && self.active.get() && self.foreground.get()
    }
}

#[implement(ITfTextInputProcessor, ITfDisplayAttributeProvider)]
pub(crate) struct TextService {
    _module: ModuleRef,
    activation: RefCell<Option<Rc<Activation>>>,
}

impl TextService {
    pub(crate) fn new() -> Self {
        Self {
            _module: ModuleRef::new(),
            activation: RefCell::new(None),
        }
    }

    fn deactivate(&self) -> Result<()> {
        let Some(activation) = self
            .activation
            .try_borrow_mut()
            .map_err(|_| E_UNEXPECTED)?
            .take()
        else {
            return Ok(());
        };
        activation.unadvise()
    }
}

impl Drop for TextService {
    fn drop(&mut self) {
        let _ = self.deactivate();
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, thread_manager: Option<&ITfThreadMgr>, client_id: u32) -> Result<()> {
        let manager = thread_manager.ok_or(E_POINTER)?;
        if client_id == 0
            || self
                .activation
                .try_borrow()
                .map_err(|_| E_UNEXPECTED)?
                .is_some()
        {
            return Err(E_UNEXPECTED.into());
        }
        let activation = Rc::new(Activation {
            thread_manager: manager.clone(),
            keystroke_manager: manager.cast()?,
            client_id,
            active: Cell::new(false),
            advised: Cell::new(false),
            foreground: Cell::new(false),
            document_subscription: Cell::new(None),
            preedit: Preedit::default(),
            host: RefCell::new(None),
            timer: RefCell::new(None),
        });
        let source: ITfSource = manager.cast()?;
        let document_sink: ITfThreadMgrEventSink = DocumentSink {
            _module: ModuleRef::new(),
            activation: Rc::downgrade(&activation),
        }
        .into();
        let identity: IUnknown = document_sink.cast()?;
        let cookie = unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &identity)? };
        activation.document_subscription.set(Some(cookie));
        *self.activation.try_borrow_mut().map_err(|_| E_UNEXPECTED)? = Some(activation.clone());
        let sink: ITfKeyEventSink = KeySink {
            _module: ModuleRef::new(),
            activation: Rc::downgrade(&activation),
        }
        .into();
        let registered = unsafe {
            activation
                .keystroke_manager
                .AdviseKeyEventSink(client_id, &sink, true)
        };
        if let Err(error) = registered {
            let mut current = self.activation.try_borrow_mut().map_err(|_| E_UNEXPECTED)?;
            if current
                .as_ref()
                .is_some_and(|current| Rc::ptr_eq(current, &activation))
            {
                *current = None;
            }
            drop(current);
            let _ = activation.unadvise();
            return Err(error);
        }
        activation.advised.set(true);
        if !self
            .activation
            .try_borrow()
            .map_err(|_| E_UNEXPECTED)?
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, &activation))
        {
            let _ = activation.unadvise();
            return Err(E_FAIL.into());
        }
        activation.active.set(true);
        if let Some(host) = Bridge::open()
            && let Some(timer) = Timer::open(&activation)
        {
            activation.host.replace(Some(host));
            activation.timer.replace(Some(timer));
        }
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        self.deactivate()
    }
}

#[implement(ITfKeyEventSink)]
struct KeySink {
    _module: ModuleRef,
    activation: Weak<Activation>,
}

impl ITfKeyEventSink_Impl for KeySink_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        if let Some(activation) = self.activation.upgrade() {
            activation.foreground.set(foreground.as_bool());
            if !foreground.as_bool() {
                activation.cancel_host();
                activation.preedit.invalidate(None, activation.client_id);
            }
        }
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        context: Option<&ITfContext>,
        key: WPARAM,
        message: LPARAM,
    ) -> Result<BOOL> {
        Ok(context
            .zip(self.activation.upgrade())
            .is_some_and(|(context, activation)| {
                key_from_message(key, message)
                    .is_some_and(|key| activation.preedit.key(&activation, context, key, false))
            })
            .into())
    }

    fn OnTestKeyUp(&self, _: Option<&ITfContext>, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnKeyDown(
        &self,
        context: Option<&ITfContext>,
        key: WPARAM,
        message: LPARAM,
    ) -> Result<BOOL> {
        Ok(context
            .zip(self.activation.upgrade())
            .is_some_and(|(context, activation)| {
                key_from_message(key, message)
                    .is_some_and(|key| activation.preedit.key(&activation, context, key, true))
            })
            .into())
    }

    fn OnKeyUp(&self, _: Option<&ITfContext>, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnPreservedKey(&self, _: Option<&ITfContext>, _: *const GUID) -> Result<BOOL> {
        Ok(false.into())
    }
}

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(display::attributes(false))
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        display::attribute(guid)
    }
}

#[implement(ITfThreadMgrEventSink)]
struct DocumentSink {
    _module: ModuleRef,
    activation: Weak<Activation>,
}

impl DocumentSink_Impl {
    fn refresh(&self) {
        if let Some(activation) = self.activation.upgrade() {
            let current = unsafe {
                activation
                    .thread_manager
                    .GetFocus()
                    .and_then(|doc| doc.GetTop())
                    .ok()
            };
            activation.observe_context(current.as_ref());
        }
    }
}

impl ITfThreadMgrEventSink_Impl for DocumentSink_Impl {
    fn OnInitDocumentMgr(&self, _: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _: Option<&ITfDocumentMgr>) -> Result<()> {
        self.refresh();
        Ok(())
    }

    fn OnSetFocus(
        &self,
        current: Option<&ITfDocumentMgr>,
        _: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        if let Some(activation) = self.activation.upgrade() {
            let context = current.and_then(|doc| unsafe { doc.GetTop().ok() });
            activation.observe_context(context.as_ref());
        }
        Ok(())
    }

    fn OnPushContext(&self, _: Option<&ITfContext>) -> Result<()> {
        self.refresh();
        Ok(())
    }

    fn OnPopContext(&self, context: Option<&ITfContext>) -> Result<()> {
        if let Some(activation) = self.activation.upgrade() {
            let current = unsafe {
                activation
                    .thread_manager
                    .GetFocus()
                    .and_then(|doc| doc.GetTop())
                    .ok()
            };
            activation
                .observe_context(current.as_ref().filter(|current| Some(*current) != context));
        }
        Ok(())
    }
}
