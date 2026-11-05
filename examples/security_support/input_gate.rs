//! Trusted pre-dispatch gate. No input bodies or Node-provided protection flags.
use std::sync::Arc;
#[derive(Clone)]
pub struct Ticket(Arc<()>);
#[derive(Debug, PartialEq, Eq)]
pub enum Denial {
    Protected,
    NotAllowed,
}
#[derive(Default)]
pub struct InputGate {
    manual: bool,
    automatic: bool,
    failed: bool,
    epoch: Arc<()>,
}
impl InputGate {
    pub fn manual(&self) -> bool {
        self.manual
    }
    pub fn automatic(&self) -> bool {
        self.automatic
    }
    pub fn blocked(&self) -> bool {
        self.manual || self.automatic || self.failed
    }
    /// Only the trusted input adapter may call these setters. On failure remain
    /// closed, including when leaving protection, until invalidation succeeds.
    pub fn set_manual<E>(
        &mut self,
        value: bool,
        invalidate: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        let changed = self.manual != value;
        self.manual = value;
        self.transition(changed, invalidate)
    }
    pub fn set_automatic<E>(
        &mut self,
        value: bool,
        invalidate: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        let changed = self.automatic != value;
        self.automatic = value;
        self.transition(changed, invalidate)
    }
    fn transition<E>(
        &mut self,
        changed: bool,
        invalidate: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        if !changed && !self.failed {
            return Ok(());
        }
        self.epoch = Arc::new(());
        self.failed = true;
        invalidate()?;
        self.failed = false;
        Ok(())
    }
    /// Place all Node/learning/debug/network delivery inside the closure. This
    /// gate never receives or stores a body, key event, context, or queued input.
    pub fn dispatch<R>(
        &self,
        allowed: bool,
        deliver: impl FnOnce() -> R,
    ) -> Result<(Ticket, R), Denial> {
        if self.blocked() {
            return Err(Denial::Protected);
        }
        if !allowed {
            return Err(Denial::NotAllowed);
        }
        Ok((Ticket(self.epoch.clone()), deliver()))
    }
    pub fn accepts(&self, ticket: &Ticket) -> bool {
        !self.blocked() && Arc::ptr_eq(&self.epoch, &ticket.0)
    }
}
