//! Private x86-64 clock/sequence reservation; selected only after CPU detection.
use super::{MAX_EVENTS, Stamp};
use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

const CAPTURE_ATTEMPTS: usize = 8;

#[repr(align(16))]
struct PairCell(UnsafeCell<u128>);

// SAFETY: construction verifies the CPU feature;16-byte aligned storage is
// accessed only by cmpxchg16b once shared. Every access borrows its live owner.
unsafe impl Sync for PairCell {}

impl PairCell {
    fn compare_exchange(&self, old: u128, new: u128) -> u128 {
        // SAFETY: the constructor's CPU check, repr alignment, atomic-only
        // accesses and live shared borrow meet the intrinsic's requirements.
        unsafe {
            std::arch::x86_64::cmpxchg16b(
                self.0.get(),
                old,
                new,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
        }
    }
    fn load(&self) -> u128 {
        self.compare_exchange(0, 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Failure {
    Closed,
    Full,
    Exhausted,
    Contended,
    SealContended,
}

pub(super) struct NativeClock {
    cell: PairCell,
    closed: AtomicBool,
}

fn pack(sequence: u64, at_us: u64) -> u128 {
    (u128::from(at_us) << 64) | u128::from(sequence)
}
fn unpack(value: u128) -> Stamp {
    Stamp {
        sequence: value as u64,
        at_us: (value >> 64) as u64,
    }
}

impl NativeClock {
    pub(super) fn new() -> Option<Self> {
        std::is_x86_feature_detected!("cmpxchg16b").then(|| Self {
            cell: PairCell(UnsafeCell::new(0)),
            closed: AtomicBool::new(false),
        })
    }

    pub(super) fn reserve(&self, observed_us: u64, visible: &AtomicU64) -> Result<Stamp, Failure> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Failure::Closed);
        }
        let mut old = self.cell.load();
        for _ in 0..CAPTURE_ATTEMPTS {
            if self.closed.load(Ordering::Acquire) {
                return Err(Failure::Closed);
            }
            let before = unpack(old);
            if before
                .sequence
                .saturating_sub(visible.load(Ordering::Acquire))
                >= MAX_EVENTS as u64
            {
                return Err(Failure::Full);
            }
            let sequence = before.sequence.checked_add(1).ok_or(Failure::Exhausted)?;
            // Both choices are real trusted-clock observations inside the
            // overlapping producer capture interval. No future time is invented
            // and no timestamp is adjusted later at the recorder/consumer.
            let at_us = before.at_us.max(observed_us);
            let actual = self.cell.compare_exchange(old, pack(sequence, at_us));
            if actual == old {
                return Ok(Stamp { sequence, at_us });
            }
            old = actual;
        }
        Err(Failure::Contended)
    }

    pub(super) fn last_sequence(&self) -> u64 {
        unpack(self.cell.load()).sequence
    }

    pub(super) fn seal(&self) -> Result<u64, Failure> {
        self.closed.store(true, Ordering::Release);
        let mut old = self.cell.load();
        // Stop already scans at most MAX_EVENTS completion tags. Its same finite
        // bound fences pre-close producers; this is not a producer-side retry.
        for _ in 0..=MAX_EVENTS {
            // Change the expected word without reserving an event or sacrificing
            // a valid64-bit clock/sequence sentinel. The fence word's clock is
            // never consumed after close. A stale producer cannot CAS past it.
            let actual = self.cell.compare_exchange(old, old ^ (1u128 << 127));
            if actual == old {
                return Ok(unpack(old).sequence);
            }
            old = actual;
        }
        Err(Failure::SealContended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAX_PENDING: u64 = MAX_EVENTS as u64;
    struct Counter(NativeClock);
    impl std::ops::Deref for Counter {
        type Target = NativeClock;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl Counter {
        fn new() -> Self {
            Self(NativeClock::new().expect("checked CPU support"))
        }
        fn initial(sequence: u64, at_us: u64) -> Self {
            let mut counter = Self::new();
            counter.0.cell = PairCell(UnsafeCell::new(pack(sequence, at_us)));
            counter
        }
        fn reserve(&self, at_us: u64, visible: u64) -> Result<Stamp, Failure> {
            self.0.reserve(at_us, &AtomicU64::new(visible))
        }
    }
    #[test]
    fn aligned_storage_and_serial_source_observations() {
        if NativeClock::new().is_none() {
            return;
        }
        let c = Counter::new();
        assert_eq!(c.cell.0.get() as usize % 16, 0);
        assert_eq!(
            c.reserve(7, 0),
            Ok(Stamp {
                sequence: 1,
                at_us: 7
            })
        );
        assert_eq!(
            c.reserve(9, 0),
            Ok(Stamp {
                sequence: 2,
                at_us: 9
            })
        );
    }
    #[test]
    fn delayed_capture_does_not_own_a_probe_or_block_other_producers() {
        if NativeClock::new().is_none() {
            return;
        }
        use std::sync::{Arc, mpsc};
        let counter = Arc::new(Counter::new());
        let (sampled, ready) = mpsc::channel();
        let (resume, release) = mpsc::channel();
        let c = counter.clone();
        let delayed = std::thread::spawn(move || {
            let own_clock_observation = 7;
            sampled.send(()).unwrap();
            release.recv().unwrap();
            c.reserve(own_clock_observation, 0)
        });
        ready.recv().unwrap();
        assert_eq!(
            counter.reserve(20, 0),
            Ok(Stamp {
                sequence: 1,
                at_us: 20
            })
        );
        resume.send(()).unwrap();
        assert_eq!(
            delayed.join().unwrap(),
            Ok(Stamp {
                sequence: 2,
                at_us: 20
            })
        );
    }
    #[test]
    fn pending_capacity_is_unchanged_and_completed_prefix_releases_capacity() {
        if NativeClock::new().is_none() {
            return;
        }
        let c = Counter::new();
        for n in 1..=MAX_PENDING {
            assert_eq!(c.reserve(n, 0).unwrap().sequence, n)
        }
        assert_eq!(c.reserve(MAX_PENDING + 1, 0), Err(Failure::Full));
        assert_eq!(
            c.reserve(MAX_PENDING + 1, MAX_PENDING).unwrap().sequence,
            MAX_PENDING + 1
        );
    }
    #[test]
    fn full_u64_clock_and_sequence_do_not_use_a_data_value_as_closed_sentinel() {
        if NativeClock::new().is_none() {
            return;
        }
        let c = Counter::initial(u64::MAX - 1, u64::MAX);
        assert_eq!(
            c.reserve(u64::MAX, u64::MAX - 1),
            Ok(Stamp {
                sequence: u64::MAX,
                at_us: u64::MAX
            })
        );
        assert_eq!(c.reserve(u64::MAX, u64::MAX), Err(Failure::Exhausted));
        assert_eq!(c.seal(), Ok(u64::MAX));
        assert_eq!(c.reserve(0, u64::MAX), Err(Failure::Closed));
    }
    #[test]
    fn seal_fences_an_old_expected_word_without_issuing_a_fake_event() {
        if NativeClock::new().is_none() {
            return;
        }
        let c = Counter::new();
        let before = c.cell.load();
        assert_eq!(c.seal(), Ok(0));
        assert_ne!(c.cell.compare_exchange(before, pack(1, 10)), before);
        assert_eq!(unpack(c.cell.load()).sequence, 0);
        assert_eq!(c.reserve(20, 0), Err(Failure::Closed));
    }
    #[test]
    fn concurrent_successes_remain_contiguous_and_monotonic_with_explicit_failures() {
        if NativeClock::new().is_none() {
            return;
        }
        use std::sync::{Arc, Barrier};
        use std::time::Instant;
        let counter = Arc::new(Counter::new());
        let barrier = Arc::new(Barrier::new(8));
        let origin = Instant::now();
        let producers: Vec<_> = (0..8)
            .map(|_| {
                let c = counter.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    let mut values = vec![];
                    let mut failures = 0;
                    let mut max_observed = 0;
                    for _ in 0..2000 {
                        let observed = u64::try_from(origin.elapsed().as_micros()).unwrap();
                        max_observed = max_observed.max(observed);
                        match c.reserve(observed, 0) {
                            Ok(stamp) => {
                                assert!(stamp.at_us >= observed);
                                values.push(stamp)
                            }
                            Err(Failure::Contended) => failures += 1,
                            other => panic!("unexpected finite-window result: {other:?}"),
                        }
                    }
                    (values, failures, max_observed)
                })
            })
            .collect();
        let mut all = vec![];
        let mut failed = 0;
        let mut max_observed = 0;
        for p in producers {
            let (mut values, n, maximum) = p.join().unwrap();
            all.append(&mut values);
            failed += n;
            max_observed = max_observed.max(maximum)
        }
        all.sort_by_key(|s| s.sequence);
        assert_eq!(all.len() + failed, 16000);
        for (i, s) in all.iter().enumerate() {
            assert_eq!(s.sequence, i as u64 + 1);
            assert!(s.at_us <= max_observed)
        }
        assert!(all.windows(2).all(|p| p[0].at_us <= p[1].at_us));
        assert_eq!(counter.seal(), Ok(all.len() as u64));
        println!(
            "nativePair attempts=16000 successful={} explicitContentionFailures={failed}",
            all.len()
        );
    }
}
