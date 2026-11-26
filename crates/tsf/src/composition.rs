use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    rc::Rc,
};

use windows::Win32::{
    Foundation::E_FAIL,
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    UI::TextServices::{
        CLSID_TF_CategoryMgr, GUID_PROP_ATTRIBUTE, ITfCategoryMgr, ITfComposition,
        ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
        ITfEditRecord, ITfEditSession, ITfEditSession_Impl, ITfRange, ITfSource, ITfTextEditSink,
        ITfTextEditSink_Impl, TF_AE_END, TF_ANCHOR_END, TF_ANCHOR_START,
        TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
        TF_SELECTIONSTYLE,
    },
};
use windows_core::{Interface, Result, VARIANT, implement};

use crate::{
    ModuleRef,
    display::PREEDIT_ATTRIBUTE,
    document::{editable_selection, is_utf16_boundary},
    keymap::{self, Action, Context, Key},
    service::Activation,
};

const MAX_PREEDIT: usize = 4096;

#[derive(Default)]
pub(crate) struct Preedit {
    current: RefCell<Option<Rc<Owned>>>,
    pending_cleanup: RefCell<Option<Rc<Owned>>>,
    generation: Cell<u64>,
    busy: Cell<bool>,
    uncertain: Cell<bool>,
    display_atom: Cell<Option<u32>>,
}

struct Owned {
    context: ITfContext,
    composition: ITfComposition,
    original: Vec<u16>,
    text: RefCell<String>,
    validity: Rc<Validity>,
    _watch: EditWatch,
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
}

impl ITfCompositionSink_Impl for Observer_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Option<&ITfComposition>) -> Result<()> {
        self.validity.ended.set(true);
        Ok(())
    }
}

impl ITfTextEditSink_Impl for Observer_Impl {
    fn OnEndEdit(&self, _: Option<&ITfContext>, _: u32, _: Option<&ITfEditRecord>) -> Result<()> {
        if !self.validity.own_edit.get() {
            self.validity.modified.set(true);
        }
        Ok(())
    }
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
        let generation = self.generation.get();
        if generation == u64::MAX
            || self.uncertain.get()
            || !activation.owns_focus(context)
            || self.busy.replace(true)
        {
            return false;
        }
        let _busy = Busy(&self.busy);
        let completed = Rc::new(Cell::new(None));
        let own_edit = Rc::new(RefCell::new(None::<Rc<Validity>>));
        let session: ITfEditSession = Session {
            _module: ModuleRef::new(),
            activation: activation.clone(),
            context: context.clone(),
            generation,
            key,
            apply,
            completed: completed.clone(),
            own_edit: own_edit.clone(),
        }
        .into();
        let access = if apply { TF_ES_READWRITE } else { TF_ES_READ };
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
        let handled = completed.replace(Some(false)) == Some(true);
        if let Some(owned) = self.pending_cleanup.take() {
            Self::cleanup(owned, activation.client_id);
        }
        handled
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
    completed: Rc<Cell<Option<bool>>>,
    own_edit: Rc<RefCell<Option<Rc<Validity>>>>,
}

impl Session_Impl {
    fn current(&self) -> bool {
        let preedit = &self.activation.preedit;
        preedit.generation.get() == self.generation
            && !preedit.uncertain.get()
            && self.activation.owns_focus(&self.context)
            && preedit.generation.get() == self.generation
    }

    fn inspect(&self, cookie: u32) -> Option<(ITfRange, Option<Rc<Owned>>, String)> {
        if !self.current() {
            return None;
        }
        let Some(selection) = editable_selection(&self.context, cookie) else {
            if self.apply {
                self.activation
                    .preedit
                    .invalidate(None, self.activation.client_id);
            }
            return None;
        };
        let owned = self.activation.preedit.current.borrow().clone();
        let raw = match owned.as_ref() {
            Some(owned) if owned.context == self.context && owned.validity.owned() => {
                let range = owned.verified_range(cookie)?;
                if !unsafe { selection.IsEmpty(cookie).ok()?.as_bool() }
                    || unsafe { selection.CompareStart(cookie, &range, TF_ANCHOR_END).ok()? } != 0
                {
                    return None;
                }
                owned.text.borrow().clone()
            }
            Some(owned) if !owned.validity.ended.get() => return None,
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
        let action = keymap::action(
            self.key,
            Context {
                editable: true,
                preedit: &raw,
                selecting_candidate: false,
            },
        );
        match action {
            Some(Action::Append(character)) if raw.len() < MAX_PREEDIT => raw.push(character),
            Some(Action::ReplaceWithPrefix(prefix)) => raw.truncate(prefix),
            Some(Action::Cancel) => raw.clear(),
            _ => return Ok(()),
        }
        if !self.apply {
            self.completed.set(Some(true));
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
        let text: Vec<u16> = raw.encode_utf16().collect();
        let replacement = if ending { &owned.original } else { &text };
        let result = (|| unsafe {
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
        } else if ending {
            preedit.current.replace(None);
        } else {
            owned.text.replace(raw);
        }
        result
    }
}

impl Session_Impl {
    fn start(&self, cookie: u32, selection: &ITfRange, original: Vec<u16>) -> Result<Rc<Owned>> {
        let validity = Rc::new(Validity::default());
        let sink: ITfCompositionSink = Observer {
            _module: ModuleRef::new(),
            validity: validity.clone(),
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
            validity,
            _watch: watch,
        }))
    }
}

impl Owned {
    fn verified_range(&self, cookie: u32) -> Option<ITfRange> {
        let range = unsafe { self.composition.GetRange().ok()? };
        if !valid_boundaries(&range, cookie) {
            return None;
        }
        let expected_units = self.text.borrow().encode_utf16().count();
        let text = read_range(&range, cookie, expected_units)?;
        (text.iter().copied().eq(self.text.borrow().encode_utf16()) && self.validity.owned())
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
