use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use windows::Win32::{
    Foundation::{BOOL, E_FAIL, E_INVALIDARG, E_POINTER, HWND, RECT},
    UI::{
        TextServices::{
            ITfCandidateListUIElement, ITfCandidateListUIElement_Impl, ITfDocumentMgr,
            ITfUIElement, ITfUIElement_Impl, ITfUIElementMgr, TF_CLUIE_COUNT, TF_CLUIE_CURRENTPAGE,
            TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION, TF_CLUIE_STRING,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetWindowRect, HWND_TOP, SW_HIDE, SWP_NOACTIVATE,
            SWP_SHOWWINDOW, SetWindowPos, SetWindowTextW, ShowWindow, WS_BORDER, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};
use windows_core::{BSTR, GUID, Interface, PCWSTR, Result, implement};

use crate::ModuleRef;

const ROWS_PER_PAGE: usize = 8;
const CANDIDATE_ELEMENT: GUID = GUID::from_u128(0x292533e7_5ac4_414c_a8dd_0fd8f091a212);
const ALL_FLAGS: u32 = TF_CLUIE_COUNT
    | TF_CLUIE_CURRENTPAGE
    | TF_CLUIE_DOCUMENTMGR
    | TF_CLUIE_PAGEINDEX
    | TF_CLUIE_SELECTION
    | TF_CLUIE_STRING;

pub(crate) struct CandidateUi {
    manager: ITfUIElementMgr,
    id: u32,
    model: Rc<Model>,
}

struct Model {
    document: ITfDocumentMgr,
    items: RefCell<Vec<String>>,
    page_starts: RefCell<Vec<usize>>,
    selected: Cell<usize>,
    live: Cell<bool>,
    shown: Cell<bool>,
    popup: RefCell<Option<Popup>>,
}

impl CandidateUi {
    pub(crate) fn open(
        manager: ITfUIElementMgr,
        document: ITfDocumentMgr,
        items: Vec<String>,
        selected: usize,
        owner: HWND,
        rectangle: Option<RECT>,
    ) -> Result<Self> {
        if selected >= items.len() {
            return Err(E_INVALIDARG.into());
        }
        let page_starts = (0..items.len()).step_by(ROWS_PER_PAGE).collect();
        let model = Rc::new(Model {
            document,
            items: RefCell::new(items),
            page_starts: RefCell::new(page_starts),
            selected: Cell::new(selected),
            live: Cell::new(true),
            shown: Cell::new(true),
            popup: RefCell::new(None),
        });
        let element: ITfCandidateListUIElement = CandidateElement {
            _module: ModuleRef::new(),
            model: model.clone(),
        }
        .into();
        let element: ITfUIElement = element.cast()?;
        let mut show = true.into();
        let mut id = 0;
        unsafe { manager.BeginUIElement(&element, &mut show, &mut id)? };
        let ui = Self { manager, id, model };
        if show.as_bool() {
            let popup = Popup::open(owner, rectangle)?;
            ui.model.popup.replace(Some(popup));
            ui.model.paint()?;
        }
        unsafe { ui.manager.UpdateUIElement(ui.id)? };
        Ok(ui)
    }

    pub(crate) fn next(&self, forward: bool) -> (usize, String) {
        let items = self.model.items.borrow();
        let selected = self.model.selected.get();
        let index = if forward {
            (selected + 1) % items.len()
        } else {
            (selected + items.len() - 1) % items.len()
        };
        (index, items[index].clone())
    }

    pub(crate) fn select(&self, index: usize) {
        self.model.selected.set(index);
        let _ = self.model.paint();
        let _ = unsafe { self.manager.UpdateUIElement(self.id) };
    }
}

impl CandidateUi {
    pub(crate) fn close(&self) {
        if !self.model.live.replace(false) {
            return;
        }
        self.model.shown.set(false);
        self.model.items.borrow_mut().clear();
        self.model.popup.take();
        let _ = unsafe { self.manager.EndUIElement(self.id) };
    }
}

impl Drop for CandidateUi {
    fn drop(&mut self) {
        self.close();
    }
}

impl Model {
    fn page(&self) -> usize {
        let selected = self.selected.get();
        self.page_starts
            .borrow()
            .partition_point(|&start| start <= selected)
            .saturating_sub(1)
    }

    fn paint(&self) -> Result<()> {
        let popup = self.popup.try_borrow().map_err(|_| E_FAIL)?;
        let Some(popup) = popup.as_ref() else {
            return Ok(());
        };
        if !self.live.get() || !self.shown.get() {
            let _ = unsafe { ShowWindow(popup.window, SW_HIDE) };
            return Ok(());
        }
        let (rows, text) = {
            let selected = self.selected.get();
            let starts = self.page_starts.borrow();
            let items = self.items.borrow();
            let page = self.page();
            let start = starts[page];
            let end = starts.get(page + 1).copied().unwrap_or(items.len());
            let text = items[start..end]
                .iter()
                .enumerate()
                .map(|(offset, item)| {
                    let indicator = if start + offset == selected {
                        "▶ "
                    } else {
                        "    "
                    };
                    let single_line: String = item
                        .chars()
                        .map(|character| {
                            if character.is_control() {
                                ' '
                            } else {
                                character
                            }
                        })
                        .collect();
                    format!("{indicator}{single_line}")
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            (end - start, text)
        };
        popup.paint(rows, &text)
    }
}

#[implement(ITfCandidateListUIElement)]
struct CandidateElement {
    _module: ModuleRef,
    model: Rc<Model>,
}

impl ITfUIElement_Impl for CandidateElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("DIY IME candidates"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        Ok(CANDIDATE_ELEMENT)
    }

    fn Show(&self, shown: BOOL) -> Result<()> {
        self.model
            .shown
            .set(self.model.live.get() && shown.as_bool());
        self.model.paint()
    }

    fn IsShown(&self) -> Result<BOOL> {
        Ok(self.model.shown.get().into())
    }
}

impl ITfCandidateListUIElement_Impl for CandidateElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        Ok(ALL_FLAGS)
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.model
            .live
            .get()
            .then(|| self.model.document.clone())
            .ok_or(E_FAIL.into())
    }

    fn GetCount(&self) -> Result<u32> {
        Ok(self.model.items.borrow().len() as u32)
    }

    fn GetSelection(&self) -> Result<u32> {
        self.model
            .live
            .get()
            .then(|| self.model.selected.get() as u32)
            .ok_or(E_FAIL.into())
    }

    fn GetString(&self, index: u32) -> Result<BSTR> {
        self.model
            .items
            .borrow()
            .get(index as usize)
            .map(|item| BSTR::from(item.as_str()))
            .ok_or(E_INVALIDARG.into())
    }

    fn GetPageIndex(&self, indexes: *mut u32, size: u32, count: *mut u32) -> Result<()> {
        if count.is_null() || (size != 0 && indexes.is_null()) {
            return Err(E_POINTER.into());
        }
        if !self.model.live.get() {
            unsafe { count.write(0) };
            return Ok(());
        }
        let starts = self.model.page_starts.borrow();
        unsafe { count.write(starts.len() as u32) };
        for (offset, &start) in starts.iter().take(size as usize).enumerate() {
            unsafe { indexes.add(offset).write(start as u32) };
        }
        Ok(())
    }

    fn SetPageIndex(&self, indexes: *const u32, count: u32) -> Result<()> {
        let length = self.model.items.borrow().len();
        if count == 0 || count as usize > length {
            return Err(E_INVALIDARG.into());
        }
        if indexes.is_null() {
            return Err(E_POINTER.into());
        }
        let indexes = unsafe { std::slice::from_raw_parts(indexes, count as usize) };
        if indexes[0] != 0
            || indexes.last().is_some_and(|&last| last as usize >= length)
            || indexes.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(E_INVALIDARG.into());
        }
        self.model
            .page_starts
            .replace(indexes.iter().map(|&index| index as usize).collect());
        self.model.paint()
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(self.model.page() as u32)
    }
}

struct Popup {
    window: HWND,
    rectangle: RECT,
}

impl Popup {
    fn open(owner: HWND, rectangle: Option<RECT>) -> Result<Self> {
        if owner.is_invalid() {
            return Err(E_FAIL.into());
        }
        let rectangle = match rectangle {
            Some(rectangle) => rectangle,
            None => {
                let mut rectangle = RECT::default();
                unsafe { GetWindowRect(owner, &mut rectangle)? };
                rectangle
            }
        };
        let window = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                windows_core::w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP | WS_BORDER,
                0,
                0,
                0,
                0,
                owner,
                None,
                None,
                None,
            )?
        };
        Ok(Self { window, rectangle })
    }

    fn paint(&self, rows: usize, text: &str) -> Result<()> {
        let wide: Vec<_> = text.encode_utf16().chain([0]).collect();
        unsafe {
            SetWindowTextW(self.window, PCWSTR(wide.as_ptr()))?;
            SetWindowPos(
                self.window,
                HWND_TOP,
                self.rectangle.left,
                self.rectangle.bottom,
                280,
                (rows as i32 * 22 + 8).min(200),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        }
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.window) };
    }
}
