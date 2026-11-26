//! Trust-side state for platform IME adapters. It owns intent and generations;
//! the platform owns edit cookies and live document ranges.

pub mod input_gate;
pub mod utf16;

use std::{ops::Range, sync::Arc};

use diy_ime2_runtime::{
    execution_control::{DecimalId, RunKey, SessionEpoch},
    node::{InputMode, MAX_INPUT_REVISION, RuntimeInput},
};

use input_gate::{InputGate, Ticket};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentStamp {
    /// Changes on every focus acquisition, including reacquisition of the same context.
    pub focus: u64,
    pub context: u64,
    pub revision: u64,
    pub selection: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputScope {
    Editable,
    Protected,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub session: SessionEpoch,
    pub version: DecimalId,
}

#[derive(Clone, Debug)]
pub struct RequestId(Arc<()>);

impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RequestId {}

#[derive(Clone, Debug)]
pub struct CompositionId(Arc<()>);

impl PartialEq for CompositionId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CompositionId {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditTarget {
    pub document: DocumentStamp,
    pub composition: CompositionId,
    /// False means this operation can use only the original, validated selection.
    pub has_composition_range: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Edit<'a> {
    Preview(&'a str),
    Commit(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditOutcome {
    Applied {
        document: DocumentStamp,
        has_composition_range: bool,
    },
    Unchanged,
    Uncertain,
}

/// Implemented by the adapter inside a granted write session. Recheck the
/// live context, selection/range, ownership and protection before any mutation.
/// An empty commit may clear an owned composition but never replace a selection.
/// Return Unchanged only when no write could have occurred.
pub trait EditSession {
    /// Inspect only the local range needed to prove the two UTF-16 boundaries.
    /// Return None if the target, range ownership or protection cannot be confirmed.
    fn snapshot(&mut self, target: &EditTarget) -> Option<EditSnapshot>;
    fn edit(&mut self, target: &EditTarget, operation: Edit<'_>) -> EditOutcome;
}

pub struct EditSnapshot {
    pub utf16: Vec<u16>,
    pub range: Range<usize>,
}

/// On any invalidation, first discard queued results/submissions, then revoke
/// the old run(s) through the Host. Cleanup is limited to the exact old
/// composition if ownership can still be proven. It never uses the new selection.
pub struct Invalidation {
    /// None means protocol revision exhaustion. Keep this input path closed.
    pub revision: Option<u64>,
    pub cleanup: Option<EditTarget>,
    ticket: Ticket,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StateError {
    Protected,
    NoComposition,
    Busy,
    Stale,
    InvalidInput,
    InvalidTarget,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Conversion {
    Text(String),
    NoOutput,
    Invalid,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Ready,
    NoOutput,
    Invalid,
}

pub enum Application {
    Applied,
    Unchanged,
    Uncertain(Invalidation),
}

struct Composition {
    target: EditTarget,
    input: RuntimeInput,
}

struct Request {
    id: RequestId,
    phase: Phase,
}

enum Phase {
    Queued,
    Dispatched(Ticket),
    Running(Ticket, RunKey),
    Ready(Ticket, RunKey, String),
}

pub struct InputState {
    plan: Plan,
    document: Option<DocumentStamp>,
    quarantined: Option<DocumentStamp>,
    scope: InputScope,
    revision: u64,
    gate: InputGate,
    composition: Option<Composition>,
    request: Option<Request>,
}

impl InputState {
    pub fn new(plan: Plan) -> Self {
        Self {
            plan,
            document: None,
            quarantined: None,
            scope: InputScope::Unknown,
            revision: 0,
            gate: InputGate::default(),
            composition: None,
            request: None,
        }
    }

    pub fn document(&self) -> Option<DocumentStamp> {
        self.document
    }

    pub fn accepts_input(&self) -> bool {
        self.document.is_some() && !self.gate.blocked()
    }

    /// Report a new trusted observation when focus, text, selection or scope changes.
    /// The adapter must generate a new stamp for a new snapshot and for each focus return.
    pub fn observe_document(
        &mut self,
        document: Option<DocumentStamp>,
        scope: InputScope,
    ) -> Option<Invalidation> {
        if document.is_some() && self.quarantined == document {
            return None;
        }
        if self.document == document && self.scope == scope {
            return None;
        }
        if document.is_some() {
            self.quarantined = None;
        }
        self.document = document;
        self.scope = scope;
        Some(self.invalidate(self.gate.manual(), true))
    }

    /// A live OS composition ended independently: drop requests and the reference
    /// without issuing a cleanup edit against the range that just ended.
    pub fn composition_ended(&mut self, owner: &CompositionId) -> Option<Invalidation> {
        if self.composition.as_ref()?.target.composition != *owner {
            return None;
        }
        Some(self.invalidate(self.gate.manual(), false))
    }

    pub fn set_manual_protection(&mut self, active: bool) -> Option<Invalidation> {
        (self.gate.manual() != active).then(|| self.invalidate(active, true))
    }

    pub fn cancel(&mut self) -> Option<Invalidation> {
        self.composition.as_ref()?;
        Some(self.invalidate(self.gate.manual(), true))
    }

    /// Check this before authorizing the Host plan switch. A forced Host change
    /// is instead handled by invalidating the old session/target at the adapter.
    pub fn can_switch_plan(&self) -> bool {
        self.composition.is_none() && self.request.is_none()
    }

    pub fn switch_plan(&mut self, plan: Plan) -> Result<Option<Invalidation>, StateError> {
        if !self.can_switch_plan() {
            return Err(StateError::Busy);
        }
        if self.plan == plan {
            return Ok(None);
        }
        self.plan = plan;
        Ok(Some(self.invalidate(self.gate.manual(), false)))
    }

    /// Acknowledge only after the trusted Host has invalidated old delivery.
    /// Failure is represented by retaining this operation to retry, without acknowledgement.
    pub fn acknowledge_invalidation(&mut self, invalidation: &Invalidation) -> bool {
        invalidation.revision.is_some() && self.gate.finish_invalidation(&invalidation.ticket)
    }

    fn invalidate(&mut self, manual: bool, allow_cleanup: bool) -> Invalidation {
        let automatic = self.document.is_none() || self.scope != InputScope::Editable;
        let ticket = self.gate.begin_invalidation(manual, automatic);
        self.request = None;
        let cleanup = self
            .composition
            .take()
            .map(|composition| composition.target)
            .filter(|target| allow_cleanup && target.has_composition_range);
        // Never reuse a preceding revision for new input or invalidation, and
        // leave the gate closed permanently if the protocol range is exhausted.
        let revision = self
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_INPUT_REVISION);
        if let Some(revision) = revision {
            self.revision = revision;
        }
        Invalidation {
            revision,
            cleanup,
            ticket,
        }
    }

    pub fn replace_input(&mut self, text: String) -> Result<RequestId, StateError> {
        let document = self
            .document
            .filter(|_| self.accepts_input())
            .ok_or(StateError::Protected)?;
        let input = self.next_input(text, InputMode::Compose)?;
        if let Some(composition) = &mut self.composition {
            composition.input = input;
        } else {
            self.composition = Some(Composition {
                target: EditTarget {
                    document,
                    composition: CompositionId(Arc::new(())),
                    has_composition_range: false,
                },
                input,
            });
        }
        Ok(self.queue_request())
    }

    pub fn commit(&mut self) -> Result<RequestId, StateError> {
        if !self.accepts_input() {
            return Err(StateError::Protected);
        }
        let composition = self.composition.as_ref().ok_or(StateError::NoComposition)?;
        if composition.input.mode == InputMode::Finalize && self.request.is_some() {
            return Err(StateError::Busy);
        }
        let input = self.next_input(composition.input.text.clone(), InputMode::Finalize)?;
        self.composition.as_mut().expect("composition exists").input = input;
        Ok(self.queue_request())
    }

    fn next_input(&self, text: String, mode: InputMode) -> Result<RuntimeInput, StateError> {
        let input = RuntimeInput {
            revision: self
                .revision
                .checked_add(1)
                .ok_or(StateError::InvalidInput)?,
            text,
            mode,
        };
        input.validate().map_err(|_| StateError::InvalidInput)?;
        Ok(input)
    }

    fn queue_request(&mut self) -> RequestId {
        self.revision = self
            .composition
            .as_ref()
            .expect("composition exists")
            .input
            .revision;
        let id = RequestId(Arc::new(()));
        self.request = Some(Request {
            id: id.clone(),
            phase: Phase::Queued,
        });
        id
    }

    /// Place dispatch at the last trusted boundary before Host submission.
    /// Once dispatched, a lost reply must be resolved or failed explicitly.
    pub fn dispatch<T>(
        &mut self,
        id: &RequestId,
        submit: impl FnOnce(&Plan, &RuntimeInput) -> T,
    ) -> Result<T, StateError> {
        let (request, composition) = self.current_request(id)?;
        if !matches!(request.phase, Phase::Queued) {
            return Err(StateError::Stale);
        }
        let (ticket, result) = self
            .gate
            .dispatch(true, || submit(&self.plan, &composition.input))
            .map_err(|_| StateError::Protected)?;
        self.request.as_mut().expect("request exists").phase = Phase::Dispatched(ticket);
        Ok(result)
    }

    /// Accept the key only from the response to the corresponding trusted submission.
    pub fn accept_run(&mut self, id: &RequestId, key: RunKey) -> Result<(), StateError> {
        let (request, composition) = self.current_request(id)?;
        let Phase::Dispatched(ticket) = &request.phase else {
            return Err(StateError::Stale);
        };
        if !self.gate.accepts(ticket)
            || key.session_epoch != self.plan.session
            || key.plan_version != self.plan.version
            || key.input_revision != Some(composition.input.revision)
        {
            return Err(StateError::Stale);
        }
        self.request.as_mut().expect("request exists").phase = Phase::Running(ticket.clone(), key);
        Ok(())
    }

    pub fn receive(
        &mut self,
        id: &RequestId,
        key: &RunKey,
        output: Conversion,
    ) -> Result<Delivery, StateError> {
        let (request, _) = self.current_request(id)?;
        let Phase::Running(ticket, accepted) = &request.phase else {
            return Err(StateError::Stale);
        };
        if accepted != key || !self.gate.accepts(ticket) {
            return Err(StateError::Stale);
        }
        let delivery = match output {
            Conversion::Text(text) => {
                self.request.as_mut().expect("request exists").phase =
                    Phase::Ready(ticket.clone(), key.clone(), text);
                Delivery::Ready
            }
            Conversion::NoOutput => {
                self.request = None;
                Delivery::NoOutput
            }
            Conversion::Invalid => {
                self.request = None;
                Delivery::Invalid
            }
        };
        Ok(delivery)
    }

    /// Fail the corresponding execution without confusing it with empty text.
    pub fn execution_failed(&mut self, id: &RequestId) -> Result<(), StateError> {
        let (request, _) = self.current_request(id)?;
        if matches!(request.phase, Phase::Ready(..)) {
            return Err(StateError::Stale);
        }
        self.request = None;
        Ok(())
    }

    pub fn apply(
        &mut self,
        id: &RequestId,
        session: &mut impl EditSession,
    ) -> Result<Application, StateError> {
        let (request, composition) = self.current_request(id)?;
        let Phase::Ready(ticket, key, text) = &request.phase else {
            return Err(StateError::Stale);
        };
        if !self.gate.accepts(ticket) {
            return Err(StateError::Protected);
        }
        if key.session_epoch != self.plan.session
            || key.plan_version != self.plan.version
            || key.input_revision != Some(composition.input.revision)
        {
            return Err(StateError::Stale);
        }
        let target = &composition.target;
        let valid_range = session
            .snapshot(target)
            .is_some_and(|snapshot| utf16::utf8_selection(&snapshot.utf16, snapshot.range).is_ok());
        if !valid_range {
            self.request = None;
            return Err(StateError::InvalidTarget);
        }
        let commit = composition.input.mode == InputMode::Finalize;
        let operation = if commit {
            Edit::Commit(text)
        } else {
            Edit::Preview(text)
        };
        let outcome = session.edit(target, operation);
        match outcome {
            EditOutcome::Unchanged => Ok(Application::Unchanged),
            EditOutcome::Applied {
                document,
                has_composition_range,
            } if !(commit && has_composition_range)
                && document.focus == target.document.focus
                && document.context == target.document.context
                && document.revision >= target.document.revision =>
            {
                self.document = Some(document);
                self.request = None;
                if commit {
                    self.composition = None;
                } else {
                    let composition = self.composition.as_mut().expect("composition exists");
                    composition.target.document = document;
                    composition.target.has_composition_range = has_composition_range;
                }
                Ok(Application::Applied)
            }
            EditOutcome::Uncertain | EditOutcome::Applied { .. } => {
                self.quarantined = self.document.take();
                self.scope = InputScope::Unknown;
                Ok(Application::Uncertain(
                    self.invalidate(self.gate.manual(), false),
                ))
            }
        }
    }

    fn current_request(&self, id: &RequestId) -> Result<(&Request, &Composition), StateError> {
        let request = self
            .request
            .as_ref()
            .filter(|request| request.id == *id)
            .ok_or(StateError::Stale)?;
        let composition = self.composition.as_ref().ok_or(StateError::Stale)?;
        if !self.accepts_input() {
            return Err(StateError::Protected);
        }
        if self.document != Some(composition.target.document) {
            return Err(StateError::Stale);
        }
        Ok((request, composition))
    }
}
