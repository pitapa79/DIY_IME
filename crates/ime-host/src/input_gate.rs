//! Trusted pre-dispatch gate. It never receives input bodies or Node-provided flags.

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
    pending: Option<Ticket>,
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
        self.manual || self.automatic || self.pending.is_some()
    }

    pub fn set_manual<E>(
        &mut self,
        value: bool,
        invalidate: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        self.set_flags(value, self.automatic, invalidate)
    }

    pub fn set_automatic<E>(
        &mut self,
        value: bool,
        invalidate: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        self.set_flags(self.manual, value, invalidate)
    }

    fn set_flags<E>(
        &mut self,
        manual: bool,
        automatic: bool,
        invalidate: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        if manual == self.manual && automatic == self.automatic && self.pending.is_none() {
            return Ok(());
        }
        let ticket = self.begin_invalidation(manual, automatic);
        invalidate()?;
        self.finish_invalidation(&ticket);
        Ok(())
    }

    pub(crate) fn begin_invalidation(&mut self, manual: bool, automatic: bool) -> Ticket {
        self.manual = manual;
        self.automatic = automatic;
        self.epoch = Arc::new(());
        let ticket = Ticket(self.epoch.clone());
        self.pending = Some(ticket.clone());
        ticket
    }

    pub(crate) fn finish_invalidation(&mut self, ticket: &Ticket) -> bool {
        if !self
            .pending
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(&pending.0, &ticket.0))
        {
            return false;
        }
        self.pending = None;
        true
    }

    /// Deliver Node/learning/debug/network data only inside the closure.
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
