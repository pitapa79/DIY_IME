//! Private, bounded trusted-Worker metadata. Never part of the guest wire.
use crate::execution_control::WorkerTracePhase;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    time::Instant,
};
const MAGIC: &[u8; 8] = b"DIYTRC1\0";
pub(super) const LIMIT: usize = 4096;

pub(super) struct Recorder {
    output: RefCell<Option<File>>,
    inline: RefCell<Option<Vec<u8>>>,
    deferred: bool,
    origin: Cell<Option<Instant>>,
    frames: Cell<u8>,
}
impl Recorder {
    pub(super) fn new(mut file: File) -> Self {
        let valid = file.write_all(MAGIC).is_ok();
        let result = Self {
            output: RefCell::new(valid.then_some(file)),
            inline: RefCell::new(None),
            deferred: false,
            origin: Cell::new(Some(Instant::now())),
            frames: Cell::new(0),
        };
        result.write(0, WorkerTracePhase::Main, 0);
        result
    }
    pub(super) fn deferred() -> Self {
        Self {
            output: RefCell::new(None),
            inline: RefCell::new(None),
            deferred: true,
            origin: Cell::new(None),
            frames: Cell::new(0),
        }
    }
    pub(super) fn is_deferred(&self) -> bool {
        self.deferred
    }
    pub(super) fn enable_inline(&self) {
        if !self.deferred || self.origin.get().is_some() || self.frames.get() != 0 {
            return;
        }
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(MAGIC.len() + 24 * 10).is_err() {
            return;
        }
        bytes.extend_from_slice(MAGIC);
        *self.inline.borrow_mut() = Some(bytes);
        self.origin.set(Some(Instant::now()));
        self.write(0, WorkerTracePhase::Main, 0);
    }
    pub(super) fn finish_inline(&self) -> Option<Vec<u8>> {
        if !self.deferred || self.origin.get().is_none() {
            return None;
        }
        self.event(1, WorkerTracePhase::Main);
        self.origin.set(None);
        self.inline.borrow_mut().take()
    }
    fn write(&self, kind: u8, phase: WorkerTracePhase, at: u64) {
        let Ok(mut output) = self.output.try_borrow_mut() else {
            return;
        };
        // Twelve static start/finish pairs fit well below the 4096-byte pipe
        // buffer even if the Parent reader has not yet been scheduled.
        if self.frames.get() >= 24 {
            *output = None;
            *self.inline.borrow_mut() = None;
            return;
        }
        let mut frame = [0u8; 10];
        frame[0] = kind;
        frame[1] = phase as u8;
        frame[2..].copy_from_slice(&at.to_le_bytes());
        if let Some(file) = output.as_mut() {
            if file.write_all(&frame).is_err() {
                *output = None;
            } else {
                self.frames.set(self.frames.get() + 1);
            }
        } else if let Some(bytes) = self.inline.borrow_mut().as_mut() {
            bytes.extend_from_slice(&frame);
            self.frames.set(self.frames.get() + 1);
        }
    }
    fn event(&self, kind: u8, phase: WorkerTracePhase) {
        let Some(origin) = self.origin.get() else {
            return;
        };
        if self.output.borrow().is_none() && self.inline.borrow().is_none() {
            return;
        }
        self.write(
            kind,
            phase,
            u64::try_from(origin.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
    }
    pub(super) fn span(&self, phase: WorkerTracePhase) -> Span<'_> {
        self.event(0, phase);
        Span {
            recorder: self,
            phase,
            active: self.origin.get().is_some(),
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.event(1, WorkerTracePhase::Main);
    }
}
pub(super) struct Span<'a> {
    recorder: &'a Recorder,
    phase: WorkerTracePhase,
    active: bool,
}
impl Drop for Span<'_> {
    fn drop(&mut self) {
        if self.active {
            self.recorder.event(1, self.phase);
        }
    }
}

/// Drain even on overflow: diagnostics cannot terminate or block guest stdout.
pub(super) fn read(mut input: impl Read) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 512];
    let mut overflow = false;
    loop {
        match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if bytes.len().saturating_add(n) <= LIMIT
                    && !overflow
                    && bytes.try_reserve(n).is_ok()
                {
                    bytes.extend_from_slice(&buf[..n]);
                } else {
                    overflow = true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(_) => return Err(()),
        }
    }
    if overflow { Err(()) } else { Ok(bytes) }
}
pub(super) struct Interval {
    pub phase: WorkerTracePhase,
    pub start: u64,
    pub end: Option<u64>,
}
pub(super) fn decode(bytes: &[u8]) -> Result<Vec<Interval>, ()> {
    if bytes.len() > LIMIT
        || !bytes.starts_with(MAGIC)
        || !(bytes.len() - MAGIC.len()).is_multiple_of(10)
    {
        return Err(());
    }
    let mut phases: BTreeMap<WorkerTracePhase, (u64, Option<u64>)> = BTreeMap::new();
    let mut order = Vec::new();
    let mut previous = 0;
    let mut ended = false;
    let mut retried = [false; 2];
    for (i, frame) in bytes[MAGIC.len()..].chunks_exact(10).enumerate() {
        let phase = WorkerTracePhase::try_from(frame[1])?;
        let at = u64::from_le_bytes(frame[2..].try_into().map_err(|_| ())?);
        if ended
            || at < previous
            || i == 0 && (frame[0] != 0 || phase != WorkerTracePhase::Main || at != 0)
        {
            return Err(());
        }
        previous = at;
        match frame[0] {
            0 => {
                // JIT preparation may retry once with the standard compiler.
                // Keep the first start and report the final end for each phase.
                if let Some((_, end)) = phases.get_mut(&phase) {
                    let retry = match phase {
                        WorkerTracePhase::EngineSetup => &mut retried[0],
                        WorkerTracePhase::Compile => &mut retried[1],
                        _ => return Err(()),
                    };
                    if *retry || end.take().is_none() {
                        return Err(());
                    }
                    *retry = true;
                    continue;
                }
                phases.insert(phase, (at, None));
                order.push(phase);
            }
            1 => {
                let (_, end) = phases.get_mut(&phase).ok_or(())?;
                if end.replace(at).is_some() {
                    return Err(());
                }
                ended = phase == WorkerTracePhase::Main;
            }
            _ => return Err(()),
        }
    }
    if phases.is_empty() {
        return Err(());
    }
    Ok(order
        .into_iter()
        .map(|phase| {
            let (start, end) = phases[&phase];
            Interval { phase, start, end }
        })
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deferred_off_has_no_clock_file_buffer_or_frames() {
        let recorder = Recorder::deferred();
        for _ in 0..1000 {
            drop(recorder.span(WorkerTracePhase::Compile));
        }
        assert!(recorder.origin.get().is_none());
        assert!(recorder.output.borrow().is_none());
        assert!(recorder.inline.borrow().is_none());
        assert_eq!(recorder.frames.get(), 0);
        assert!(recorder.finish_inline().is_none());
    }
    #[test]
    fn deferred_on_records_only_observed_phases_and_finishes_once() {
        let recorder = Recorder::deferred();
        let unobserved = recorder.span(WorkerTracePhase::Compile);
        recorder.enable_inline();
        drop(unobserved); // cannot manufacture an end for the unobserved start
        drop(recorder.span(WorkerTracePhase::GuestCall));
        drop(recorder.span(WorkerTracePhase::Cleanup));
        let bytes = recorder.finish_inline().unwrap();
        assert!(bytes.len() <= MAGIC.len() + 24 * 10);
        let intervals = decode(&bytes).unwrap();
        assert_eq!(intervals.len(), 3);
        assert!(intervals.iter().all(|phase| phase.end.is_some()));
        assert!(
            !intervals
                .iter()
                .any(|phase| phase.phase == WorkerTracePhase::Compile)
        );
        assert!(recorder.finish_inline().is_none());
        assert!(recorder.origin.get().is_none());
        recorder.enable_inline();
        assert!(recorder.origin.get().is_none());
    }
    #[test]
    fn inline_overflow_loses_metadata_without_growing_the_buffer() {
        let recorder = Recorder::deferred();
        recorder.enable_inline();
        for _ in 0..1000 {
            drop(recorder.span(WorkerTracePhase::GuestCall));
        }
        assert!(recorder.inline.borrow().is_none());
        assert_eq!(recorder.frames.get(), 24);
        assert!(recorder.finish_inline().is_none());
    }
    fn stream(frames: &[(u8, u8, u64)]) -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        for (k, p, t) in frames {
            b.extend([*k, *p]);
            b.extend(t.to_le_bytes());
        }
        b
    }
    #[test]
    fn incomplete_intervals_do_not_claim_completion() {
        let v = decode(&stream(&[(0, 0, 0), (0, 5, 11), (1, 5, 32), (0, 6, 33)])).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].end, Some(32));
        assert!(v[0].end.is_none());
        assert!(v[2].end.is_none());
    }
    #[test]
    fn unknown_duplicate_backwards_unowned_and_truncated_frames_are_rejected() {
        for frames in [
            vec![],
            vec![(0, 0, 1)],
            vec![(1, 0, 0)],
            vec![(0, 0, 0), (0, 99, 1)],
            vec![(0, 0, 0), (2, 5, 1)],
            vec![(0, 0, 0), (1, 5, 1)],
            vec![(0, 0, 0), (0, 5, 2), (0, 5, 3)],
            vec![(0, 0, 0), (0, 5, 2), (1, 5, 1)],
            vec![(0, 0, 0), (1, 0, 1), (0, 5, 2)],
        ] {
            assert!(decode(&stream(&frames)).is_err());
        }
        let valid = stream(&[(0, 0, 0), (0, 5, 1), (1, 5, 2), (1, 0, 3)]);
        for n in 0..valid.len() {
            if n < 8 || !(n - 8).is_multiple_of(10) || n == 8 {
                assert!(decode(&valid[..n]).is_err());
            }
        }
        let mut body = valid.clone();
        body.extend_from_slice(b"PRIVATE-CANARY");
        assert!(decode(&body).is_err());
    }
    #[test]
    fn jit_retry_keeps_first_start_and_final_end() {
        let bytes = stream(&[
            (0, 0, 0),
            (0, 4, 1),
            (1, 4, 2),
            (0, 5, 3),
            (1, 5, 4),
            (0, 4, 5),
            (1, 4, 6),
            (0, 5, 7),
            (1, 5, 8),
            (0, 6, 9),
            (1, 6, 10),
            (1, 0, 11),
        ]);
        let intervals = decode(&bytes).unwrap();
        assert_eq!(intervals.len(), 4);
        assert_eq!((intervals[1].start, intervals[1].end), (1, Some(6)));
        assert_eq!((intervals[2].start, intervals[2].end), (3, Some(8)));
    }

    #[test]
    fn oversized_diagnostics_are_drained_but_never_retained() {
        let mut input = std::io::Cursor::new(vec![0u8; LIMIT * 3]);
        assert!(read(&mut input).is_err());
        assert_eq!(input.position(), (LIMIT * 3) as u64);
    }
    #[test]
    fn recorder_is_bounded_even_if_a_future_caller_repeats_a_phase() {
        let path = std::env::temp_dir().join(format!(
            "diyime-trace-bound-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let recorder = Recorder::new(file);
        for _ in 0..1000 {
            drop(recorder.span(WorkerTracePhase::Compile));
        }
        drop(recorder);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(bytes.len() <= MAGIC.len() + 24 * 10);
        assert!(decode(&bytes).is_err());
    }
}
