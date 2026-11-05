//! Coalesced notification only: no job data, leases, callbacks or graph adoption.
use crate::node::CoreError;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    pending: bool,
    closed: bool,
}
#[derive(Default)]
pub(crate) struct Inner {
    state: Mutex<State>,
    changed: Condvar,
}
/// A single consumer's wake handle. Clones share one coalesced notification bit.
/// Drain `poll_one` to None before waiting again: notifications are not event counts.
/// Waiting never adopts a result or releases a physical job reservation.
#[derive(Clone, Default)]
pub struct CompletionWake {
    inner: Arc<Inner>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionWakeStatus {
    Notified,
    TimedOut,
    Closed,
}
impl CompletionWake {
    pub(crate) fn downgrade(&self) -> std::sync::Weak<Inner> {
        Arc::downgrade(&self.inner)
    }
    pub(crate) fn from_inner(inner: Arc<Inner>) -> Self {
        Self { inner }
    }
    /// None waits indefinitely. An unrepresentable native deadline is rejected
    /// before consuming a pending notification. Closed takes precedence on drop.
    pub fn wait(&self, timeout: Option<Duration>) -> Result<CompletionWakeStatus, CoreError> {
        let deadline = timeout
            .map(|timeout| {
                Instant::now().checked_add(timeout).ok_or_else(|| {
                    CoreError::new("invalid-deadline", "wake timeout exceeds native clock")
                })
            })
            .transpose()?;
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if state.closed {
                return Ok(CompletionWakeStatus::Closed);
            }
            if state.pending {
                state.pending = false;
                return Ok(CompletionWakeStatus::Notified);
            }
            state = if let Some(deadline) = deadline {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return Ok(CompletionWakeStatus::TimedOut);
                };
                self.inner
                    .changed
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|p| p.into_inner())
                    .0
            } else {
                self.inner
                    .changed
                    .wait(state)
                    .unwrap_or_else(|p| p.into_inner())
            };
        }
    }
    pub(crate) fn notify(&self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.closed {
            state.pending = true;
            self.inner.changed.notify_one();
        }
    }
    pub(crate) fn close(&self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        self.inner.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread};

    #[test]
    fn notifications_before_wait_are_coalesced_and_consumed_once() {
        let wake = CompletionWake::default();
        for _ in 0..1000 {
            wake.notify();
        }
        assert_eq!(
            wake.wait(Some(Duration::ZERO)).unwrap(),
            CompletionWakeStatus::Notified
        );
        assert_eq!(
            wake.wait(Some(Duration::ZERO)).unwrap(),
            CompletionWakeStatus::TimedOut
        );
    }
    #[test]
    fn notification_racing_with_wait_is_not_lost() {
        let wake = CompletionWake::default();
        let barrier = Arc::new(Barrier::new(2));
        let sender = wake.clone();
        let start = Arc::clone(&barrier);
        let producer = thread::spawn(move || {
            for _ in 0..200 {
                start.wait();
                sender.notify();
            }
        });
        for _ in 0..200 {
            barrier.wait();
            assert_eq!(
                wake.wait(Some(Duration::from_secs(5))).unwrap(),
                CompletionWakeStatus::Notified
            );
        }
        producer.join().unwrap();
    }
    #[test]
    fn close_releases_an_indefinite_wait_and_remains_closed() {
        let wake = CompletionWake::default();
        let receiver = wake.clone();
        let waiter = thread::spawn(move || receiver.wait(None).unwrap());
        wake.close();
        let first = waiter.join().unwrap();
        assert_eq!(first, CompletionWakeStatus::Closed);
        wake.notify();
        assert_eq!(wake.wait(None).unwrap(), CompletionWakeStatus::Closed);
    }
    #[test]
    fn native_timeout_overflow_does_not_consume_a_notification() {
        let wake = CompletionWake::default();
        wake.notify();
        assert_eq!(
            wake.wait(Some(Duration::MAX)).unwrap_err().code,
            "invalid-deadline"
        );
        assert_eq!(
            wake.wait(Some(Duration::ZERO)).unwrap(),
            CompletionWakeStatus::Notified
        );
    }
    #[test]
    fn closure_takes_precedence_over_a_pending_notification() {
        let wake = CompletionWake::default();
        wake.notify();
        wake.close();
        assert_eq!(wake.wait(None).unwrap(), CompletionWakeStatus::Closed);
    }
    #[test]
    fn timeout_does_not_close_future_notifications() {
        let wake = CompletionWake::default();
        assert_eq!(
            wake.wait(Some(Duration::from_millis(1))).unwrap(),
            CompletionWakeStatus::TimedOut
        );
        wake.notify();
        assert_eq!(
            wake.clone().wait(None).unwrap(),
            CompletionWakeStatus::Notified
        );
    }
}
