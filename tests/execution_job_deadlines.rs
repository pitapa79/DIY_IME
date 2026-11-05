use diy_ime2_core::{
    Candidate, Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue, RecoveryClaim},
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlInput,
        ControlNode, DecimalId, DeferredNode, DispatchQueue, ExecutionHost, GraphExecution,
        GraphObservation, OwnedInvocation, ResourcePolicy, RunDelivery, RunLedger, SessionEpoch,
        StartRequest, TerminalReason, WakeReason,
    },
    graph::{ExecutionPlan, Graph},
    node::{CoreError, InputPort, NodeDefinition, Port, Registry, RunContext},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
struct Started {
    node: String,
    permit: CompletionPermit,
    cancel: CancellationToken,
}
struct Delay(mpsc::Sender<Started>);
impl DeferredNode for Delay {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.delay",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn start(
        &self,
        inv: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let node = inv.context().node_id.to_owned();
        self.0
            .send(Started {
                node: node.clone(),
                permit,
                cancel,
            })
            .unwrap();
        if node == "start-fail" {
            Err(CoreError::new("start-failed", "adapter failed"))
        } else {
            Ok(())
        }
    }
}
struct Recover;
impl ControlNode for Recover {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.recover",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec!["in".into()]
    }
    fn observe(
        &self,
        _: RunContext<'_>,
        frame: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        match &frame.inputs["in"] {
            ControlInput::Pending => Ok(ControlDecision::Wait {
                wake_after_ms: None,
            }),
            ControlInput::Resolved(ErrorAwareValue::Error(errors)) => {
                let mut out = data("fallback");
                out.recoveries.push(RecoveryClaim {
                    input: "in".into(),
                    output: "out".into(),
                    causes: errors
                        .causes()
                        .iter()
                        .map(|cause| cause.id.clone())
                        .collect(),
                });
                Ok(ControlDecision::Finish(out))
            }
            ControlInput::Resolved(value) => Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                [("out".into(), value.clone())].into(),
            ))),
        }
    }
}
fn data(value: &str) -> ErrorAwareExecution {
    ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: value.into(),
                meta: None,
            }))),
        )]
        .into(),
    )
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
struct Host {
    ledger: RunLedger,
    queue: DispatchQueue,
    graph: GraphExecution,
    observers:
        std::collections::BTreeMap<String, diy_ime2_core::execution_control::ControlObserver>,
    entered: mpsc::Receiver<Started>,
    host: ExecutionHost,
    initial: Vec<diy_ime2_core::execution_control::DispatchEvent>,
}
impl Host {
    fn new(
        names: &[&str],
        edges: &[(&str, &str)],
        target: &[&str],
        budgets: &[(&str, Option<u64>)],
        now: Duration,
        legacy: bool,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut policy = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["oneSlot"]
                .clone(),
        )
        .unwrap();
        policy.queue_wait_ms = None;
        let host = ExecutionHost::new(format!("deadline-test-{n}"), policy).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut registry = Registry::new();
        registry.register_deferred(Delay(tx)).unwrap();
        registry.register_control(Recover).unwrap();
        let graph=Graph::from_json(&json!({"version":1,"nodes":names.iter().map(|node|json!({"id":node,"kind":if *node=="recover"{"test.recover"}else{"test.delay"},"version":1})).collect::<Vec<_>>(),"edges":edges.iter().map(|(a,b)|json!({"from":{"node":a,"port":"out"},"to":{"node":b,"port":"in"}})).collect::<Vec<_>>()} ).to_string()).unwrap();
        let mut ledger = RunLedger::new(
            host.clone(),
            SessionEpoch::new(format!("{n:032x}")).unwrap(),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            id(1),
        );
        ledger.advance_clock(now).unwrap();
        let req:StartRequest=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":target.iter().map(|node|json!({"node":node,"port":"out"})).collect::<Vec<_>>(),"jobBudgets":budgets.iter().map(|(node,budget)|json!({"nodeId":node,"budgetMs":budget})).collect::<Vec<_>>()})).unwrap();
        let mut queue = DispatchQueue::new(host.clone()).unwrap();
        let (graph, observers, initial) = if legacy {
            let key = ledger.start_run(req).unwrap().key;
            let mut graph = ledger.graph_execution(&key).unwrap();
            let events = queue.schedule_ready(&mut graph, now).unwrap();
            (graph, Default::default(), events)
        } else {
            let admitted = queue.admit(&mut ledger, req).unwrap();
            (admitted.graph.unwrap(), admitted.observers, admitted.events)
        };
        ledger.mark_running(graph.key()).unwrap();
        Self {
            ledger,
            queue,
            graph,
            observers,
            entered: rx,
            host,
            initial,
        }
    }
    fn start(&self) -> Started {
        self.entered.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    fn advance(&mut self, now: Duration) {
        self.ledger.advance_clock(now).unwrap();
    }
    fn apply(&mut self, events: Vec<diy_ime2_core::execution_control::DispatchEvent>) {
        for event in events {
            let released = event.apply(&mut self.graph).unwrap();
            self.queue
                .cancel_released(self.graph.key(), &released, self.ledger.now())
                .unwrap();
        }
    }
    fn logical(&mut self) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let events = self.queue.poll();
            let yes = events
                .iter()
                .any(|e| e.completion().is_some_and(|e| e.execution().is_some()));
            self.apply(events);
            if yes {
                return;
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    fn drain(&mut self) {
        let until = Instant::now() + Duration::from_secs(5);
        while self.host.usage().physical_jobs > 0 {
            let events = self.queue.poll();
            assert!(
                events
                    .iter()
                    .all(|e| e.completion().unwrap().execution().is_none())
            );
            self.apply(events);
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    fn finish(&mut self) -> diy_ime2_core::execution_control::RunSnapshot {
        let key = self.graph.key().clone();
        self.ledger
            .update_physical(&key, self.queue.counts(&key))
            .unwrap();
        self.graph
            .finish(&mut self.ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
    }
}

#[test]
fn exact_job_deadline_resolves_error_but_retains_the_cancelled_physical_slot() {
    let mut h = Host::new(
        &["a"],
        &[],
        &["a"],
        &[("a", Some(10))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    assert_eq!(h.queue.next_job_deadline(), Some(ms(10)));
    assert!(
        h.queue
            .expire_jobs(Duration::from_nanos(9_999_999))
            .unwrap()
            .is_empty()
    );
    h.advance(ms(10));
    let events = h.queue.expire_jobs(ms(10)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].error().unwrap().code, "deadline-exceeded");
    h.apply(events);
    assert!(a.cancel.is_cancelled());
    assert_eq!(h.host.usage().physical_jobs, 1);
    assert!(h.queue.expire_jobs(ms(10)).unwrap().is_empty());
    let report = h.finish();
    assert_eq!(report.result.unwrap().delivery, RunDelivery::Failed);
    assert_eq!(report.physical.cancel_requested, 1);
    assert!(a.permit.complete(Ok(data("late"))));
    h.drain();
    assert_eq!(h.host.usage().physical_jobs, 0);
}

#[test]
fn a_producer_result_waiting_in_the_channel_cannot_backdate_completion_at_the_deadline() {
    let mut h = Host::new(
        &["a"],
        &[],
        &["a"],
        &[("a", Some(1))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    assert!(a.permit.complete(Ok(data("not adopted"))));
    h.advance(ms(1));
    let events = h.queue.expire_jobs(ms(1)).unwrap();
    h.apply(events);
    h.drain();
    let report = h.finish().result.unwrap();
    assert_eq!(report.delivery, RunDelivery::Failed);
    assert_eq!(
        report.causes[0]["diagnostic"]["error"]["code"],
        "deadline-exceeded"
    );
}

#[test]
fn validated_completion_clears_the_job_timer_before_physical_start_return_cleanup() {
    let mut h = Host::new(
        &["a"],
        &[],
        &["a"],
        &[("a", Some(10))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    assert!(a.permit.complete(Ok(data("on time"))));
    h.advance(ms(9));
    h.logical();
    assert_eq!(h.queue.next_job_deadline(), None);
    h.advance(ms(10));
    assert!(h.queue.expire_jobs(ms(10)).unwrap().is_empty());
    h.drain();
    assert_eq!(h.finish().result.unwrap().delivery, RunDelivery::Complete);
}

#[test]
fn queued_time_does_not_consume_the_job_budget() {
    let mut h = Host::new(
        &["a", "b"],
        &[],
        &["a", "b"],
        &[("a", None), ("b", Some(5))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    assert_eq!(h.queue.next_job_deadline(), None);
    h.advance(ms(20));
    assert!(h.queue.expire_jobs(ms(20)).unwrap().is_empty());
    assert!(a.permit.complete(Ok(data("a"))));
    h.logical();
    h.drain();
    let events = h.queue.dispatch_queued(ms(20)).unwrap();
    h.apply(events);
    let b = h.start();
    assert_eq!(b.node, "b");
    assert_eq!(h.queue.next_job_deadline(), Some(ms(25)));
    assert!(h.queue.expire_jobs(ms(24)).unwrap().is_empty());
    h.advance(ms(25));
    let events = h.queue.expire_jobs(ms(25)).unwrap();
    h.apply(events);
    assert!(b.cancel.is_cancelled());
    assert!(b.permit.finish_cancelled());
    h.drain();
    assert_eq!(h.finish().result.unwrap().delivery, RunDelivery::Failed);
}

#[test]
fn control_can_recover_a_job_deadline_while_the_old_worker_is_still_owned() {
    let mut h = Host::new(
        &["a", "recover"],
        &[("a", "recover")],
        &["recover"],
        &[("a", Some(5))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    let mut observer = h.observers.remove("recover").unwrap();
    assert!(matches!(
        h.graph
            .observe(
                "recover",
                &mut observer,
                WakeReason::Started,
                id(10),
                Duration::ZERO
            )
            .unwrap(),
        GraphObservation::Wait { .. }
    ));
    h.advance(ms(5));
    let events = h.queue.expire_jobs(ms(5)).unwrap();
    h.apply(events);
    assert!(matches!(
        h.graph
            .observe("recover", &mut observer, WakeReason::Input, id(11), ms(5))
            .unwrap(),
        GraphObservation::Finished { .. }
    ));
    let report = h.finish();
    assert_eq!(
        report.result.as_ref().unwrap().delivery,
        RunDelivery::Recovered
    );
    assert_eq!(
        report.result.unwrap().causes[0]["diagnostic"]["error"]["code"],
        "deadline-exceeded"
    );
    assert_eq!(h.host.usage().physical_jobs, 1);
    assert!(a.permit.finish_cancelled());
    h.drain();
}

#[test]
fn initial_deadline_overflow_returns_a_node_error_without_starting_the_adapter() {
    let mut h = Host::new(&["a"], &[], &["a"], &[("a", Some(1))], Duration::MAX, false);
    assert_eq!(h.initial.len(), 1);
    assert_eq!(h.initial[0].error().unwrap().code, "invalid-deadline");
    assert!(h.entered.try_recv().is_err());
    assert_eq!(h.host.usage().physical_jobs, 0);
    let events = std::mem::take(&mut h.initial);
    h.apply(events);
    assert_eq!(
        h.finish().result.unwrap().causes[0]["diagnostic"]["error"]["code"],
        "invalid-deadline"
    );
}

#[test]
fn queued_and_later_ready_overflow_fail_before_handoff_and_release_reservations() {
    for queued in [false, true] {
        let mut h = Host::new(
            &["a", "b"],
            if queued { &[] } else { &[("a", "b")] },
            if queued { &["a", "b"] } else { &["b"] },
            &[("b", Some(5))],
            Duration::MAX - ms(2),
            false,
        );
        let a = h.start();
        h.advance(Duration::MAX);
        assert!(a.permit.complete(Ok(data("a"))));
        h.logical();
        h.drain();
        let events = if queued {
            h.queue.dispatch_queued(Duration::MAX).unwrap()
        } else {
            h.queue.schedule_ready(&mut h.graph, Duration::MAX).unwrap()
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].error().unwrap().code, "invalid-deadline");
        h.apply(events);
        assert!(h.entered.try_recv().is_err());
        assert_eq!(h.host.usage().physical_jobs, 0);
        assert_eq!(h.host.usage().queued_input_bytes, 0);
        assert_eq!(h.finish().result.unwrap().delivery, RunDelivery::Failed);
    }
}

#[test]
fn explicit_cancel_removes_the_deadline_and_only_real_termination_reclaims_capacity() {
    let mut h = Host::new(
        &["a"],
        &[],
        &["a"],
        &[("a", Some(5))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    let key = h.graph.key().clone();
    h.advance(ms(1));
    h.queue.cancel_run(&key, ms(1)).unwrap();
    assert_eq!(h.queue.next_job_deadline(), None);
    h.advance(ms(10));
    assert!(h.queue.expire_jobs(ms(10)).unwrap().is_empty());
    h.queue.mark_stalled(ms(10)).unwrap();
    assert_eq!(h.queue.counts(&key).stalled, 1);
    assert_eq!(h.host.usage().physical_jobs, 1);
    h.graph
        .finish(&mut h.ledger, TerminalReason::Cancelled)
        .unwrap();
    assert!(a.permit.finish_cancelled());
    h.drain();
}

#[test]
fn ledger_graph_factory_and_later_ready_nodes_preserve_the_declared_budget() {
    let mut h = Host::new(
        &["a", "b"],
        &[("a", "b")],
        &["b"],
        &[("b", Some(3))],
        Duration::ZERO,
        true,
    );
    let a = h.start();
    assert_eq!(h.queue.next_job_deadline(), None);
    assert!(a.permit.complete(Ok(data("a"))));
    h.logical();
    h.drain();
    h.advance(ms(10));
    let events = h.queue.schedule_ready(&mut h.graph, ms(10)).unwrap();
    h.apply(events);
    let b = h.start();
    assert_eq!(h.queue.next_job_deadline(), Some(ms(13)));
    h.advance(ms(13));
    let events = h.queue.expire_jobs(ms(13)).unwrap();
    h.apply(events);
    assert!(b.permit.finish_cancelled());
    h.drain();
    assert_eq!(h.finish().result.unwrap().delivery, RunDelivery::Failed);
}

#[test]
fn a_logical_start_failure_clears_its_timer_even_without_termination_proof() {
    let mut h = Host::new(
        &["start-fail"],
        &[],
        &["start-fail"],
        &[("start-fail", Some(1))],
        Duration::ZERO,
        false,
    );
    let a = h.start();
    h.logical();
    assert_eq!(h.queue.next_job_deadline(), None);
    h.advance(ms(10));
    assert!(h.queue.expire_jobs(ms(10)).unwrap().is_empty());
    assert_eq!(h.host.usage().physical_jobs, 1);
    assert_eq!(
        h.finish().result.unwrap().causes[0]["diagnostic"]["error"]["code"],
        "start-failed"
    );
    h.queue.cancel_run(h.graph.key(), ms(10)).unwrap();
    assert!(a.permit.finish_cancelled());
    h.drain();
}
