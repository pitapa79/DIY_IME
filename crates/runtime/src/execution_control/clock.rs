//! Trusted Core clocks. Clock reads never execute user callbacks.
use super::CompletionWake;
use crate::node::CoreError;
use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug)]
enum Source {
    Native { origin: Instant, offset: Duration },
    Controlled(Mutex<Duration>),
}
struct Inner {
    source: Source,
    wakes: Mutex<Vec<Weak<super::wake::Inner>>>,
    revision: AtomicU64,
}
/// Share one clock across all sessions owned by a control service. Controlled
/// clocks are host/test inputs; no wire request can supply or advance this clock.
#[derive(Clone)]
pub struct ExecutionClock {
    inner: Arc<Inner>,
}
impl std::fmt::Debug for ExecutionClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionClock")
            .field("source", &self.inner.source)
            .finish_non_exhaustive()
    }
}
impl ExecutionClock {
    pub fn native(offset: Duration) -> Result<Self, CoreError> {
        let clock = Self {
            inner: Arc::new(Inner {
                source: Source::Native {
                    origin: Instant::now(),
                    offset,
                },
                wakes: Mutex::new(vec![]),
                revision: AtomicU64::new(0),
            }),
        };
        clock.now()?;
        Ok(clock)
    }
    pub fn controlled(now: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                source: Source::Controlled(Mutex::new(now)),
                wakes: Mutex::new(vec![]),
                revision: AtomicU64::new(0),
            }),
        }
    }
    pub fn now(&self) -> Result<Duration, CoreError> {
        match &self.inner.source {
            Source::Native { origin, offset } => {
                offset.checked_add(origin.elapsed()).ok_or_else(|| {
                    CoreError::new("invalid-clock", "native clock exceeds duration domain")
                })
            }
            Source::Controlled(now) => Ok(*now.lock().unwrap_or_else(|p| p.into_inner())),
        }
    }
    pub fn advance_to(&self, now: Duration) -> Result<(), CoreError> {
        let Source::Controlled(current) = &self.inner.source else {
            return Err(CoreError::new(
                "invalid-clock",
                "native clock cannot be advanced by the caller",
            ));
        };
        let mut current = current.lock().unwrap_or_else(|p| p.into_inner());
        if now < *current {
            return Err(CoreError::new(
                "invalid-clock",
                "controlled clock moved backwards",
            ));
        }
        if now == *current {
            return Ok(());
        }
        let revision = self
            .inner
            .revision
            .load(Ordering::Relaxed)
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "clock revision exhausted"))?;
        *current = now;
        self.inner.revision.store(revision, Ordering::Release);
        drop(current);
        let mut wakes = self.inner.wakes.lock().unwrap_or_else(|p| p.into_inner());
        wakes.retain(|weak| {
            if let Some(inner) = weak.upgrade() {
                CompletionWake::from_inner(inner).notify();
                true
            } else {
                false
            }
        });
        Ok(())
    }
    pub(crate) fn register(&self, wake: &CompletionWake) {
        let weak = wake.downgrade();
        let mut wakes = self.inner.wakes.lock().unwrap_or_else(|p| p.into_inner());
        wakes.retain(|wake| wake.strong_count() > 0);
        if !wakes.iter().any(|wake| Weak::ptr_eq(wake, &weak)) {
            wakes.push(weak);
        }
    }
    pub(crate) fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::Acquire)
    }
    pub(crate) fn wall_time(&self) -> bool {
        matches!(self.inner.source, Source::Native { .. })
    }
    pub fn same_clock(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}
#[derive(Clone)]
pub(crate) struct AdmissionClock {
    pub clock: ExecutionClock,
    pub minimum: Duration,
    pub before: Option<Duration>,
}
impl AdmissionClock {
    pub fn sample(&self) -> Result<Duration, CoreError> {
        let now = self.clock.now()?;
        if now < self.minimum {
            return Err(CoreError::new(
                "invalid-clock",
                "admission clock moved backwards",
            ));
        }
        if self.before.is_some_and(|deadline| deadline <= now) {
            return Err(CoreError::new(
                "clock-retry",
                "process existing deadlines before reserving this start",
            ));
        }
        Ok(now)
    }
}
