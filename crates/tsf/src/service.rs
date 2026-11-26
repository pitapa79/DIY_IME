use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use windows::Win32::{
    Foundation::{BOOL, E_FAIL, E_POINTER, E_UNEXPECTED, LPARAM, WPARAM},
    UI::TextServices::{
        ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor,
        ITfTextInputProcessor_Impl, ITfThreadMgr,
    },
};
use windows_core::{GUID, Interface, Result, implement};

use crate::ModuleRef;

pub(crate) struct Activation {
    thread_manager: ITfThreadMgr,
    keystroke_manager: ITfKeystrokeMgr,
    pub(crate) client_id: u32,
    active: Cell<bool>,
    advised: Cell<bool>,
    foreground: Cell<bool>,
}

impl Activation {
    fn unadvise(&self) -> Result<()> {
        self.active.set(false);
        if self.advised.replace(false) {
            unsafe { self.keystroke_manager.UnadviseKeyEventSink(self.client_id) }
        } else {
            Ok(())
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

#[implement(ITfTextInputProcessor)]
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
        });
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
        }
        Ok(())
    }

    fn OnTestKeyDown(&self, context: Option<&ITfContext>, key: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(context
            .zip(self.activation.upgrade())
            .is_some_and(|(context, activation)| {
                crate::probe::probe_key(&activation, context, key, false)
            })
            .into())
    }

    fn OnTestKeyUp(&self, _: Option<&ITfContext>, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnKeyDown(&self, context: Option<&ITfContext>, key: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(context
            .zip(self.activation.upgrade())
            .is_some_and(|(context, activation)| {
                crate::probe::probe_key(&activation, context, key, true)
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
