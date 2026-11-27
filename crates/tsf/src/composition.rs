use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    rc::{Rc, Weak},
};

use diy_ime2_ime_host::DocumentStamp;
use windows::Win32::{
    Foundation::{BOOL, E_FAIL, HWND, RECT},
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    UI::TextServices::{
        CLSID_TF_CategoryMgr, GUID_PROP_ATTRIBUTE, GUID_PROP_INPUTSCOPE, ITfCategoryMgr,
        ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
        ITfContextComposition, ITfDocumentMgr, ITfEditRecord, ITfEditSession, ITfEditSession_Impl,
        ITfRange, ITfSource, ITfTextEditSink, ITfTextEditSink_Impl, TF_AE_END, TF_ANCHOR_END,
        TF_ANCHOR_START, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC,
        TF_GTP_INCL_TEXT, TF_SELECTION, TF_SELECTIONSTYLE,
    },
};
use windows_core::{Interface, Result, VARIANT, implement};

use crate::{
    ModuleRef,
    candidates::CandidateUi,
    diagnostics::trace,
    display::PREEDIT_ATTRIBUTE,
    document::{editable_selection, is_utf16_boundary},
    host::{Bridge, Preview},
    keymap::{self, Action, Context, Key},
    service::Activation,
};

const MAX_PREEDIT: usize = 4096;

#[derive(Default)]
pub(crate) struct Preedit {
    current: RefCell<Option<Rc<Owned>>>,
    pending_cleanup: RefCell<Option<Rc<Owned>>>,
    generation: Cell<u64>,
    revision: Cell<u64>,
    busy: Cell<bool>,
    uncertain: Cell<bool>,
    display_atom: Cell<Option<u32>>,
    pending_finalize: Cell<Option<DocumentStamp>>,
}

struct Owned {
    context: ITfContext,
    composition: ITfComposition,
    original: Vec<u16>,
    text: RefCell<String>,
    displayed: RefCell<String>,
    validity: Rc<Validity>,
    _watch: EditWatch,
    candidates: Rc<CandidateSlot>,
}

#[derive(Default)]
struct Validity {
    ended: Cell<bool>,
    modified: Cell<bool>,
    own_edit: Cell<bool>,
}

impl Validity {
    fn owned(&self) -> bool {
        !self.ended.get() && !self.modified.get()
    }
}

#[implement(ITfCompositionSink, ITfTextEditSink)]
struct Observer {
    _module: ModuleRef,
    validity: Rc<Validity>,
    candidates: Weak<CandidateSlot>,
}

impl Observer_Impl {
    fn close_candidates(&self) {
        if let Some(slot) = self.candidates.upgrade() {
            close_candidates(&slot);
        }
    }
}

impl ITfCompositionSink_Impl for Observer_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Option<&ITfComposition>) -> Result<()> {
        trace!("DIY IME Stage B: TSF terminated composition");
        self.validity.ended.set(true);

        self.close_candidates();
        Ok(())
    }
}

impl ITfTextEditSink_Impl for Observer_Impl {
    fn OnEndEdit(
        &self,
        _: Option<&ITfContext>,
        _: u32,
        record: Option<&ITfEditRecord>,
    ) -> Result<()> {
        if !self.validity.own_edit.get() && record.is_none_or(has_document_change) {
            trace!("DIY IME Stage B: external edit invalidated composition");
            self.validity.modified.set(true);
            self.close_candidates();
        }
        Ok(())
    }
}

fn has_document_change(record: &ITfEditRecord) -> bool {
    let change = (|| unsafe {
        if record.GetSelectionStatus()?.as_bool() {
            return Ok::<_, windows_core::Error>(true);
        }
        let updates =
            record.GetTextAndPropertyUpdates(TF_GTP_INCL_TEXT, &[&GUID_PROP_INPUTSCOPE])?;
        let mut ranges = [None];
        let mut count = 0;
        updates.Next(&mut ranges, &mut count)?;
        Ok(count != 0)
    })();
    change.unwrap_or(true)
}

type CandidateSlot = RefCell<Option<Rc<CandidateUi>>>;

fn close_candidates(slot: &CandidateSlot) {
    if let Some(ui) = slot.take() {
        ui.close();
    }
}

#[derive(Clone, Copy)]
enum CandidateUpdate {
    Close,
    Select(usize),
}

#[derive(Clone, Copy)]
enum HostAction {
    Convert,
    Finalize,
    FinalizePending,
}

struct CandidateAnchor {
    document: ITfDocumentMgr,
    owner: HWND,
    rectangle: Option<RECT>,
}

struct EditWatch {
    source: ITfSource,
    cookie: u32,
}

impl Drop for EditWatch {
    fn drop(&mut self) {
        let _ = unsafe { self.source.UnadviseSink(self.cookie) };
    }
}

struct Busy<'a>(&'a Cell<bool>);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Preedit {
    fn display_atom(&self) -> Result<u32> {
        if let Some(atom) = self.display_atom.get() {
            return Ok(atom);
        }
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        let atom = unsafe { categories.RegisterGUID(&PREEDIT_ATTRIBUTE)? };
        self.display_atom.set(Some(atom));
        Ok(atom)
    }
    pub(crate) fn matches(&self, context: Option<&ITfContext>) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|owned| Some(&owned.context) == context && owned.validity.owned())
    }

    fn stamp(&self) -> DocumentStamp {
        let generation = self.generation.get();
        let revision = self.revision.get();
        DocumentStamp {
            focus: generation,
            context: generation,
            revision,
            selection: revision,
        }
    }

    pub(crate) fn invalidate(&self, keep_context: Option<&ITfContext>, client: u32) {
        let owned = self.current.borrow().clone();
        if let Some(owned) = owned.as_ref()
            && keep_context == Some(&owned.context)
        {
            return;
        }
        self.generation.set(self.generation.get().saturating_add(1));
        self.uncertain.set(false);
        self.current.replace(None);
        self.pending_finalize.set(None);
        if let Some(owned) = owned.as_ref() {
            trace!("DIY IME Stage B: service invalidated composition");
            close_candidates(&owned.candidates);
        }
        if let Some(owned) = owned
            && !owned.validity.ended.get()
        {
            if self.busy.replace(true) {
                self.pending_cleanup.replace(Some(owned));
            } else {
                let _busy = Busy(&self.busy);
                Self::cleanup(owned, client);
            }
        }
    }

    fn cleanup(owned: Rc<Owned>, client: u32) {
        if !owned.validity.ended.get() {
            let armed = Rc::new(Cell::new(true));
            let session: ITfEditSession = Cleanup {
                _module: ModuleRef::new(),
                owned: owned.clone(),
                armed: armed.clone(),
            }
            .into();
            // A deferred write is unsafe once ordinary processing has resumed.
            let _ = unsafe {
                owned.context.RequestEditSession(
                    client,
                    &session,
                    TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0),
                )
            };
            armed.set(false);
        }
    }

    pub(crate) fn key(
        &self,
        activation: &Rc<Activation>,
        context: &ITfContext,
        key: Key,
        apply: bool,
    ) -> bool {
        self.check_key(activation, context, key, apply)
            .unwrap_or(false)
    }

    fn check_key(
        &self,
        activation: &Rc<Activation>,
        context: &ITfContext,
        key: Key,
        apply: bool,
    ) -> Option<bool> {
        let generation = self.generation.get();
        let converting = matches!(key, Key::Space);
        if generation == u64::MAX || self.uncertain.get() || !activation.owns_focus(context) {
            if converting {
                trace!("DIY IME Stage B: Space rejected before edit session");
            }
            return Some(false);
        }
        if self.busy.replace(true) {
            return None;
        }
        let _busy = Busy(&self.busy);
        let completed = Rc::new(Cell::new(None));
        let host_action = Rc::new(Cell::new(None));
        let candidate_update = Rc::new(Cell::new(None));
        let own_edit = Rc::new(RefCell::new(None::<Rc<Validity>>));
        let reading_host_input = matches!(key, Key::Space | Key::Enter)
            && !self.showing_candidates()
            && activation
                .host
                .try_borrow()
                .is_ok_and(|host| host.as_ref().is_some_and(Bridge::available));
        let writable = apply && !reading_host_input;
        let session: ITfEditSession = Session {
            _module: ModuleRef::new(),
            activation: activation.clone(),
            context: context.clone(),
            generation,
            key,
            apply,
            writable,
            completed: completed.clone(),
            host_action: host_action.clone(),
            candidate_update: candidate_update.clone(),
            own_edit: own_edit.clone(),
        }
        .into();
        let access = if writable {
            TF_ES_READWRITE
        } else {
            TF_ES_READ
        };
        let _ = unsafe {
            context.RequestEditSession(
                activation.client_id,
                &session,
                TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | access.0),
            )
        };
        if let Some(validity) = own_edit.take() {
            validity.own_edit.set(false);
        }
        let handled = completed.replace(Some(false));
        if converting && handled.is_none() {
            trace!("DIY IME Stage B: Space edit session did not run");
        }
        if let Some(owned) = self.pending_cleanup.take() {
            Self::cleanup(owned, activation.client_id);
        }
        if apply && handled == Some(true) {
            if matches!(host_action.get(), Some(HostAction::FinalizePending)) {
                return handled;
            }
            if let Some(action) = host_action.get() {
                if self.generation.get() == generation
                    && let Some(owned) = self.current.borrow().as_ref()
                    && owned.validity.owned()
                    && let Ok(mut host) = activation.host.try_borrow_mut()
                    && let Some(host) = host.as_mut()
                {
                    let stamp = self.stamp();
                    let finalize = matches!(action, HostAction::Finalize);
                    self.pending_finalize.set(finalize.then_some(stamp));
                    if converting {
                        trace!("DIY IME Stage B: Space submitted conversion to Host bridge");
                    }
                    host.convert(stamp, owned.text.borrow().clone(), finalize);
                }
            } else {
                self.revision.set(self.revision.get().saturating_add(1));
                self.pending_finalize.set(None);
                activation.cancel_host();
                let owned = self.current.borrow().clone();
                if let Some(owned) = owned {
                    match candidate_update.get() {
                        Some(CandidateUpdate::Select(index)) => {
                            let candidates = owned.candidates.borrow().clone();
                            if let Some(candidates) = candidates {
                                candidates.select(index);
                            }
                        }
                        Some(CandidateUpdate::Close) => close_candidates(&owned.candidates),
                        None => {}
                    }
                }
            }
        }
        handled
    }

    pub(crate) fn finish_host_request(&self) {
        self.pending_finalize.set(None);
    }

    pub(crate) fn showing_candidates(&self) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|owned| owned.candidates.borrow().is_some())
    }

    pub(crate) fn can_deliver(&self, activation: &Rc<Activation>) -> Option<bool> {
        let owned = self.current.borrow().clone();
        let Some(owned) = owned.filter(|owned| owned.validity.owned()) else {
            return Some(false);
        };
        self.check_key(activation, &owned.context, Key::Space, false)
    }

    pub(crate) fn preview(&self, activation: &Rc<Activation>, preview: Preview) {
        if preview.stamp != self.stamp()
            || self.revision.get() == u64::MAX
            || self.uncertain.get()
            || preview.text.encode_utf16().count() > MAX_PREEDIT
            || self.busy.replace(true)
        {
            return;
        }
        let _busy = Busy(&self.busy);
        let owned = self.current.borrow().clone();
        let Some(owned) = owned else { return };
        let armed = Rc::new(Cell::new(true));
        let anchor = Rc::new(RefCell::new(None));
        let selected = preview
            .candidates
            .iter()
            .position(|candidate| candidate == &preview.text);
        let session: ITfEditSession = PreviewSession {
            _module: ModuleRef::new(),
            activation: activation.clone(),
            owned: owned.clone(),
            stamp: preview.stamp,
            text: preview.text,
            finalize: preview.finalize,
            armed: armed.clone(),
            anchor: anchor.clone(),
        }
        .into();
        let _ = unsafe {
            owned.context.RequestEditSession(
                activation.client_id,
                &session,
                TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0),
            )
        };
        armed.set(false);
        owned.validity.own_edit.set(false);
        if let Some(owned) = self.pending_cleanup.take() {
            Self::cleanup(owned, activation.client_id);
        }
        if let (Some(anchor), Some(selected)) = (anchor.take(), selected)
            && preview.stamp == self.stamp()
            && owned.validity.owned()
            && activation.owns_focus(&owned.context)
        {
            close_candidates(&owned.candidates);
            if let Ok(manager) = activation.ui_manager()
                && let Ok(ui) = CandidateUi::open(
                    manager,
                    anchor.document,
                    preview.candidates,
                    selected,
                    anchor.owner,
                    anchor.rectangle,
                )
                && preview.stamp == self.stamp()
                && owned.validity.owned()
                && activation.owns_focus(&owned.context)
            {
                owned.candidates.replace(Some(Rc::new(ui)));
            }
        }
    }
}

#[implement(ITfEditSession)]
struct Session {
    _module: ModuleRef,
    activation: Rc<Activation>,
    context: ITfContext,
    generation: u64,
    key: Key,
    apply: bool,
    writable: bool,
    completed: Rc<Cell<Option<bool>>>,
    host_action: Rc<Cell<Option<HostAction>>>,
    candidate_update: Rc<Cell<Option<CandidateUpdate>>>,
    own_edit: Rc<RefCell<Option<Rc<Validity>>>>,
}

impl Session_Impl {
    fn current(&self) -> bool {
        let preedit = &self.activation.preedit;
        preedit.generation.get() == self.generation
            && !preedit.uncertain.get()
            && !self.activation.manually_blocked()
            && self.activation.owns_focus(&self.context)
            && preedit.generation.get() == self.generation
    }

    fn inspect(&self, cookie: u32) -> Option<(ITfRange, Option<Rc<Owned>>, String)> {
        if !self.current() {
            if matches!(self.key, Key::Space) {
                trace!("DIY IME Stage B: Space lost current document");
            }
            return None;
        }
        let Some(selection) = editable_selection(&self.context, cookie) else {
            trace!("DIY IME Stage B: input field rejected by protection checks");
            if self.apply {
                self.activation.protect();
            }
            return None;
        };
        if self.apply && !self.activation.allow_ordinary_input() {
            return None;
        }
        let owned = self.activation.preedit.current.borrow().clone();
        let raw = match owned.as_ref() {
            Some(owned) if owned.context == self.context && owned.validity.owned() => {
                let Some(range) = owned.verified_range(cookie) else {
                    if matches!(self.key, Key::Space) {
                        trace!("DIY IME Stage B: Space could not verify composition range");
                    }
                    return None;
                };
                if !unsafe { selection.IsEmpty(cookie).ok()?.as_bool() }
                    || unsafe { selection.CompareStart(cookie, &range, TF_ANCHOR_END).ok()? } != 0
                {
                    if matches!(self.key, Key::Space) {
                        trace!("DIY IME Stage B: Space found selection outside composition end");
                    }
                    return None;
                }
                owned.text.borrow().clone()
            }
            Some(owned) if !owned.validity.ended.get() => {
                if matches!(self.key, Key::Space) {
                    trace!("DIY IME Stage B: Space found invalidated composition");
                }
                if self.apply {
                    self.activation.cancel_host();
                    self.activation
                        .preedit
                        .invalidate(None, self.activation.client_id);
                }
                return None;
            }
            _ if valid_boundaries(&selection, cookie) => String::new(),
            _ => return None,
        };
        self.current().then_some((selection, owned, raw))
    }
}

impl ITfEditSession_Impl for Session_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        if self.completed.replace(Some(false)).is_some() {
            return Ok(());
        }
        let preedit = &self.activation.preedit;
        let Some((selection, owned, mut raw)) = self.inspect(cookie) else {
            return Ok(());
        };
        let owned = owned.filter(|owned| !owned.validity.ended.get());
        let candidates = owned
            .as_ref()
            .and_then(|owned| owned.candidates.borrow().clone());
        let mut candidate_update = CandidateUpdate::Close;
        let mut selected_surface = None;
        let mut committing = false;
        let host_ready = self
            .activation
            .host
            .try_borrow()
            .is_ok_and(|host| host.as_ref().is_some_and(Bridge::available));
        let action = keymap::action(
            self.key,
            Context {
                editable: true,
                preedit: &raw,
                selecting_candidate: candidates.is_some(),
            },
        );
        if matches!(self.key, Key::Space) {
            if raw.is_empty() {
                trace!("DIY IME Stage B: Space found no owned preedit");
            } else if !host_ready && matches!(action, Some(Action::Convert)) {
                trace!("DIY IME Stage B: Space found preedit but Host unavailable");
            }
        }
        match action {
            Some(Action::Append(character)) if raw.len() < MAX_PREEDIT => raw.push(character),
            Some(Action::ReplaceWithPrefix(prefix)) => raw.truncate(prefix),
            Some(Action::Cancel) => raw.clear(),
            Some(Action::ReturnToPreedit) => {}
            Some(Action::NextCandidate | Action::PreviousCandidate) => {
                let candidates = candidates.as_ref().ok_or(E_FAIL)?;
                let (index, text) = candidates.next(action == Some(Action::NextCandidate));
                candidate_update = CandidateUpdate::Select(index);
                selected_surface = Some(text);
            }
            Some(Action::Convert) if host_ready => {
                self.completed.set(Some(true));
                if self.apply {
                    self.host_action.set(Some(HostAction::Convert));
                }
                return Ok(());
            }
            Some(Action::Commit) if preedit.pending_finalize.get() == Some(preedit.stamp()) => {
                self.completed.set(Some(true));
                if self.apply {
                    self.host_action.set(Some(HostAction::FinalizePending));
                }
                return Ok(());
            }
            Some(Action::Commit) if candidates.is_none() && host_ready => {
                self.completed.set(Some(true));
                if self.apply {
                    self.host_action.set(Some(HostAction::Finalize));
                }
                return Ok(());
            }
            Some(Action::Commit) => committing = true,
            _ => return Ok(()),
        }
        if !self.apply {
            self.completed.set(Some(true));
            return Ok(());
        }
        if !self.writable {
            return Ok(());
        }
        let property = unsafe { self.context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
        let atom = preedit.display_atom()?;
        if !self.current()
            || !selection_is_current(&self.context, cookie, &selection)
            || !self.current()
        {
            return Ok(());
        }
        let starting = owned.is_none();
        let owned = match owned {
            Some(owned) => owned,
            None => {
                let original = read_range(&selection, cookie, MAX_PREEDIT).ok_or(E_FAIL)?;
                if String::from_utf16(&original).is_err() {
                    return Ok(());
                }
                if !self.current() {
                    return Ok(());
                }
                let owned = self.start(cookie, &selection, original)?;
                trace!("DIY IME Stage B: TSF started composition");
                if !self.current() || !owned.validity.owned() {
                    let _ = unsafe { owned.composition.EndComposition(cookie) };
                    return Ok(());
                }
                preedit.current.replace(Some(owned.clone()));
                owned
            }
        };
        let range = if starting {
            unsafe { owned.composition.GetRange().ok() }
        } else {
            owned.verified_range(cookie)
        };
        let Some(range) = range.filter(|_| {
            self.current()
                && selection_is_current(&self.context, cookie, &selection)
                && self.current()
                && owned.validity.owned()
        }) else {
            if starting {
                let _ = unsafe { owned.composition.EndComposition(cookie) };
                preedit.current.replace(None);
            }
            return Ok(());
        };
        owned.validity.own_edit.set(true);
        self.own_edit.replace(Some(owned.validity.clone()));
        // A failed COM call after this point does not prove that the app made no edit.
        self.completed.set(Some(true));
        let ending = raw.is_empty();
        let displayed = selected_surface.unwrap_or_else(|| raw.clone());
        let text: Vec<u16> = displayed.encode_utf16().collect();
        let replacement = if ending { &owned.original } else { &text };
        let result = (|| unsafe {
            if committing {
                property.Clear(cookie, &range)?;
                if !self.current() || !owned.validity.owned() {
                    return Err(E_FAIL.into());
                }
                owned.composition.EndComposition(cookie)?;
                owned.validity.ended.set(true);
                return Ok(());
            }
            range.SetText(cookie, 0, replacement)?;
            if !self.current()
                || !owned.validity.owned()
                || editable_selection(&self.context, cookie).is_none()
                || !self.current()
                || !owned.validity.owned()
            {
                return Err(E_FAIL.into());
            }
            let actual = owned.composition.GetRange()?;
            if read_range(&actual, cookie, replacement.len()).as_deref()
                != Some(replacement.as_slice())
            {
                return Err(E_FAIL.into());
            }
            if !self.current() || !owned.validity.owned() {
                return Err(E_FAIL.into());
            }
            if ending {
                property.Clear(cookie, &actual)?;
                owned.composition.EndComposition(cookie)?;
                owned.validity.ended.set(true);
                set_selection(&self.context, cookie, actual, owned.original.is_empty())
            } else {
                property.SetValue(cookie, &actual, &VARIANT::from(atom as i32))?;
                if !self.current() || !owned.validity.owned() {
                    return Err(E_FAIL.into());
                }
                set_selection(&self.context, cookie, actual, true)
            }
        })();
        if result.is_err() || !self.current() {
            if preedit.generation.get() == self.generation {
                preedit.uncertain.set(true);
            }
            // End our composition if TSF still has it. Do not guess which text was changed.
            if !owned.validity.ended.get() {
                let _ = unsafe { owned.composition.EndComposition(cookie) };
            }
            preedit.current.replace(None);
        } else if ending || committing {
            preedit.current.replace(None);
        } else {
            owned.displayed.replace(displayed);
            self.candidate_update.set(Some(candidate_update));
            owned.text.replace(raw);
        }
        result
    }
}

impl Session_Impl {
    fn start(&self, cookie: u32, selection: &ITfRange, original: Vec<u16>) -> Result<Rc<Owned>> {
        let validity = Rc::new(Validity::default());
        let candidates = Rc::new(RefCell::new(None));
        let sink: ITfCompositionSink = Observer {
            _module: ModuleRef::new(),
            validity: validity.clone(),
            candidates: Rc::downgrade(&candidates),
        }
        .into();
        let source: ITfSource = self.context.cast()?;
        let identity: windows_core::IUnknown = sink.cast()?;
        let cookie_sink = unsafe { source.AdviseSink(&ITfTextEditSink::IID, &identity)? };
        let watch = EditWatch {
            source,
            cookie: cookie_sink,
        };
        let context: ITfContextComposition = self.context.cast()?;
        if !self.current() {
            return Err(E_FAIL.into());
        }
        let composition = unsafe { context.StartComposition(cookie, selection, &sink)? };

        Ok(Rc::new(Owned {
            context: self.context.clone(),
            composition,
            original,
            text: RefCell::new(String::new()),
            displayed: RefCell::new(String::new()),
            validity,
            _watch: watch,
            candidates,
        }))
    }
}

impl Owned {
    fn verified_range(&self, cookie: u32) -> Option<ITfRange> {
        let range = unsafe { self.composition.GetRange().ok()? };
        if !valid_boundaries(&range, cookie) {
            return None;
        }
        let expected_units = self.displayed.borrow().encode_utf16().count();
        let text = read_range(&range, cookie, expected_units)?;
        (text
            .iter()
            .copied()
            .eq(self.displayed.borrow().encode_utf16())
            && self.validity.owned())
        .then_some(range)
    }
}

fn selection_is_current(context: &ITfContext, cookie: u32, expected: &ITfRange) -> bool {
    editable_selection(context, cookie).is_some_and(|current| unsafe {
        current.CompareStart(cookie, expected, TF_ANCHOR_START) == Ok(0)
            && current.CompareEnd(cookie, expected, TF_ANCHOR_END) == Ok(0)
    })
}

fn valid_boundaries(range: &ITfRange, cookie: u32) -> bool {
    [TF_ANCHOR_START, TF_ANCHOR_END].into_iter().all(|anchor| {
        let Ok(caret) = (unsafe { range.Clone() }) else {
            return false;
        };
        (unsafe { caret.Collapse(cookie, anchor).is_ok() }) && is_utf16_boundary(&caret, cookie)
    })
}

fn read_range(range: &ITfRange, cookie: u32, limit: usize) -> Option<Vec<u16>> {
    // One extra code unit to reject a range longer than the bound.
    let mut units = vec![0; limit + 1];
    let mut count = 0;
    unsafe { range.GetText(cookie, 0, &mut units, &mut count).ok()? };
    if count as usize > limit {
        return None;
    }
    units.truncate(count as usize);
    Some(units)
}

fn set_selection(context: &ITfContext, cookie: u32, range: ITfRange, collapse: bool) -> Result<()> {
    if collapse {
        unsafe { range.Collapse(cookie, TF_ANCHOR_END)? };
    }
    let selection = TF_SELECTION {
        range: ManuallyDrop::new(Some(range)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_END,
            fInterimChar: false.into(),
        },
    };
    let outcome = unsafe { context.SetSelection(cookie, std::slice::from_ref(&selection)) };
    drop(ManuallyDrop::into_inner(selection.range));
    outcome
}

#[implement(ITfEditSession)]
struct Cleanup {
    _module: ModuleRef,
    owned: Rc<Owned>,
    armed: Rc<Cell<bool>>,
}

impl ITfEditSession_Impl for Cleanup_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        if !self.armed.replace(false) || self.owned.validity.ended.get() {
            return Ok(());
        }
        let owned = &self.owned;
        // At focus loss the old context may become protected or disconnected.
        // Only edit the exact original composition when the old context still permits it.
        let mut outcome = Ok(());
        if editable_selection(&owned.context, cookie).is_some()
            && owned.validity.owned()
            && let Some(range) = owned.verified_range(cookie)
        {
            outcome = (|| unsafe {
                owned
                    .context
                    .GetProperty(&GUID_PROP_ATTRIBUTE)?
                    .Clear(cookie, &range)?;
                range.SetText(cookie, 0, &owned.original)
            })();
        }
        let end = unsafe { owned.composition.EndComposition(cookie) };
        owned.validity.ended.set(true);
        outcome.and(end)
    }
}

#[implement(ITfEditSession)]
struct PreviewSession {
    _module: ModuleRef,
    activation: Rc<Activation>,
    owned: Rc<Owned>,
    stamp: DocumentStamp,
    text: String,
    finalize: bool,
    armed: Rc<Cell<bool>>,
    anchor: Rc<RefCell<Option<CandidateAnchor>>>,
}

impl PreviewSession_Impl {
    fn valid(&self) -> bool {
        let preedit = &self.activation.preedit;
        self.stamp == preedit.stamp()
            && !self.activation.is_protected()
            && self.owned.validity.owned()
            && self.activation.owns_focus(&self.owned.context)
            && self.stamp == preedit.stamp()
            && self.owned.validity.owned()
    }
}

impl ITfEditSession_Impl for PreviewSession_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        if !self.armed.replace(false) || !self.valid() {
            return Ok(());
        }
        let preedit = &self.activation.preedit;
        let context = &self.owned.context;
        let Some(selection) = editable_selection(context, cookie) else {
            self.activation.protect();
            return Ok(());
        };
        let Some(range) = self.owned.verified_range(cookie) else {
            return Ok(());
        };
        if !unsafe { selection.IsEmpty(cookie)?.as_bool() }
            || unsafe { selection.CompareStart(cookie, &range, TF_ANCHOR_END)? } != 0
        {
            return Ok(());
        }
        let atom = preedit.display_atom()?;
        let property = unsafe { context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
        if !self.valid() || !selection_is_current(context, cookie, &selection) || !self.valid() {
            return Ok(());
        }

        let text: Vec<u16> = self.text.encode_utf16().collect();
        self.owned.validity.own_edit.set(true);
        let result = (|| unsafe {
            range.SetText(cookie, 0, &text)?;
            if !self.valid() || editable_selection(context, cookie).is_none() || !self.valid() {
                return Err(E_FAIL.into());
            }
            let actual = self.owned.composition.GetRange()?;
            if read_range(&actual, cookie, text.len()).as_deref() != Some(text.as_slice())
                || !self.valid()
            {
                return Err(E_FAIL.into());
            }
            if self.finalize {
                property.Clear(cookie, &actual)?;
                if !self.valid() {
                    return Err(E_FAIL.into());
                }
                set_selection(context, cookie, actual, true)?;
                if !self.valid() || editable_selection(context, cookie).is_none() || !self.valid() {
                    return Err(E_FAIL.into());
                }
                self.owned.composition.EndComposition(cookie)?;
                self.owned.validity.ended.set(true);
                return Ok(());
            }
            property.SetValue(cookie, &actual, &VARIANT::from(atom as i32))?;
            if !self.valid() {
                return Err(E_FAIL.into());
            }
            set_selection(context, cookie, actual, true)
        })();
        if result.is_ok() && self.finalize {
            if self.stamp == preedit.stamp() {
                preedit.current.replace(None);
                preedit.pending_finalize.set(None);
            }
        } else if result.is_ok() && self.valid() {
            self.owned.displayed.replace(self.text.clone());
            if let Ok(anchor) = candidate_anchor(context, cookie, &self.owned.composition)
                && self.valid()
            {
                self.anchor.replace(Some(anchor));
            }
        } else {
            self.activation.cancel_host();
            preedit.invalidate(None, self.activation.client_id);
            preedit.uncertain.set(true);
        }
        result
    }
}

fn candidate_anchor(
    context: &ITfContext,
    cookie: u32,
    composition: &ITfComposition,
) -> Result<CandidateAnchor> {
    let document = unsafe { context.GetDocumentMgr()? };
    let view = unsafe { context.GetActiveView()? };
    let owner = unsafe { view.GetWnd() }.unwrap_or_default();
    let range = unsafe { composition.GetRange()? };
    let mut rectangle = RECT::default();
    let mut clipped = BOOL::default();
    let visible = unsafe {
        view.GetTextExt(cookie, &range, &mut rectangle, &mut clipped)
            .is_ok()
    } && !clipped.as_bool();
    Ok(CandidateAnchor {
        document,
        owner,
        rectangle: visible.then_some(rectangle),
    })
}
