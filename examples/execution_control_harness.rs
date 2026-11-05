//! Independent virtual-time experiment; not Engine or a proposed public Node API.
//! See docs/execution-control-harness.md for assumptions and responsibility boundaries.
use std::collections::{BTreeMap, VecDeque};

type Tick = u64;
type JobId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Generation {
    session: u64,
    plan: u64,
    run: u64,
    input: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Path {
    Fast,
    Slow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Candidate {
    path: Path,
    text: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    Completion,
    Deadline,
}

// Selection meaning belongs to replaceable configuration, outside the scheduler.
trait SelectionPolicy {
    fn choose(&self, trigger: Trigger, available: &[Candidate]) -> Option<Candidate>;
}

struct FirstAvailable;
impl SelectionPolicy for FirstAvailable {
    fn choose(&self, _: Trigger, available: &[Candidate]) -> Option<Candidate> {
        available.first().copied()
    }
}

struct PreferSlowUntilDeadline;
impl SelectionPolicy for PreferSlowUntilDeadline {
    fn choose(&self, trigger: Trigger, available: &[Candidate]) -> Option<Candidate> {
        available
            .iter()
            .find(|candidate| candidate.path == Path::Slow)
            .copied()
            .or_else(|| {
                if trigger == Trigger::Deadline {
                    available.first().copied()
                } else {
                    None
                }
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Selected(Candidate),
    DeadlineWithoutResult,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkState {
    Running,
    CancelRequested { reclaim_by: Tick },
    Stalled,
}

#[derive(Clone, Copy, Debug)]
struct Work {
    generation: Generation,
    path: Path,
    state: WorkState,
}

#[derive(Clone, Copy, Debug)]
struct Queued {
    id: JobId,
    work: Work,
    expires: Tick,
}

#[derive(Clone, Copy, Debug)]
struct Limits {
    slots: usize,
    queue: usize,
    queue_wait: Tick,
    cancel_grace: Tick,
    trace: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notice {
    Started(JobId),
    Queued(JobId),
    Rejected,
    QueueExpired(JobId),
    QueueDropped(JobId),
    Waiting,
    Deadline,
    Decision(Decision),
    CancelRequested(JobId),
    Stalled(JobId),
    Reclaimed(JobId),
    Ignored(JobId),
    Available(JobId),
    GenerationChanged,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Submit(Path),
    Observe,
    Complete {
        id: JobId,
        generation: Generation,
        text: &'static str,
    },
    CancelAck {
        id: JobId,
        generation: Generation,
    },
    Cancel,
    Replace {
        generation: Generation,
        budget: Tick,
    },
}

struct Harness<P> {
    policy: P,
    limits: Limits,
    now: Tick,
    generation: Generation,
    deadline: Tick,
    next_id: JobId,
    active: BTreeMap<JobId, Work>,
    queue: VecDeque<Queued>,
    // At most one candidate per path; not an unbounded completion history.
    available: Vec<Candidate>,
    decision: Option<Decision>,
    trace: VecDeque<(Tick, Notice)>,
}

impl<P: SelectionPolicy> Harness<P> {
    fn new(policy: P, generation: Generation, budget: Tick, limits: Limits) -> Self {
        assert!(budget > 0 && limits.slots > 0 && limits.queue_wait > 0);
        assert!(limits.cancel_grace > 0 && limits.trace > 0);
        Self {
            policy,
            limits,
            now: 0,
            generation,
            deadline: budget,
            next_id: 1,
            active: BTreeMap::new(),
            queue: VecDeque::new(),
            available: Vec::new(),
            decision: None,
            trace: VecDeque::new(),
        }
    }

    fn note(&mut self, notice: Notice) {
        if self.trace.len() == self.limits.trace {
            self.trace.pop_front();
        }
        self.trace.push_back((self.now, notice));
    }

    // Only the script/host advances time. Timers due at `at` precede its action.
    // Process intermediate timer instants too, so jumping over them preserves order.
    fn advance(&mut self, at: Tick) -> Result<(), &'static str> {
        if at < self.now {
            return Err("clock-backwards");
        }
        loop {
            let next = self
                .queue
                .iter()
                .map(|queued| queued.expires)
                .chain(self.decision.is_none().then_some(self.deadline))
                .chain(self.active.values().filter_map(|work| match work.state {
                    WorkState::CancelRequested { reclaim_by } => Some(reclaim_by),
                    _ => None,
                }))
                .filter(|tick| *tick <= at)
                .min();
            let Some(next) = next else { break };
            self.now = next;
            // Equal-time timers: queue expiry, run deadline, cancellation grace.
            while self
                .queue
                .front()
                .is_some_and(|queued| queued.expires <= next)
            {
                let queued = self.queue.pop_front().unwrap();
                self.note(Notice::QueueExpired(queued.id));
            }
            if self.decision.is_none() && self.deadline <= next {
                self.note(Notice::Deadline);
                let decision = self
                    .policy
                    .choose(Trigger::Deadline, &self.available)
                    .map(Decision::Selected)
                    .unwrap_or(Decision::DeadlineWithoutResult);
                self.finish(decision);
            }
            let stalled: Vec<_> = self.active.iter().filter_map(|(&id, work)| {
                matches!(work.state, WorkState::CancelRequested { reclaim_by } if reclaim_by <= next)
                    .then_some(id)
            }).collect();
            for id in stalled {
                self.active.get_mut(&id).unwrap().state = WorkState::Stalled;
                self.note(Notice::Stalled(id));
            }
        }
        self.now = at;
        Ok(())
    }

    fn cancel_work(&mut self) {
        while let Some(queued) = self.queue.pop_front() {
            self.note(Notice::QueueDropped(queued.id));
        }
        let ids: Vec<_> = self
            .active
            .iter()
            .filter_map(|(&id, work)| (work.state == WorkState::Running).then_some(id))
            .collect();
        for id in ids {
            // Saturation is safe: grace is reached no later than the final tick.
            self.active.get_mut(&id).unwrap().state = WorkState::CancelRequested {
                reclaim_by: self.now.saturating_add(self.limits.cancel_grace),
            };
            self.note(Notice::CancelRequested(id));
        }
    }

    fn finish(&mut self, decision: Decision) {
        self.decision = Some(decision);
        self.note(Notice::Decision(decision));
        self.cancel_work();
    }

    fn start_queued(&mut self) {
        while self.active.len() < self.limits.slots {
            let Some(queued) = self.queue.pop_front() else {
                break;
            };
            self.active.insert(queued.id, queued.work);
            self.note(Notice::Started(queued.id));
        }
    }

    fn submit(&mut self, path: Path) -> Result<Option<JobId>, &'static str> {
        if self.decision.is_some()
            || (self.active.len() == self.limits.slots && self.queue.len() == self.limits.queue)
        {
            self.note(Notice::Rejected);
            return Ok(None);
        }
        let next = self.next_id.checked_add(1).ok_or("job-id-exhausted")?;
        let expires = self
            .now
            .checked_add(self.limits.queue_wait)
            .ok_or("time-overflow")?;
        let id = self.next_id;
        self.next_id = next;
        let work = Work {
            generation: self.generation,
            path,
            state: WorkState::Running,
        };
        if self.active.len() < self.limits.slots {
            self.active.insert(id, work);
            self.note(Notice::Started(id));
        } else {
            self.queue.push_back(Queued { id, work, expires });
            self.note(Notice::Queued(id));
        }
        Ok(Some(id))
    }

    fn complete(&mut self, id: JobId, generation: Generation, text: Option<&'static str>) {
        let Some(work) = self.active.get(&id).copied() else {
            self.note(Notice::Ignored(id));
            return;
        };
        // A mismatched token cannot reclaim another job's resources.
        if work.generation != generation || (text.is_none() && work.state == WorkState::Running) {
            self.note(Notice::Ignored(id));
            return;
        }
        self.active.remove(&id);
        self.note(Notice::Reclaimed(id));
        if let Some(text) = text {
            if generation == self.generation
                && work.state == WorkState::Running
                && self.decision.is_none()
            {
                let candidate = Candidate {
                    path: work.path,
                    text,
                };
                self.available.retain(|old| old.path != candidate.path);
                self.available.push(candidate);
                self.note(Notice::Available(id));
                if let Some(selected) = self.policy.choose(Trigger::Completion, &self.available) {
                    self.finish(Decision::Selected(selected));
                }
            } else {
                self.note(Notice::Ignored(id));
            }
        }
        // Completion/ack may reclaim an obsolete job but never select its result.
        self.start_queued();
    }

    fn event(&mut self, at: Tick, action: Action) -> Result<Option<JobId>, &'static str> {
        // Validate a replacement before applying timers or invalidating the old run.
        let replacement = match action {
            Action::Replace { generation, budget } => {
                // A host must issue fresh, monotone generation tokens (no ABA reuse).
                let old = self.generation;
                if budget == 0
                    || generation == old
                    || generation.session < old.session
                    || (generation.session == old.session
                        && (generation.plan < old.plan
                            || generation.run < old.run
                            || generation.input < old.input))
                {
                    return Err("invalid-generation-or-budget");
                }
                Some(at.checked_add(budget).ok_or("time-overflow")?)
            }
            _ => None,
        };
        self.advance(at)?;
        match action {
            Action::Submit(path) => return self.submit(path),
            Action::Observe => {
                if self.decision.is_none() {
                    self.note(Notice::Waiting);
                }
            }
            Action::Complete {
                id,
                generation,
                text,
            } => self.complete(id, generation, Some(text)),
            Action::CancelAck { id, generation } => self.complete(id, generation, None),
            Action::Cancel => {
                if self.decision.is_none() {
                    self.finish(Decision::Cancelled);
                }
            }
            Action::Replace { generation, .. } => {
                self.cancel_work();
                self.generation = generation;
                self.deadline = replacement.unwrap();
                self.available.clear();
                self.decision = None;
                self.note(Notice::GenerationChanged);
            }
        }
        Ok(None)
    }
}

const GENERATION: Generation = Generation {
    session: 1,
    plan: 1,
    run: 1,
    input: 1,
};
const LIMITS: Limits = Limits {
    slots: 2,
    queue: 2,
    queue_wait: 8,
    cancel_grace: 3,
    trace: 128,
};

fn main() {
    let mut harness = Harness::new(PreferSlowUntilDeadline, GENERATION, 10, LIMITS);
    // IDs follow accepted submissions, not wall time or threads.
    for (at, action) in [
        (0, Action::Submit(Path::Fast)),
        (0, Action::Submit(Path::Slow)),
        (
            2,
            Action::Complete {
                id: 1,
                generation: GENERATION,
                text: "fast candidate",
            },
        ),
        (4, Action::Observe),
        (
            10,
            Action::Complete {
                id: 2,
                generation: GENERATION,
                text: "slow at deadline",
            },
        ),
    ] {
        harness.event(at, action).unwrap();
    }
    println!("prefer slow until deadline:");
    for (at, notice) in &harness.trace {
        println!("{at:>2}: {notice:?}");
    }

    // Exercise the alternative policy and explicit cancellation/generation boundary.
    let mut first = Harness::new(FirstAvailable, GENERATION, 10, LIMITS);
    first.event(0, Action::Submit(Path::Fast)).unwrap();
    first
        .event(
            2,
            Action::Complete {
                id: 1,
                generation: GENERATION,
                text: "first",
            },
        )
        .unwrap();
    first
        .event(
            3,
            Action::Replace {
                generation: Generation {
                    run: 2,
                    input: 2,
                    ..GENERATION
                },
                budget: 10,
            },
        )
        .unwrap();
    let id = first.event(3, Action::Submit(Path::Slow)).unwrap().unwrap();
    first.event(4, Action::Cancel).unwrap();
    first
        .event(
            5,
            Action::CancelAck {
                id,
                generation: first.generation,
            },
        )
        .unwrap();
    println!(
        "first available, then a new cancelled run: {:?}",
        first.decision
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(id: JobId, generation: Generation, text: &'static str) -> Action {
        Action::Complete {
            id,
            generation,
            text,
        }
    }

    fn selected(path: Path, text: &'static str) -> Option<Decision> {
        Some(Decision::Selected(Candidate { path, text }))
    }

    fn pair<P: SelectionPolicy>(policy: P) -> Harness<P> {
        let mut harness = Harness::new(policy, GENERATION, 10, LIMITS);
        assert_eq!(harness.event(0, Action::Submit(Path::Fast)), Ok(Some(1)));
        assert_eq!(harness.event(0, Action::Submit(Path::Slow)), Ok(Some(2)));
        harness
    }

    #[test]
    fn fixed_script_replays_exact_trace_with_deadline_before_completion() {
        fn replay() -> Harness<PreferSlowUntilDeadline> {
            let mut harness = pair(PreferSlowUntilDeadline);
            for (at, action) in [
                (2, complete(1, GENERATION, "fast")),
                (4, Action::Observe),
                (10, complete(2, GENERATION, "late")),
            ] {
                harness.event(at, action).unwrap();
            }
            harness
        }
        let harness = replay();
        let decision = Decision::Selected(Candidate {
            path: Path::Fast,
            text: "fast",
        });
        assert_eq!(
            harness.trace,
            VecDeque::from([
                (0, Notice::Started(1)),
                (0, Notice::Started(2)),
                (2, Notice::Reclaimed(1)),
                (2, Notice::Available(1)),
                (4, Notice::Waiting),
                (10, Notice::Deadline),
                (10, Notice::Decision(decision)),
                (10, Notice::CancelRequested(2)),
                (10, Notice::Reclaimed(2)),
                (10, Notice::Ignored(2)),
            ])
        );
        assert_eq!(harness.trace, replay().trace);
        assert_eq!(harness.decision, Some(decision));
        assert!(harness.active.is_empty());
    }

    #[test]
    fn changing_policy_changes_when_and_which_result_is_selected() {
        let mut first = pair(FirstAvailable);
        let mut slow = pair(PreferSlowUntilDeadline);
        first.event(2, complete(1, GENERATION, "fast")).unwrap();
        slow.event(2, complete(1, GENERATION, "fast")).unwrap();
        assert_eq!(first.decision, selected(Path::Fast, "fast"));
        assert_eq!(slow.decision, None);
        slow.event(8, Action::Observe).unwrap();
        assert_eq!(slow.decision, None);
        slow.event(9, complete(2, GENERATION, "slow")).unwrap();
        first.event(9, complete(2, GENERATION, "slow")).unwrap();
        assert_eq!(slow.decision, selected(Path::Slow, "slow"));
        assert_eq!(first.decision, selected(Path::Fast, "fast"));
    }

    #[test]
    fn simultaneous_completions_follow_explicit_script_order() {
        for (one, two, path) in [(1, 2, Path::Fast), (2, 1, Path::Slow)] {
            let mut harness = pair(FirstAvailable);
            harness
                .event(2, complete(one, GENERATION, "winner"))
                .unwrap();
            harness
                .event(2, complete(two, GENERATION, "loser"))
                .unwrap();
            assert_eq!(harness.decision, selected(path, "winner"));
            assert!(harness.active.is_empty());
        }
    }

    #[test]
    fn deadline_without_result_and_without_any_jobs_is_terminal() {
        for mut harness in [
            pair(FirstAvailable),
            Harness::new(FirstAvailable, GENERATION, 10, LIMITS),
        ] {
            harness.event(10, Action::Observe).unwrap();
            assert_eq!(harness.decision, Some(Decision::DeadlineWithoutResult));
            assert_eq!(harness.event(10, Action::Submit(Path::Fast)), Ok(None));
            harness.event(20, Action::Observe).unwrap();
            assert_eq!(
                harness
                    .trace
                    .iter()
                    .filter(|(_, n)| *n == Notice::Deadline)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn cancel_request_does_not_reclaim_but_ack_and_late_completion_do() {
        let mut harness = pair(PreferSlowUntilDeadline);
        harness.event(1, Action::Cancel).unwrap();
        assert_eq!(harness.decision, Some(Decision::Cancelled));
        assert_eq!(harness.active.len(), 2);
        harness
            .event(
                2,
                Action::CancelAck {
                    id: 1,
                    generation: GENERATION,
                },
            )
            .unwrap();
        assert_eq!(harness.active.len(), 1);
        harness.event(3, complete(2, GENERATION, "late")).unwrap();
        assert!(harness.active.is_empty());
        assert!(harness.available.is_empty());
        assert_eq!(harness.decision, Some(Decision::Cancelled));
        harness
            .event(3, complete(2, GENERATION, "duplicate"))
            .unwrap();
        harness
            .event(
                3,
                Action::CancelAck {
                    id: 1,
                    generation: GENERATION,
                },
            )
            .unwrap();
        assert_eq!(harness.decision, Some(Decision::Cancelled));
    }

    #[test]
    fn unsolicited_ack_cannot_free_running_work() {
        let mut harness = pair(FirstAvailable);
        harness
            .event(
                1,
                Action::CancelAck {
                    id: 1,
                    generation: GENERATION,
                },
            )
            .unwrap();
        assert_eq!(harness.active.len(), 2);
        harness.event(2, complete(1, GENERATION, "valid")).unwrap();
        assert_eq!(harness.decision, selected(Path::Fast, "valid"));
    }

    fn changed_generations() -> [Generation; 4] {
        [
            Generation {
                session: 2,
                ..GENERATION
            },
            Generation {
                plan: 2,
                ..GENERATION
            },
            Generation {
                run: 2,
                ..GENERATION
            },
            Generation {
                input: 2,
                ..GENERATION
            },
        ]
    }

    #[test]
    fn all_four_generation_fields_guard_completion_and_ack() {
        for wrong in changed_generations() {
            let mut harness = pair(FirstAvailable);
            harness.event(1, complete(1, wrong, "wrong")).unwrap();
            assert!(harness.available.is_empty());
            assert_eq!(harness.active.len(), 2);
            harness.event(2, Action::Cancel).unwrap();
            harness
                .event(
                    3,
                    Action::CancelAck {
                        id: 1,
                        generation: wrong,
                    },
                )
                .unwrap();
            assert_eq!(harness.active.len(), 2);
            harness
                .event(
                    3,
                    Action::CancelAck {
                        id: 1,
                        generation: GENERATION,
                    },
                )
                .unwrap();
            assert_eq!(harness.active.len(), 1);
        }
    }

    #[test]
    fn replacing_any_generation_clears_candidates_and_rejects_old_results() {
        for generation in changed_generations() {
            let mut harness = pair(PreferSlowUntilDeadline);
            harness
                .event(1, complete(1, GENERATION, "old fast"))
                .unwrap();
            harness
                .event(
                    2,
                    Action::Replace {
                        generation,
                        budget: 10,
                    },
                )
                .unwrap();
            assert!(harness.available.is_empty());
            assert_eq!(harness.deadline, 12);
            harness
                .event(3, complete(2, GENERATION, "old slow"))
                .unwrap();
            assert!(harness.active.is_empty());
            assert_eq!(harness.decision, None);
            let id = harness
                .event(3, Action::Submit(Path::Slow))
                .unwrap()
                .unwrap();
            harness
                .event(4, complete(id, generation, "current"))
                .unwrap();
            assert_eq!(harness.decision, selected(Path::Slow, "current"));
        }
    }

    #[test]
    fn queue_is_bounded_fifo_and_expired_work_never_starts() {
        let limits = Limits {
            slots: 1,
            queue: 2,
            ..LIMITS
        };
        let mut harness = Harness::new(PreferSlowUntilDeadline, GENERATION, 30, limits);
        assert_eq!(harness.event(0, Action::Submit(Path::Fast)), Ok(Some(1)));
        assert_eq!(harness.event(1, Action::Submit(Path::Fast)), Ok(Some(2)));
        assert_eq!(harness.event(2, Action::Submit(Path::Slow)), Ok(Some(3)));
        assert_eq!(harness.event(3, Action::Submit(Path::Slow)), Ok(None));
        harness.event(4, complete(1, GENERATION, "first")).unwrap();
        assert!(harness.active.contains_key(&2));
        assert_eq!(harness.queue.front().unwrap().id, 3);
        harness
            .event(10, complete(2, GENERATION, "second"))
            .unwrap();
        assert!(harness.active.is_empty());
        assert!(harness.queue.is_empty());
        assert!(harness.trace.contains(&(10, Notice::QueueExpired(3))));
        assert!(!harness.trace.iter().any(|(_, n)| *n == Notice::Started(3)));
    }

    #[test]
    fn never_finishing_job_retains_slot_and_new_run_has_bounded_admission() {
        let limits = Limits {
            slots: 1,
            queue: 1,
            ..LIMITS
        };
        let mut harness = Harness::new(FirstAvailable, GENERATION, 10, limits);
        harness.event(0, Action::Submit(Path::Slow)).unwrap();
        harness.event(20, Action::Observe).unwrap();
        assert_eq!(harness.active[&1].state, WorkState::Stalled);
        assert!(harness.trace.contains(&(10, Notice::Deadline)));
        assert!(harness.trace.contains(&(13, Notice::Stalled(1))));
        let generation = Generation {
            run: 2,
            input: 2,
            ..GENERATION
        };
        harness
            .event(
                20,
                Action::Replace {
                    generation,
                    budget: 30,
                },
            )
            .unwrap();
        assert_eq!(harness.event(20, Action::Submit(Path::Fast)), Ok(Some(2)));
        assert_eq!(harness.event(20, Action::Submit(Path::Slow)), Ok(None));
        harness.event(28, Action::Observe).unwrap();
        assert!(harness.queue.is_empty());
        assert_eq!(harness.active.len(), 1);
        assert_eq!(harness.event(29, Action::Submit(Path::Fast)), Ok(Some(3)));
        harness
            .event(
                30,
                Action::CancelAck {
                    id: 1,
                    generation: GENERATION,
                },
            )
            .unwrap();
        assert!(harness.active.contains_key(&3));
        harness.event(31, complete(3, generation, "new")).unwrap();
        assert_eq!(harness.decision, selected(Path::Fast, "new"));
    }

    #[test]
    fn terminal_selection_drops_queued_unselected_paths() {
        let limits = Limits { slots: 1, ..LIMITS };
        let mut harness = Harness::new(FirstAvailable, GENERATION, 10, limits);
        harness.event(0, Action::Submit(Path::Fast)).unwrap();
        harness.event(0, Action::Submit(Path::Slow)).unwrap();
        harness.event(1, complete(1, GENERATION, "chosen")).unwrap();
        assert!(harness.active.is_empty() && harness.queue.is_empty());
        assert!(harness.trace.contains(&(1, Notice::QueueDropped(2))));
    }

    #[test]
    fn clock_and_generation_reuse_errors_preserve_state() {
        let mut harness = pair(FirstAvailable);
        harness.event(3, Action::Observe).unwrap();
        let trace = harness.trace.clone();
        assert_eq!(harness.event(2, Action::Cancel), Err("clock-backwards"));
        assert_eq!(harness.trace, trace);
        for (generation, budget) in [(GENERATION, 10), (changed_generations()[2], 0)] {
            assert_eq!(
                harness.event(20, Action::Replace { generation, budget }),
                Err("invalid-generation-or-budget")
            );
            assert_eq!(harness.trace, trace);
        }
        let generation = Generation {
            run: 2,
            ..GENERATION
        };
        harness
            .event(
                4,
                Action::Replace {
                    generation,
                    budget: 10,
                },
            )
            .unwrap();
        assert_eq!(
            harness.event(
                5,
                Action::Replace {
                    generation: GENERATION,
                    budget: 10
                }
            ),
            Err("invalid-generation-or-budget")
        );
        assert_eq!(
            harness.event(
                u64::MAX,
                Action::Replace {
                    generation: Generation {
                        run: 3,
                        ..GENERATION
                    },
                    budget: 1,
                }
            ),
            Err("time-overflow")
        );
        assert_eq!(harness.now, 4);
    }

    #[test]
    fn repeated_completions_keep_candidates_and_trace_bounded() {
        let limits = Limits { trace: 8, ..LIMITS };
        let mut harness = Harness::new(PreferSlowUntilDeadline, GENERATION, 100, limits);
        for at in 0..50 {
            let id = harness
                .event(at, Action::Submit(Path::Fast))
                .unwrap()
                .unwrap();
            harness.event(at, complete(id, GENERATION, "fast")).unwrap();
            assert!(harness.available.len() <= 2);
            assert!(harness.trace.len() <= limits.trace);
        }
        harness.event(100, Action::Observe).unwrap();
        assert_eq!(harness.decision, selected(Path::Fast, "fast"));
    }

    #[test]
    fn jumping_clock_preserves_intermediate_timer_order() {
        let limits = Limits { slots: 1, ..LIMITS };
        let mut jumped = Harness::new(FirstAvailable, GENERATION, 10, limits);
        let mut stepped = Harness::new(FirstAvailable, GENERATION, 10, limits);
        for harness in [&mut jumped, &mut stepped] {
            harness.event(0, Action::Submit(Path::Slow)).unwrap();
            harness.event(0, Action::Submit(Path::Fast)).unwrap();
        }
        jumped.advance(20).unwrap();
        for at in [8, 10, 13, 20] {
            stepped.advance(at).unwrap();
        }
        assert_eq!(jumped.trace, stepped.trace);
        assert_eq!(
            jumped.trace,
            VecDeque::from([
                (0, Notice::Started(1)),
                (0, Notice::Queued(2)),
                (8, Notice::QueueExpired(2)),
                (10, Notice::Deadline),
                (10, Notice::Decision(Decision::DeadlineWithoutResult)),
                (10, Notice::CancelRequested(1)),
                (13, Notice::Stalled(1)),
            ])
        );
        assert_eq!(jumped.active[&1].state, WorkState::Stalled);
    }
}
