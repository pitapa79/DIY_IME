use std::{cell::Cell, mem::ManuallyDrop, os::raw::c_void, ptr, rc::Rc};

use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::{Com::CoTaskMemFree, Variant::VT_I4},
    UI::TextServices::{
        GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_INPUTSCOPE, INSERT_TEXT_AT_SELECTION_FLAGS,
        IS_DEFAULT, ITfCompartmentMgr, ITfContext, ITfEditSession, ITfEditSession_Impl,
        ITfInputScope, ITfInsertAtSelection, ITfRange, TF_CONTEXT_EDIT_CONTEXT_FLAGS,
        TF_DEFAULT_SELECTION, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
        TS_SD_READONLY,
    },
};
use windows_core::{IUnknown, Interface, Result, implement};

use crate::{
    ModuleRef,
    keymap::{self, Action, Context},
    service::Activation,
    windows_keys::key_from_message,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    Pending,
    Rejected,
    Handled,
    Disarmed,
}

pub(crate) fn probe_key(
    activation: &Rc<Activation>,
    context: &ITfContext,
    key: WPARAM,
    message: LPARAM,
    insert: bool,
) -> bool {
    if !activation.owns_focus(context) {
        return false;
    }
    let action = key_from_message(key, message).and_then(|key| {
        keymap::action(
            key,
            Context {
                editable: true,
                preedit: "",
                selecting_candidate: false,
            },
        )
    });
    // The stage A diagnostic service still writes only its original probe character.
    if action != Some(Action::Append('a')) {
        return false;
    }
    let outcome = Rc::new(Cell::new(ProbeState::Pending));
    let session: ITfEditSession = ProbeSession {
        _module: ModuleRef::new(),
        activation: activation.clone(),
        context: context.clone(),
        outcome: outcome.clone(),
        insert,
    }
    .into();
    let access = if insert { TF_ES_READWRITE } else { TF_ES_READ };
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | access.0);
    // An HRESULT can report failure after the callback attempted a write.
    // The callback's handled flag avoids forwarding that key a second time.
    let _ = unsafe { context.RequestEditSession(activation.client_id, &session, flags) };
    outcome.replace(ProbeState::Disarmed) == ProbeState::Handled
}

#[implement(ITfEditSession)]
struct ProbeSession {
    _module: ModuleRef,
    activation: Rc<Activation>,
    context: ITfContext,
    outcome: Rc<Cell<ProbeState>>,
    insert: bool,
}

impl ITfEditSession_Impl for ProbeSession_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        if self.outcome.get() != ProbeState::Pending {
            return Ok(());
        }
        self.outcome.set(ProbeState::Rejected);
        if !self.activation.owns_focus(&self.context) {
            return Ok(());
        }
        let Some(selection) = editable_caret(&self.context, cookie) else {
            return Ok(());
        };
        if !is_utf16_boundary(&selection, cookie) || !self.activation.owns_focus(&self.context) {
            return Ok(());
        }
        if self.insert {
            let insert: ITfInsertAtSelection = self.context.cast()?;
            self.outcome.set(ProbeState::Handled);
            // Only the literal probe character. Real conversion belongs to the Node Host.
            unsafe {
                insert.InsertTextAtSelection(
                    cookie,
                    INSERT_TEXT_AT_SELECTION_FLAGS(0),
                    &[u16::from(b'a')],
                )?
            };
        } else {
            self.outcome.set(ProbeState::Handled);
        }
        Ok(())
    }
}

fn editable_caret(context: &ITfContext, cookie: u32) -> Option<ITfRange> {
    let manager: ITfCompartmentMgr = context.cast().ok()?;
    let compartment = unsafe {
        manager
            .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)
            .ok()?
    };
    let flag = unsafe { compartment.GetValue().ok()? };
    let fields = unsafe { flag.as_raw().Anonymous.Anonymous };
    if fields.vt != VT_I4.0
        || unsafe { fields.Anonymous.lVal } != 0
        || unsafe { context.GetStatus().ok()?.dwDynamicFlags } & TS_SD_READONLY != 0
    {
        return None;
    }
    let mut selections = [TF_SELECTION::default()];
    let mut count = 0;
    let result =
        unsafe { context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selections, &mut count) };
    let [selection] = selections;
    let range = ManuallyDrop::into_inner(selection.range)?;
    result.ok()?;
    if count != 1 || !unsafe { range.IsEmpty(cookie).ok()?.as_bool() } {
        return None;
    }
    let property = unsafe { context.GetAppProperty(&GUID_PROP_INPUTSCOPE).ok()? };
    let value = unsafe { property.GetValue(cookie, &range).ok()? };
    let scope: ITfInputScope = IUnknown::try_from(&value).ok()?.cast().ok()?;
    ordinary_scope(&scope).then_some(range)
}

fn ordinary_scope(scope: &ITfInputScope) -> bool {
    let mut scopes = ptr::null_mut();
    let mut count = 0;
    let result = unsafe { scope.GetInputScopes(&mut scopes, &mut count) };
    let ordinary =
        result.is_ok() && count == 1 && !scopes.is_null() && unsafe { *scopes == IS_DEFAULT };
    unsafe { CoTaskMemFree(Some(scopes.cast::<c_void>())) };
    ordinary
}

fn is_utf16_boundary(caret: &ITfRange, cookie: u32) -> bool {
    let Ok(witness) = (unsafe { caret.Clone() }) else {
        return false;
    };
    let mut left = 0;
    let mut right = 0;
    // Read at most one UTF-16 code unit on either side, only in an ordinary field.
    if unsafe {
        witness
            .ShiftStart(cookie, -1, &mut left, ptr::null())
            .is_err()
    } || unsafe {
        witness
            .ShiftEnd(cookie, 1, &mut right, ptr::null())
            .is_err()
    } || !(-1..=0).contains(&left)
        || !(0..=1).contains(&right)
    {
        return false;
    }
    let mut text = [0; 2];
    let mut read = 0;
    if unsafe { witness.GetText(cookie, 0, &mut text, &mut read).is_err() }
        || read != (right - left) as u32
    {
        return false;
    }
    (left == 0 || !(0xd800..=0xdbff).contains(&text[0]))
        && (right == 0 || !(0xdc00..=0xdfff).contains(&text[(-left) as usize]))
}
