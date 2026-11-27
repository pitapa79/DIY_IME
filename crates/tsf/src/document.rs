use std::{mem::ManuallyDrop, os::raw::c_void, ptr};

use crate::diagnostics::trace;
use windows::Win32::{
    System::{Com::CoTaskMemFree, Variant::VT_I4},
    UI::TextServices::{
        GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_INPUTSCOPE, IS_DEFAULT, ITfCompartmentMgr,
        ITfContext, ITfInputScope, ITfRange, TF_DEFAULT_SELECTION, TF_SELECTION, TS_SD_READONLY,
    },
};
use windows_core::{IUnknown, Interface};

pub(crate) fn editable_selection(context: &ITfContext, cookie: u32) -> Option<ITfRange> {
    let manager: ITfCompartmentMgr = context.cast().ok()?;
    let compartment = unsafe {
        manager
            .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)
            .ok()?
    };
    let flag = unsafe { compartment.GetValue().ok()? };
    let fields = unsafe { flag.as_raw().Anonymous.Anonymous };
    if fields.vt != VT_I4.0 || unsafe { fields.Anonymous.lVal } != 0 {
        trace!("DIY IME Stage B: keyboard compartment did not confirm enabled");
        return None;
    }
    if unsafe { context.GetStatus().ok()?.dwDynamicFlags } & TS_SD_READONLY != 0 {
        trace!("DIY IME Stage B: document is read-only");
        return None;
    }
    let mut selections = [TF_SELECTION::default()];
    let mut count = 0;
    let result =
        unsafe { context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selections, &mut count) };
    let [selection] = selections;
    let range = ManuallyDrop::into_inner(selection.range)?;
    result.ok()?;
    if count != 1 {
        return None;
    }
    let property = unsafe { context.GetAppProperty(&GUID_PROP_INPUTSCOPE).ok()? };
    let value = unsafe { property.GetValue(cookie, &range).ok()? };
    let scope: ITfInputScope = IUnknown::try_from(&value).ok()?.cast().ok()?;
    if !ordinary_scope(&scope) {
        trace!("DIY IME Stage B: InputScope did not confirm ordinary field");
        return None;
    }
    Some(range)
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

pub(crate) fn is_utf16_boundary(caret: &ITfRange, cookie: u32) -> bool {
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
