//! Bounded trusted-Native call metadata, never a guest reply or authority.
use super::resource_transport::ResourceReader;
use std::{cell::Cell, collections::VecDeque, io, sync::Mutex, time::Instant};

pub(super) const WIRE_SIZE: usize = 52;
const SAMPLE_CAPACITY: usize = 16;

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Sample {
    pub run_id: crate::execution_control::DecimalId,
    pub plan_version: crate::execution_control::DecimalId,
    pub input_revision: Option<u64>,
    pub delivery_generation: crate::execution_control::DecimalId,
    pub stage: usize,
    pub generation: u64,
    pub process_id: Option<u64>,
    pub parent_call_ns: Option<u64>,
    pub outcome: &'static str,
    pub native: Option<Metrics>,
}
struct Ring {
    samples: VecDeque<Sample>,
    dropped: u64,
}
pub(super) struct Samples {
    ring: Mutex<Ring>,
    _budget: crate::execution_control::ResourceLease,
}
impl Samples {
    pub(super) fn new(
        host: &crate::execution_control::ExecutionHost,
    ) -> Result<Self, crate::node::CoreError> {
        // Fixed-size control records only: no guest strings, result or input.
        // Conservatively cover the ring and bounded status/serialization copies.
        let budget = host.try_reserve(crate::execution_control::ResourceUsage {
            retained_bytes: (SAMPLE_CAPACITY * 8192) as u64,
            ..crate::execution_control::ResourceUsage::default()
        })?;
        Ok(Self {
            ring: Mutex::new(Ring {
                samples: VecDeque::with_capacity(SAMPLE_CAPACITY),
                dropped: 0,
            }),
            _budget: budget,
        })
    }
    pub(super) fn push(&self, sample: Sample) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.samples.len() == SAMPLE_CAPACITY {
            ring.samples.pop_front();
            ring.dropped = ring.dropped.saturating_add(1);
        }
        ring.samples.push_back(sample);
    }
    pub(super) fn snapshot(&self) -> serde_json::Value {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({"capacity":SAMPLE_CAPACITY,"dropped":ring.dropped,"samples":ring.samples})
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Metrics {
    pub invocation_ns: u64,
    pub guest_call_ns: u64,
    pub guest_callback_ns: u64,
    pub resource_wait_ns: u64,
    pub resource_reads: u64,
}
impl Metrics {
    fn valid(self) -> bool {
        self.guest_callback_ns <= self.guest_call_ns
            && self.guest_call_ns <= self.invocation_ns
            && self.guest_callback_ns <= self.resource_wait_ns
            && self.resource_wait_ns <= self.invocation_ns
    }
    pub(super) fn encode(self, sequence: u64, slot: u32) -> [u8; WIRE_SIZE] {
        let mut bytes = [0; WIRE_SIZE];
        bytes[..8].copy_from_slice(&sequence.to_le_bytes());
        bytes[8..12].copy_from_slice(&slot.to_le_bytes());
        for (chunk, value) in bytes[12..].chunks_exact_mut(8).zip([
            self.invocation_ns,
            self.guest_call_ns,
            self.guest_callback_ns,
            self.resource_wait_ns,
            self.resource_reads,
        ]) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
    pub(super) fn decode(bytes: &[u8], sequence: u64, slot: u32) -> Option<Self> {
        if bytes.len() != WIRE_SIZE
            || u64::from_le_bytes(bytes[..8].try_into().ok()?) != sequence
            || u32::from_le_bytes(bytes[8..12].try_into().ok()?) != slot
        {
            return None;
        }
        let mut values = bytes[12..].chunks_exact(8);
        let mut next = || Some(u64::from_le_bytes(values.next()?.try_into().ok()?));
        let metrics = Self {
            invocation_ns: next()?,
            guest_call_ns: next()?,
            guest_callback_ns: next()?,
            resource_wait_ns: next()?,
            resource_reads: next()?,
        };
        metrics.valid().then_some(metrics)
    }
}

pub(super) struct Capture {
    started: Instant,
    guest_started: Cell<Option<Instant>>,
    guest_ns: Cell<Option<u64>>,
    guest_callback_ns: Cell<u64>,
    resource_ns: Cell<u64>,
    reads: Cell<u64>,
    valid: Cell<bool>,
}
impl Capture {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            guest_started: Cell::new(None),
            guest_ns: Cell::new(None),
            guest_callback_ns: Cell::new(0),
            resource_ns: Cell::new(0),
            reads: Cell::new(0),
            valid: Cell::new(true),
        }
    }
    pub(super) fn guest_span(&self) -> GuestSpan<'_> {
        let active = self.guest_started.get().is_none() && self.guest_ns.get().is_none();
        if active {
            self.guest_started.set(Some(Instant::now()));
        } else {
            self.valid.set(false);
        }
        GuestSpan {
            capture: self,
            active,
        }
    }
    fn add(&self, counter: &Cell<u64>, value: u64) {
        if let Some(total) = counter.get().checked_add(value) {
            counter.set(total);
        } else {
            self.valid.set(false);
        }
    }
    pub(super) fn finish(&self) -> Option<Metrics> {
        if !self.valid.get() || self.guest_started.get().is_some() {
            return None;
        }
        let metrics = Metrics {
            invocation_ns: u64::try_from(self.started.elapsed().as_nanos()).ok()?,
            guest_call_ns: self.guest_ns.get()?,
            guest_callback_ns: self.guest_callback_ns.get(),
            resource_wait_ns: self.resource_ns.get(),
            resource_reads: self.reads.get(),
        };
        metrics.valid().then_some(metrics)
    }
}
pub(super) struct GuestSpan<'a> {
    capture: &'a Capture,
    active: bool,
}
impl Drop for GuestSpan<'_> {
    fn drop(&mut self) {
        if self.active {
            let elapsed = self
                .capture
                .guest_started
                .take()
                .and_then(|start| u64::try_from(start.elapsed().as_nanos()).ok());
            self.capture.guest_ns.set(elapsed);
            if elapsed.is_none() {
                self.capture.valid.set(false);
            }
        }
    }
}
pub(super) struct TimedReader<'a, 'b> {
    pub reader: &'a mut dyn ResourceReader,
    pub capture: &'b Capture,
}
impl ResourceReader for TimedReader<'_, '_> {
    fn read_resource(&mut self, resource: u32, offset: u64, length: u32) -> io::Result<Vec<u8>> {
        let inside_guest = self.capture.guest_started.get().is_some();
        let started = Instant::now();
        let result = self.reader.read_resource(resource, offset, length);
        if let Ok(elapsed) = u64::try_from(started.elapsed().as_nanos()) {
            self.capture.add(&self.capture.resource_ns, elapsed);
            self.capture.add(&self.capture.reads, 1);
            if inside_guest {
                self.capture.add(&self.capture.guest_callback_ns, elapsed);
            }
        } else {
            self.capture.valid.set(false);
        }
        result
    }
    fn emit_artifact(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.reader.emit_artifact(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_is_exact_length_call_bound_and_consistent() {
        let metrics = Metrics {
            invocation_ns: 100,
            guest_call_ns: 80,
            guest_callback_ns: 30,
            resource_wait_ns: 40,
            resource_reads: 2,
        };
        let bytes = metrics.encode(7, 1);
        assert_eq!(Metrics::decode(&bytes, 7, 1), Some(metrics));
        assert_eq!(Metrics::decode(&bytes, 8, 1), None);
        assert_eq!(Metrics::decode(&bytes, 7, 2), None);
        assert_eq!(Metrics::decode(&bytes[..WIRE_SIZE - 1], 7, 1), None);
        let mut extended = bytes.to_vec();
        extended.push(0);
        assert_eq!(Metrics::decode(&extended, 7, 1), None);
        for invalid in [
            Metrics {
                guest_callback_ns: 81,
                ..metrics
            },
            Metrics {
                invocation_ns: 79,
                ..metrics
            },
            Metrics {
                resource_wait_ns: 29,
                ..metrics
            },
            Metrics {
                resource_wait_ns: 101,
                ..metrics
            },
        ] {
            assert_eq!(Metrics::decode(&invalid.encode(7, 1), 7, 1), None);
        }
    }
    #[test]
    fn absent_incomplete_nested_or_overflowed_measurement_is_not_zero() {
        let capture = Capture::new();
        assert!(capture.finish().is_none());
        let span = capture.guest_span();
        assert!(capture.finish().is_none());
        drop(span);
        assert!(capture.finish().is_some());
        let duplicate = capture.guest_span();
        drop(duplicate);
        assert!(capture.finish().is_none());
        let capture = Capture::new();
        let outer = capture.guest_span();
        let nested = capture.guest_span();
        drop(nested);
        drop(outer);
        assert!(capture.finish().is_none());
        let capture = Capture::new();
        drop(capture.guest_span());
        capture.add(&capture.reads, u64::MAX);
        capture.add(&capture.reads, 1);
        assert!(capture.finish().is_none());
    }
}
