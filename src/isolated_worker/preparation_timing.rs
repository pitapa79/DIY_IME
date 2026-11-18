//! Private progress diagnostics for package verification before activation.
use std::cell::Cell;
use std::io::{self, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();
static NEXT: AtomicU32 = AtomicU32::new(0);
thread_local! {
    static METADATA: Cell<bool> = const { Cell::new(false) };
}

fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("DIY_EXPERIMENT_PACKAGE_PREPARATION_TIMING").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    })
}

pub(super) struct Span {
    phase: &'static str,
    timing: Option<(u32, Instant)>,
}

impl Span {
    pub(super) fn start(phase: &'static str) -> Self {
        Self::when(phase, enabled())
    }

    pub(super) fn worker(phase: &'static str) -> Self {
        Self::when(phase, METADATA.get())
    }

    fn when(phase: &'static str, active: bool) -> Self {
        let timing = active
            .then(|| {
                NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                    (id < 128).then_some(id + 1)
                })
                .ok()
            })
            .flatten()
            .map(|id| {
                emit(id, phase, "start", 0);
                (id, Instant::now())
            });
        Self { phase, timing }
    }

    pub(super) fn complete(mut self) {
        self.end("complete");
    }

    fn end(&mut self, event: &str) {
        if let Some((id, started)) = self.timing.take() {
            emit(id, self.phase, event, started.elapsed().as_micros());
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        self.end("aborted");
    }
}

fn emit(id: u32, phase: &str, event: &str, elapsed_us: u128) {
    let mut output = io::stderr().lock();
    let _ = writeln!(
        output,
        "package-preparation id={id} phase={phase} event={event} us={elapsed_us}"
    );
    let _ = output.flush();
}

pub(super) fn metadata<T, E>(
    phase: &'static str,
    call: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if !enabled() {
        return call();
    }
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            METADATA.set(self.0);
        }
    }
    let _restore = Restore(METADATA.replace(true));
    let span = Span::start(phase);
    let result = call();
    if result.is_ok() {
        span.complete();
    }
    result
}
