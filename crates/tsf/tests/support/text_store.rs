#![allow(non_snake_case)]

use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    ptr,
    rc::Rc,
    slice,
};

use windows::Win32::{
    Foundation::{BOOL, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HWND, POINT, RECT},
    System::Com::{FORMATETC, IDataObject},
    UI::TextServices::{
        GUID_PROP_INPUTSCOPE, ITextStoreACP, ITextStoreACP_Impl, ITextStoreACPSink, ITfInputScope,
        InputScope, TS_AE_END, TS_ATTRVAL, TS_DEFAULT_SELECTION, TS_E_NOLOCK, TS_E_SYNCHRONOUS,
        TS_IAS_QUERYONLY, TS_LF_READWRITE, TS_RT_PLAIN, TS_RUNINFO, TS_SELECTION_ACP,
        TS_SELECTIONSTYLE, TS_STATUS, TS_TEXTCHANGE,
    },
};
use windows_core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, PWSTR, Result, VARIANT, implement};

use super::AppScope;

#[implement(ITextStoreACP)]
pub(super) struct Store {
    window: HWND,
    scope: Rc<Cell<InputScope>>,
    deny_writes: Rc<Cell<bool>>,
    deny_next_write: Rc<Cell<bool>>,
    sink: RefCell<Option<ITextStoreACPSink>>,
    text: RefCell<Vec<u16>>,
    selection: Cell<TS_SELECTION_ACP>,
    lock: Cell<u32>,
    scope_requested: Cell<bool>,
}

impl Store {
    pub(super) fn new(
        window: HWND,
        scope: Rc<Cell<InputScope>>,
        deny_writes: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            window,
            scope,
            deny_writes,
            deny_next_write: Rc::default(),
            sink: RefCell::default(),
            text: RefCell::default(),
            selection: Cell::default(),
            lock: Cell::default(),
            scope_requested: Cell::default(),
        }
    }

    pub(super) fn reject_one_write_with(mut self, control: Rc<Cell<bool>>) -> Self {
        self.deny_next_write = control;
        self
    }

    fn read_access(&self) -> Result<()> {
        if self.lock.get() == 0 {
            Err(TS_E_NOLOCK.into())
        } else {
            Ok(())
        }
    }

    fn write_access(&self) -> Result<()> {
        if self.lock.get() & TS_LF_READWRITE.0 != TS_LF_READWRITE.0 {
            Err(TS_E_NOLOCK.into())
        } else {
            Ok(())
        }
    }

    fn range(&self, start: i32, end: i32) -> Result<std::ops::Range<usize>> {
        let length = self.text.borrow().len();
        if !(0 <= start && start <= end && end as usize <= length) {
            return Err(E_INVALIDARG.into());
        }
        Ok(start as usize..end as usize)
    }

    fn edit(&self, start: i32, end: i32, text: PCWSTR, length: u32) -> Result<TS_TEXTCHANGE> {
        self.write_access()?;
        let range = self.range(start, end)?;
        if length > 32 || (length > 0 && text.is_null()) {
            return Err(E_INVALIDARG.into());
        }
        let units = if length == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(text.0, length as usize) }
        };
        self.text.borrow_mut().splice(range, units.iter().copied());
        let position = start + length as i32;
        self.selection.set(TS_SELECTION_ACP {
            acpStart: position,
            acpEnd: position,
            style: TS_SELECTIONSTYLE {
                ase: TS_AE_END,
                fInterimChar: false.into(),
            },
        });
        Ok(TS_TEXTCHANGE {
            acpStart: start,
            acpOldEnd: end,
            acpNewEnd: position,
        })
    }

    fn request_scope(&self, count: u32, attributes: *const GUID) {
        let wants_scope = count == 0
            || (!attributes.is_null()
                && unsafe { slice::from_raw_parts(attributes, count as usize) }
                    .contains(&GUID_PROP_INPUTSCOPE));
        self.scope_requested.set(wants_scope);
    }
}

impl ITextStoreACP_Impl for Store_Impl {
    fn AdviseSink(&self, _: *const GUID, sink: Option<&IUnknown>, _: u32) -> Result<()> {
        *self.sink.borrow_mut() = Some(sink.ok_or(E_POINTER)?.cast()?);
        Ok(())
    }

    fn UnadviseSink(&self, _: Option<&IUnknown>) -> Result<()> {
        self.sink.borrow_mut().take();
        Ok(())
    }

    fn RequestLock(&self, flags: u32) -> Result<HRESULT> {
        if self.lock.get() != 0
            || (flags & TS_LF_READWRITE.0 == TS_LF_READWRITE.0
                && (self.deny_writes.get() || self.deny_next_write.replace(false)))
        {
            return Ok(TS_E_SYNCHRONOUS);
        }
        let sink = self.sink.borrow().clone().ok_or(E_FAIL)?;
        self.lock.set(flags);
        let outcome = unsafe {
            sink.OnLockGranted(windows::Win32::UI::TextServices::TEXT_STORE_LOCK_FLAGS(
                flags,
            ))
        };
        self.lock.set(0);
        Ok(outcome.into())
    }

    fn GetStatus(&self) -> Result<TS_STATUS> {
        Ok(TS_STATUS::default())
    }

    fn QueryInsert(
        &self,
        start: i32,
        end: i32,
        _: u32,
        result_start: *mut i32,
        result_end: *mut i32,
    ) -> Result<()> {
        self.range(start, end)?;
        if result_start.is_null() || result_end.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            result_start.write(start);
            result_end.write(end);
        }
        Ok(())
    }

    fn GetSelection(
        &self,
        index: u32,
        count: u32,
        selection: *mut TS_SELECTION_ACP,
        fetched: *mut u32,
    ) -> Result<()> {
        self.read_access()?;
        if selection.is_null() || fetched.is_null() {
            return Err(E_POINTER.into());
        }
        let selected = count != 0 && matches!(index, 0 | TS_DEFAULT_SELECTION);
        unsafe {
            fetched.write(u32::from(selected));
            if selected {
                selection.write(self.selection.get());
            }
        }
        Ok(())
    }

    fn SetSelection(&self, count: u32, selection: *const TS_SELECTION_ACP) -> Result<()> {
        self.write_access()?;
        if count != 1 || selection.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let selection = unsafe { *selection };
        self.range(selection.acpStart, selection.acpEnd)?;
        self.selection.set(selection);
        Ok(())
    }

    fn GetText(
        &self,
        start: i32,
        end: i32,
        plain: PWSTR,
        capacity: u32,
        read: *mut u32,
        runs: *mut TS_RUNINFO,
        runs_capacity: u32,
        runs_read: *mut u32,
        next: *mut i32,
    ) -> Result<()> {
        self.read_access()?;
        if (capacity > 0 && plain.is_null())
            || (runs_capacity > 0 && runs.is_null())
            || read.is_null()
            || runs_read.is_null()
            || next.is_null()
        {
            return Err(E_POINTER.into());
        }
        let length = self.text.borrow().len() as i32;
        let range = self.range(start, if end == -1 { length } else { end })?;
        let text = self.text.borrow();
        let available = &text[range];
        let count = if capacity == 0 {
            available.len()
        } else {
            available.len().min(capacity as usize)
        };
        unsafe {
            if capacity > 0 && count > 0 {
                ptr::copy_nonoverlapping(available.as_ptr(), plain.0, count);
            }
            read.write(if capacity > 0 { count as u32 } else { 0 });
            runs_read.write(u32::from(runs_capacity > 0 && count > 0));
            if runs_capacity > 0 && count > 0 {
                runs.write(TS_RUNINFO {
                    uCount: count as u32,
                    r#type: TS_RT_PLAIN,
                });
            }
            next.write(start + count as i32);
        }
        Ok(())
    }

    fn SetText(
        &self,
        _: u32,
        start: i32,
        end: i32,
        text: &PCWSTR,
        length: u32,
    ) -> Result<TS_TEXTCHANGE> {
        self.edit(start, end, *text, length)
    }
    fn GetFormattedText(&self, _: i32, _: i32) -> Result<IDataObject> {
        Err(E_NOTIMPL.into())
    }
    fn GetEmbedded(&self, _: i32, _: *const GUID, _: *const GUID) -> Result<IUnknown> {
        Err(E_NOTIMPL.into())
    }
    fn QueryInsertEmbedded(&self, _: *const GUID, _: *const FORMATETC) -> Result<BOOL> {
        Ok(false.into())
    }
    fn InsertEmbedded(
        &self,
        _: u32,
        _: i32,
        _: i32,
        _: Option<&IDataObject>,
    ) -> Result<TS_TEXTCHANGE> {
        Err(E_NOTIMPL.into())
    }

    fn InsertTextAtSelection(
        &self,
        flags: u32,
        text: &PCWSTR,
        length: u32,
        start: *mut i32,
        end: *mut i32,
        change: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        let selection = self.selection.get();
        let end_after = selection
            .acpStart
            .checked_add(i32::try_from(length).map_err(|_| E_INVALIDARG)?)
            .ok_or(E_INVALIDARG)?;
        unsafe {
            if !start.is_null() {
                start.write(selection.acpStart);
            }
            if !end.is_null() {
                end.write(end_after);
            }
        }
        if flags & TS_IAS_QUERYONLY != 0 {
            return self.read_access();
        }
        let edited = self.edit(selection.acpStart, selection.acpEnd, *text, length)?;
        if !change.is_null() {
            unsafe { change.write(edited) };
        }
        Ok(())
    }

    fn InsertEmbeddedAtSelection(
        &self,
        _: u32,
        _: Option<&IDataObject>,
        _: *mut i32,
        _: *mut i32,
        _: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn RequestSupportedAttrs(&self, _: u32, count: u32, attrs: *const GUID) -> Result<()> {
        self.request_scope(count, attrs);
        Ok(())
    }
    fn RequestAttrsAtPosition(&self, _: i32, count: u32, attrs: *const GUID, _: u32) -> Result<()> {
        self.request_scope(count, attrs);
        Ok(())
    }
    fn RequestAttrsTransitioningAtPosition(
        &self,
        _: i32,
        count: u32,
        _: *const GUID,
        _: u32,
    ) -> Result<()> {
        self.request_scope(count, ptr::null());
        Ok(())
    }
    fn FindNextAttrTransition(
        &self,
        _: i32,
        end: i32,
        _: u32,
        _: *const GUID,
        _: u32,
        next: *mut i32,
        found: *mut BOOL,
        offset: *mut i32,
    ) -> Result<()> {
        if next.is_null() || found.is_null() || offset.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            next.write(end);
            found.write(false.into());
            offset.write(0);
        }
        Ok(())
    }

    fn RetrieveRequestedAttrs(
        &self,
        count: u32,
        attrs: *mut TS_ATTRVAL,
        fetched: *mut u32,
    ) -> Result<()> {
        if fetched.is_null() || (count > 0 && attrs.is_null()) {
            return Err(E_POINTER.into());
        }
        let present = count != 0 && self.scope_requested.replace(false);
        if present {
            let scope: ITfInputScope = AppScope(self.scope.get()).into();
            let value = VARIANT::from(scope.cast::<IUnknown>()?);
            unsafe {
                attrs.write(TS_ATTRVAL {
                    idAttr: GUID_PROP_INPUTSCOPE,
                    dwOverlapId: 0,
                    varValue: ManuallyDrop::new(value),
                })
            };
        }
        unsafe { fetched.write(u32::from(present)) };
        Ok(())
    }

    fn GetEndACP(&self) -> Result<i32> {
        self.read_access()?;
        Ok(self.text.borrow().len() as i32)
    }
    fn GetActiveView(&self) -> Result<u32> {
        Ok(1)
    }
    fn GetACPFromPoint(&self, _: u32, _: *const POINT, _: u32) -> Result<i32> {
        Err(E_NOTIMPL.into())
    }
    fn GetTextExt(&self, _: u32, _: i32, _: i32, _: *mut RECT, _: *mut BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetScreenExt(&self, _: u32) -> Result<RECT> {
        Err(E_NOTIMPL.into())
    }
    fn GetWnd(&self, _: u32) -> Result<HWND> {
        Ok(self.window)
    }
}
