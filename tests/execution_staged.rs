#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue},
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlInput,
        ControlNode, ControlObservation, ControlObserver, DecimalId, DeferredNode, DispatchQueue,
        ExecutionHost, GraphExecution, GraphNodePhase, GraphObservation, Milliseconds,
        OwnedInvocation, PreparedDispatchEvent, ResourcePolicy, RunLedger, SessionEpoch,
        StartRequest, TerminalReason, WakeReason,
    },
    graph::{ExecutionPlan, Graph},
    node::{CoreError, InputPort, NodeDefinition, Port, Registry, RunContext},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use test_nodes::register_demo_nodes;
fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn data() -> ErrorAwareExecution {
    ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: "data".into(),
                meta: None,
            }))),
        )]
        .into(),
    )
}
struct Async(mpsc::Sender<CompletionPermit>);
impl DeferredNode for Async {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.async", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        _: OwnedInvocation,
        permit: CompletionPermit,
        _: CancellationToken,
    ) -> Result<(), CoreError> {
        self.0.send(permit).unwrap();
        Ok(())
    }
}
struct Control;
impl ControlNode for Control {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.control",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn observe(
        &self,
        ctx: RunContext<'_>,
        frame: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        assert!(ctx.node_id != "panic", "test panic");
        if ctx.node_id == "finish" {
            return Ok(ControlDecision::Finish(data()));
        }
        if ctx.node_id != "wait"
            && let ControlInput::Resolved(value) = &frame.inputs["in"]
        {
            return Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                [("out".into(), value.clone())].into(),
            )));
        }
        Ok(ControlDecision::Wait {
            wake_after_ms: Some(Milliseconds::new(5).unwrap()),
        })
    }
}
struct Host {
    host: ExecutionHost,
    ledger: RunLedger,
    queue: DispatchQueue,
    graph: GraphExecution,
    observer: ControlObserver,
    entered: mpsc::Receiver<CompletionPermit>,
    permits: Vec<CompletionPermit>,
}
impl Host {
    fn new(sync: bool, name: &str, budget: bool, at: Duration) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let policy = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        let host = ExecutionHost::new(
            format!("staged-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            policy,
        )
        .unwrap();
        let (send, entered) = mpsc::channel();
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        registry.register_deferred(Async(send)).unwrap();
        registry.register_control(Control).unwrap();
        let source = if sync {
            json!({"id":"a","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"data"}}}})
        } else {
            json!({"id":"a","kind":"test.async","version":1})
        };
        let graph=Graph::from_json(&json!({"version":1,"nodes":[source,{"id":name,"kind":"test.control","version":1}],"edges":[{"from":{"node":"a","port":"out"},"to":{"node":name,"port":"in"}}]}).to_string()).unwrap();
        let mut ledger = RunLedger::new(
            host.clone(),
            SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            id(1),
        );
        ledger.advance_clock(at).unwrap();
        let request=serde_json::from_value::<StartRequest>(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":name,"port":"out"}],"jobBudgets":if budget {json!([{"nodeId":"a","budgetMs":5}])}else{json!([])}})).unwrap();
        let mut queue = DispatchQueue::new(host.clone()).unwrap();
        let mut admitted = queue.admit(&mut ledger, request).unwrap();
        assert!(admitted.events.is_empty());
        Self {
            host,
            ledger,
            queue,
            graph: admitted.graph.unwrap(),
            observer: admitted.observers.remove(name).unwrap(),
            entered,
            permits: vec![],
        }
    }
    fn permit(&mut self) -> CompletionPermit {
        let permit = self.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        self.permits.push(permit.clone());
        permit
    }
    fn event(&mut self) -> PreparedDispatchEvent {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = self.queue.prepare_event().unwrap() {
                return event;
            }
            assert!(Instant::now() < end);
            let _ = self.queue.wake_signal().wait(Some(ms(1))).unwrap();
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let now = self.ledger.now();
        self.queue.cancel_run(self.graph.key(), now).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.permits.extend(self.entered.try_iter());
            for permit in &self.permits {
                permit.finish_cancelled();
            }
            self.queue.poll();
            if self.host.usage().physical_jobs == 0 {
                break;
            }
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
    }
}

#[test]
fn wait_is_anchored_at_commit_instead_of_callback_preparation() {
    let mut h = Host::new(false, "wait", false, Duration::ZERO);
    let _permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation("wait", &mut h.observer, WakeReason::Started, id(1), ms(1))
        .unwrap();
    assert_eq!(h.host.usage().observers, 1);
    assert_eq!(h.graph.phase("wait"), Some(GraphNodePhase::Active));
    assert!(
        matches!(h.graph.commit_observation(&mut h.observer,prepared,ms(10)).unwrap(),GraphObservation::Wait{wake_at:Some(at)} if at==ms(15))
    );
}
#[test]
fn prepared_finish_keeps_the_observer_and_graph_unresolved_until_commit() {
    let mut h = Host::new(false, "finish", false, Duration::ZERO);
    let _permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation(
            "finish",
            &mut h.observer,
            WakeReason::Started,
            id(1),
            Duration::ZERO,
        )
        .unwrap();
    assert!(!h.graph.is_complete());
    assert_eq!(h.host.usage().observers, 1);
    assert!(matches!(
        h.graph
            .commit_observation(&mut h.observer, prepared, ms(2))
            .unwrap(),
        GraphObservation::Finished { .. }
    ));
    assert!(h.graph.is_complete());
    assert_eq!(h.host.usage().observers, 0);
}
#[test]
fn a_changed_input_rejects_a_prepared_decision_and_accepts_a_fresh_frame() {
    let mut h = Host::new(false, "observer", false, Duration::ZERO);
    let permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation(
            "observer",
            &mut h.observer,
            WakeReason::Started,
            id(1),
            Duration::ZERO,
        )
        .unwrap();
    assert!(permit.complete(Ok(data())));
    let event = h.event();
    h.queue
        .commit_event(event)
        .unwrap()
        .apply(&mut h.graph)
        .unwrap();
    assert_eq!(
        h.graph
            .commit_observation(&mut h.observer, prepared, ms(1))
            .err()
            .unwrap()
            .code,
        "observation-stale"
    );
    assert_eq!(h.host.usage().observers, 1);
    let fresh = h
        .graph
        .prepare_observation("observer", &mut h.observer, WakeReason::Input, id(2), ms(1))
        .unwrap();
    h.graph
        .commit_observation(&mut h.observer, fresh, ms(2))
        .unwrap();
    assert!(h.graph.is_complete());
}
#[test]
fn an_older_observer_token_cannot_commit_after_a_new_frame_was_prepared() {
    let mut h = Host::new(false, "wait", false, Duration::ZERO);
    let _permit = h.permit();
    let inputs = || BTreeMap::from([("in".into(), ControlInput::Pending)]);
    let old = h
        .observer
        .prepare(inputs(), WakeReason::Started, id(1), Duration::ZERO)
        .unwrap();
    let new = h
        .observer
        .prepare(inputs(), WakeReason::Timer, id(2), ms(1))
        .unwrap();
    assert_eq!(
        h.observer.commit(old, ms(2)).err().unwrap().code,
        "observation-stale"
    );
    assert!(
        matches!(h.observer.commit(new,ms(2)).unwrap(),ControlObservation::Wait{wake_at:Some(at)} if at==ms(7))
    );
}
#[test]
fn graph_and_observer_authorities_prevent_cross_owner_prepared_decisions() {
    let mut h = Host::new(false, "finish", false, Duration::ZERO);
    let _permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation(
            "finish",
            &mut h.observer,
            WakeReason::Started,
            id(1),
            Duration::ZERO,
        )
        .unwrap();
    let mut foreign = h.ledger.control_observer(h.graph.key(), "finish").unwrap();
    assert_eq!(
        h.graph
            .commit_observation(&mut foreign, prepared, Duration::ZERO)
            .err()
            .unwrap()
            .code,
        "observation-stale"
    );
    assert_eq!(h.host.usage().observers, 2);
    drop(foreign);
    let prepared = h
        .graph
        .prepare_observation(
            "finish",
            &mut h.observer,
            WakeReason::Started,
            id(2),
            Duration::ZERO,
        )
        .unwrap();
    let mut mirror = h.ledger.graph_execution(h.graph.key()).unwrap();
    let mut other = mirror.start_observer("finish").unwrap();
    assert_eq!(
        mirror
            .commit_observation(&mut other, prepared, Duration::ZERO)
            .err()
            .unwrap()
            .code,
        "observer-conflict"
    );
    assert_eq!(h.graph.phase("finish"), Some(GraphNodePhase::Active));
}
#[test]
fn dropped_or_expired_prepared_control_results_do_not_resolve_the_graph() {
    let mut h = Host::new(false, "finish", false, Duration::ZERO);
    let _permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation(
            "finish",
            &mut h.observer,
            WakeReason::Started,
            id(1),
            Duration::ZERO,
        )
        .unwrap();
    drop(prepared);
    assert!(!h.graph.is_complete());
    assert_eq!(h.host.usage().observers, 1);
    let prepared = h
        .graph
        .prepare_observation(
            "finish",
            &mut h.observer,
            WakeReason::Started,
            id(2),
            Duration::ZERO,
        )
        .unwrap();
    h.graph
        .finish(&mut h.ledger, TerminalReason::Cancelled)
        .unwrap();
    assert_eq!(
        h.graph
            .commit_observation(&mut h.observer, prepared, Duration::ZERO)
            .err()
            .unwrap()
            .code,
        "observation-stale"
    );
}
#[test]
fn wait_deadline_overflow_is_checked_at_the_later_commit_time() {
    let at = Duration::MAX - ms(2);
    let mut h = Host::new(false, "wait", false, at);
    let _permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation("wait", &mut h.observer, WakeReason::Started, id(1), at)
        .unwrap();
    assert!(matches!(
        h.graph
            .commit_observation(&mut h.observer, prepared, Duration::MAX)
            .unwrap(),
        GraphObservation::Finished { .. }
    ));
    h.ledger.advance_clock(Duration::MAX).unwrap();
    let report = h
        .graph
        .finish(&mut h.ledger, TerminalReason::Completed)
        .unwrap()
        .snapshot
        .result
        .unwrap();
    assert_eq!(
        report.causes[0]["diagnostic"]["error"]["code"],
        "invalid-deadline"
    );
}
#[test]
fn panic_is_prepared_as_a_node_error_and_only_committed_once() {
    let mut h = Host::new(false, "panic", false, Duration::ZERO);
    let _permit = h.permit();
    let prepared = h
        .graph
        .prepare_observation(
            "panic",
            &mut h.observer,
            WakeReason::Started,
            id(1),
            Duration::ZERO,
        )
        .unwrap();
    assert!(!h.graph.is_complete());
    assert_eq!(h.host.usage().observers, 1);
    h.graph
        .commit_observation(&mut h.observer, prepared, Duration::ZERO)
        .unwrap();
    let report = h
        .graph
        .finish(&mut h.ledger, TerminalReason::Completed)
        .unwrap()
        .snapshot
        .result
        .unwrap();
    assert_eq!(
        report.causes[0]["diagnostic"]["error"]["code"],
        "node-panic"
    );
}
#[test]
fn a_physically_finished_notification_keeps_its_job_deadline_until_adoption() {
    let mut h = Host::new(true, "observer", true, Duration::ZERO);
    let prepared = h.event();
    assert!(prepared.completion().physically_finished());
    assert_eq!(h.host.usage().physical_jobs, 0);
    assert_eq!(h.queue.next_job_deadline(), Some(ms(5)));
    let graph_result = h
        .graph
        .prepare_completion(prepared.completion().clone())
        .unwrap()
        .unwrap();
    assert_eq!(h.graph.phase("a"), Some(GraphNodePhase::Active));
    h.ledger.advance_clock(ms(5)).unwrap();
    for event in h.queue.expire_jobs(ms(5)).unwrap() {
        event.apply(&mut h.graph).unwrap();
    }
    let late = h.queue.commit_event(prepared).unwrap();
    assert!(late.completion().unwrap().execution().is_none());
    assert_eq!(
        h.graph.commit_completion(graph_result).err().unwrap().code,
        "stale-completion"
    );
    assert_eq!(h.queue.next_job_deadline(), None);
}
#[test]
fn prepared_worker_result_cannot_resurrect_a_cancelled_graph() {
    let mut h = Host::new(true, "observer", false, Duration::ZERO);
    let prepared = h.event();
    let graph_result = h
        .graph
        .prepare_completion(prepared.completion().clone())
        .unwrap()
        .unwrap();
    h.graph
        .finish(&mut h.ledger, TerminalReason::Cancelled)
        .unwrap();
    h.queue.cancel_run(h.graph.key(), Duration::ZERO).unwrap();
    assert!(
        h.queue
            .commit_event(prepared)
            .unwrap()
            .completion()
            .unwrap()
            .execution()
            .is_none()
    );
    assert_eq!(
        h.graph.commit_completion(graph_result).err().unwrap().code,
        "stale-completion"
    );
}
#[test]
fn duplicate_or_foreign_graph_prepared_completions_do_not_adopt_twice() {
    let mut h = Host::new(true, "observer", false, Duration::ZERO);
    let prepared = h.event();
    let completion = prepared.completion().clone();
    let first = h
        .graph
        .prepare_completion(completion.clone())
        .unwrap()
        .unwrap();
    let duplicate = h
        .graph
        .prepare_completion(completion.clone())
        .unwrap()
        .unwrap();
    let foreign = h
        .graph
        .prepare_completion(completion.clone())
        .unwrap()
        .unwrap();
    let mut mirror = h.ledger.graph_execution(h.graph.key()).unwrap();
    mirror.start_work("a").unwrap();
    mirror.bind_job("a", completion.stamp().job_id).unwrap();
    assert_eq!(
        mirror.commit_completion(foreign).err().unwrap().code,
        "run-key-conflict"
    );
    h.queue.commit_event(prepared).unwrap();
    h.graph.commit_completion(first).unwrap();
    assert_eq!(
        h.graph.commit_completion(duplicate).err().unwrap().code,
        "stale-completion"
    );
}
#[test]
fn queue_preparation_is_exclusive_and_rejects_a_foreign_commit() {
    let mut h = Host::new(true, "observer", true, Duration::ZERO);
    let prepared = h.event();
    assert_eq!(
        h.queue.prepare_event().err().unwrap().code,
        "dispatch-event-pending"
    );
    assert!(h.queue.poll_one().is_none());
    let other_host = ExecutionHost::new(
        format!("foreign-{}", h.host.resource_domain()),
        h.host.policy().clone(),
    )
    .unwrap();
    let mut other = DispatchQueue::new(other_host).unwrap();
    assert_eq!(
        other.commit_event(prepared).err().unwrap().code,
        "dispatch-event-conflict"
    );
    h.ledger.advance_clock(ms(5)).unwrap();
    let expired = h.queue.expire_jobs(ms(5)).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].error().unwrap().code, "deadline-exceeded");
}
