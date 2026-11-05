#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    execution_control::{
        AdmittedGraph, CancellationToken, CompletionPermit, ControlDecision, ControlFrame,
        ControlNode, DecimalId, DeferredNode, DispatchQueue, ExecutionHost, GraphExecution,
        OwnedInvocation, ResourcePolicy, ResourceUsage, RunDelivery, RunKey, RunLedger, RunPhase,
        SessionEpoch, StartRequest, TerminalReason,
    },
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputMode, InputPort, NodeDefinition, Port, Registry, RunContext, RuntimeInput,
    },
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
use test_nodes::register_demo_nodes;
struct Started {
    node: String,
    key: RunKey,
    permit: CompletionPermit,
    cancel: CancellationToken,
}
struct Async {
    sender: mpsc::Sender<Started>,
}
impl DeferredNode for Async {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.async", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        inv: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.sender
            .send(Started {
                node: inv.context().node_id.into(),
                key: inv.key().clone(),
                permit,
                cancel,
            })
            .unwrap();
        Ok(())
    }
}
struct Observer {
    calls: Arc<AtomicU64>,
}
impl ControlNode for Observer {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.observe",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn observe(
        &self,
        context: RunContext<'_>,
        _: &ControlFrame,
        _: &Value,
    ) -> Result<ControlDecision, CoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if context.node_id == "fast" {
            return Ok(ControlDecision::Finish(data(text("fast"))));
        }
        Ok(ControlDecision::Wait {
            wake_after_ms: None,
        })
    }
}
struct Setup {
    host: ExecutionHost,
    queue: DispatchQueue,
    registry: Registry,
    entered: mpsc::Receiver<Started>,
    observed: Arc<AtomicU64>,
}
impl Setup {
    fn new(profile: ResourcePolicy) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let host = ExecutionHost::new(
            format!("dispatch-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            profile,
        )
        .unwrap();
        let queue = DispatchQueue::new(host.clone()).unwrap();
        let (tx, rx) = mpsc::channel();
        let observed = Arc::new(AtomicU64::new(0));
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        registry.register_deferred(Async { sender: tx }).unwrap();
        registry
            .register_control(Observer {
                calls: Arc::clone(&observed),
            })
            .unwrap();
        Self {
            host,
            queue,
            registry,
            entered: rx,
            observed,
        }
    }
    fn ledger(&self, nodes: Value, edges: Value) -> RunLedger {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let epoch =
            SessionEpoch::new(format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed))).unwrap();
        let graph = Graph::from_json(&json!({"version":1,"nodes":nodes,"edges":edges}).to_string())
            .unwrap();
        RunLedger::new(
            self.host.clone(),
            epoch,
            ExecutionPlan::compile(&graph, &self.registry).unwrap(),
            id(1),
        )
    }
    fn entered(&self) -> Started {
        self.entered.recv_timeout(Duration::from_secs(5)).unwrap()
    }
}
fn profile() -> ResourcePolicy {
    ResourcePolicy::from_value(serde_json::from_str::<Value>(include_str!("contracts/execution_control.json")).unwrap()["resourceProfiles"]["oneSlot"].clone()).unwrap()
}
fn node(name: &str, kind: &str) -> Value {
    json!({"id":name,"kind":kind,"version":1})
}
fn roots(names: &[&str]) -> Value {
    json!(
        names
            .iter()
            .map(|name| node(name, "test.async"))
            .collect::<Vec<_>>()
    )
}
fn edge(from: &str, to: &str) -> Value {
    json!({"from":{"node":from,"port":"out"},"to":{"node":to,"port":"in"}})
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn request(key: u64, names: &[&str]) -> StartRequest {
    serde_json::from_value(json!({"startKey":key.to_string(),"expectedPlanVersion":"1","targets":names.iter().map(|name|json!({"node":name,"port":"out"})).collect::<Vec<_>>()})).unwrap()
}
fn data(packet: Arc<Packet>) -> diy_ime2_core::error_input::ErrorAwareExecution {
    diy_ime2_core::error_input::ErrorAwareExecution::new(
        [(
            "out".into(),
            diy_ime2_core::error_input::ErrorAwareValue::Data(packet),
        )]
        .into(),
    )
}
fn text(value: &str) -> Arc<Packet> {
    Arc::new(Packet::new(Candidate::Text {
        text: value.into(),
        meta: None,
    }))
}
fn complete(started: &Started) {
    assert!(started.permit.complete(Ok(data(text(&started.node)))));
}
fn apply_logical(queue: &mut DispatchQueue, graph: &mut GraphExecution) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut logical = false;
        for event in queue.poll() {
            if &event.stamp().key != graph.key() {
                assert!(event.completion().unwrap().execution().is_none());
                continue;
            }
            logical |= event.completion().is_some_and(|e| e.execution().is_some());
            event.apply(graph).unwrap();
        }
        if logical {
            return;
        }
        assert!(Instant::now() < deadline, "completion timeout");
        std::thread::yield_now();
    }
}
fn drain(queue: &mut DispatchQueue, host: &ExecutionHost) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.usage().physical_jobs > 0 {
        for event in queue.poll() {
            assert!(event.completion().unwrap().execution().is_none());
        }
        assert!(Instant::now() < deadline, "physical completion timeout");
        std::thread::yield_now();
    }
}
fn cancel(run: &mut AdmittedGraph, ledger: &mut RunLedger, queue: &mut DispatchQueue) {
    if let Some(graph) = &mut run.graph {
        graph.finish(ledger, TerminalReason::Cancelled).unwrap();
    }
    queue.cancel_run(&run.receipt.key, ledger.now()).unwrap();
    run.observers.clear();
}

struct HeldStart {
    started: Started,
    release: mpsc::Sender<()>,
}
impl Drop for HeldStart {
    fn drop(&mut self) {
        self.started.permit.finish_cancelled();
        let _ = self.release.send(());
    }
}
struct GatedAsync(mpsc::Sender<HeldStart>);
impl DeferredNode for GatedAsync {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.gated", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        inv: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let (release, gate) = mpsc::channel();
        self.0
            .send(HeldStart {
                started: Started {
                    node: inv.context().node_id.into(),
                    key: inv.key().clone(),
                    permit,
                    cancel,
                },
                release,
            })
            .unwrap();
        let _ = gate.recv();
        Ok(())
    }
}

#[test]
fn cancelling_run_keeps_committed_producer_uncancelled_until_start_returns() {
    let mut p = profile();
    p.max_physical_jobs = 2;
    let mut s = Setup::new(p);
    let (sender, entered) = mpsc::channel();
    s.registry.register_deferred(GatedAsync(sender)).unwrap();
    let mut ledger = s.ledger(
        json!([node("fast", "test.gated"), node("slow", "test.gated")]),
        json!([]),
    );
    let mut run = s
        .queue
        .admit(&mut ledger, request(1, &["fast", "slow"]))
        .unwrap();
    let a = entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let b = entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let (fast, slow) = if a.started.node == "fast" {
        (a, b)
    } else {
        (b, a)
    };
    complete(&fast.started);
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    assert_eq!(s.host.usage().physical_jobs, 2);
    cancel(&mut run, &mut ledger, &mut s.queue);
    let fast_cancelled = fast.started.cancel.is_cancelled();
    let slow_cancelled = slow.started.cancel.is_cancelled();
    assert_eq!(s.host.usage().physical_jobs, 2);
    // Producer completion cannot release the slot while its start is held.
    slow.started.permit.finish_cancelled();
    s.queue.poll();
    assert_eq!(s.host.usage().physical_jobs, 2);
    drop(fast);
    drop(slow);
    drain(&mut s.queue, &s.host);
    assert!(
        !fast_cancelled,
        "a committed producer must not receive cancellation"
    );
    assert!(
        slow_cancelled,
        "the unfinished producer must receive cancellation"
    );
}

#[test]
fn atomic_busy_preserves_start_key_input_and_old_resources_then_retry_and_replay_are_safe() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(roots(&["a", "b"]), json!([]));
    let held = s
        .host
        .try_reserve(ResourceUsage {
            queued_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    let before = s.host.usage();
    let mut req = request(1, &["a", "b"]);
    req.input = Some(RuntimeInput {
        revision: 1,
        text: "かな😀".into(),
        mode: InputMode::Compose,
    });
    assert_eq!(
        s.queue.admit(&mut ledger, req.clone()).err().unwrap().code,
        "busy"
    );
    assert_eq!(s.host.usage(), before);
    assert_eq!(ledger.latest_input_revision(), None);
    assert_eq!(ledger.get_start(id(1)).unwrap_err().code, "start-not-seen");
    assert!(s.entered.try_recv().is_err());
    assert_eq!(s.queue.queued().count(), 0);
    drop(held);
    let mut run = s.queue.admit(&mut ledger, req.clone()).unwrap();
    ledger.mark_running(&run.receipt.key).unwrap();
    let a = s.entered();
    assert_eq!(a.node, "a");
    assert_eq!(s.queue.counts(&run.receipt.key).queued, 1);
    assert_eq!(s.host.usage().physical_jobs, 1);
    let before = s.host.usage();
    let replay = s.queue.admit(&mut ledger, req.clone()).unwrap();
    assert!(replay.receipt.replayed);
    assert!(replay.graph.is_none());
    assert!(replay.observers.is_empty());
    assert_eq!(s.host.usage(), before);
    assert_eq!(
        ledger.get_run(&run.receipt.key).unwrap().state,
        RunPhase::Running
    );
    req.input.as_mut().unwrap().revision = 2;
    assert_eq!(
        s.queue.admit(&mut ledger, req).err().unwrap().code,
        "start-conflict"
    );
    assert!(!a.cancel.is_cancelled());
    assert_eq!(ledger.latest_input_revision(), Some(1));
    complete(&a);
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    assert!(s.queue.dispatch_queued(Duration::ZERO).unwrap().is_empty());
    let b = s.entered();
    assert_eq!(b.node, "b");
    complete(&b);
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    assert_eq!(
        run.graph
            .as_mut()
            .unwrap()
            .finish(&mut ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .result
            .unwrap()
            .delivery,
        RunDelivery::Complete
    );
}

#[test]
fn observer_and_byte_limits_are_checked_before_any_initial_callback() {
    for observer_limit in [true, false] {
        let mut p = profile();
        p.max_observers = 1;
        if !observer_limit {
            p.max_queued_input_bytes = 1;
        }
        let mut s = Setup::new(p);
        let mut ledger = if observer_limit {
            s.ledger(
                json!([node("a", "test.async"), node("watch", "test.observe")]),
                json!([edge("a", "watch")]),
            )
        } else {
            s.ledger(roots(&["a", "b"]), json!([]))
        };
        let held = observer_limit.then(|| {
            s.host
                .try_reserve(ResourceUsage {
                    observers: 1,
                    ..Default::default()
                })
                .unwrap()
        });
        let before = s.host.usage();
        let names = if observer_limit {
            vec!["watch"]
        } else {
            vec!["a", "b"]
        };
        assert_eq!(
            s.queue
                .admit(&mut ledger, request(1, &names))
                .err()
                .unwrap()
                .code,
            "busy"
        );
        assert_eq!(s.host.usage(), before);
        assert_eq!(s.observed.load(Ordering::Relaxed), 0);
        assert!(s.entered.try_recv().is_err());
        drop(held);
    }
}

#[test]
fn initial_observer_is_reserved_without_running_policy_and_is_reused_on_replay() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(json!([node("watch", "test.observe")]), json!([]));
    let req = request(1, &["watch"]);
    let mut run = s.queue.admit(&mut ledger, req.clone()).unwrap();
    assert_eq!(run.observers.len(), 1);
    assert_eq!(s.host.usage().observers, 1);
    assert_eq!(s.host.usage().physical_jobs, 0);
    assert_eq!(s.observed.load(Ordering::Relaxed), 0);
    assert!(
        s.queue
            .admit(&mut ledger, req)
            .unwrap()
            .observers
            .is_empty()
    );
    assert_eq!(s.host.usage().observers, 1);
    cancel(&mut run, &mut ledger, &mut s.queue);
    assert_eq!(s.host.usage().observers, 0);
}

#[test]
fn shared_host_fifo_does_not_let_a_new_run_overtake_waiting_work() {
    let mut p = profile();
    p.max_queued_jobs = 4;
    let mut s = Setup::new(p);
    let mut la = s.ledger(roots(&["a"]), json!([]));
    let mut lb = s.ledger(roots(&["b"]), json!([]));
    let mut lc = s.ledger(roots(&["c"]), json!([]));
    let mut a = s.queue.admit(&mut la, request(1, &["a"])).unwrap();
    let sa = s.entered();
    let mut b = s.queue.admit(&mut lb, request(1, &["b"])).unwrap();
    assert!(s.entered.try_recv().is_err());
    complete(&sa);
    apply_logical(&mut s.queue, a.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    let mut c = s.queue.admit(&mut lc, request(1, &["c"])).unwrap();
    assert_eq!(s.host.usage().physical_jobs, 0);
    assert_eq!(
        s.queue
            .queued()
            .map(|stamp| stamp.key.clone())
            .collect::<Vec<_>>(),
        [b.receipt.key.clone(), c.receipt.key.clone()]
    );
    s.queue.dispatch_queued(Duration::ZERO).unwrap();
    let sb = s.entered();
    assert_eq!(sb.node, "b");
    assert!(s.entered.try_recv().is_err());
    complete(&sb);
    apply_logical(&mut s.queue, b.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    s.queue.dispatch_queued(Duration::ZERO).unwrap();
    let sc = s.entered();
    assert_eq!(sc.node, "c");
    complete(&sc);
    apply_logical(&mut s.queue, c.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    for (run, ledger) in [(&mut a, &mut la), (&mut b, &mut lb), (&mut c, &mut lc)] {
        assert_eq!(
            run.graph
                .as_mut()
                .unwrap()
                .finish(ledger, TerminalReason::Completed)
                .unwrap()
                .snapshot
                .result
                .unwrap()
                .delivery,
            RunDelivery::Complete
        );
    }
}

#[test]
fn queue_cancel_drops_bytes_but_running_cancel_and_stalled_keep_the_slot() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(roots(&["a", "b"]), json!([]));
    let mut run = s.queue.admit(&mut ledger, request(1, &["a", "b"])).unwrap();
    let a = s.entered();
    let stamp = s.queue.queued().next().unwrap().clone();
    let mut wrong = stamp.clone();
    wrong.key.run_id = wrong.key.run_id.checked_next().unwrap();
    let before = s.host.usage();
    assert!(!s.queue.cancel(&wrong, Duration::ZERO).unwrap());
    assert_eq!(s.host.usage(), before);
    assert!(s.queue.cancel(&stamp, Duration::ZERO).unwrap());
    assert_eq!(s.host.usage().queued_jobs, 0);
    assert_eq!(s.host.usage().queued_input_bytes, 0);
    assert!(s.entered.try_recv().is_err());
    cancel(&mut run, &mut ledger, &mut s.queue);
    assert!(a.cancel.is_cancelled());
    assert_eq!(s.host.usage().physical_jobs, 1);
    s.queue.mark_stalled(Duration::from_millis(5)).unwrap();
    assert_eq!(s.queue.counts(&run.receipt.key).stalled, 1);
    assert_eq!(s.host.usage().physical_jobs, 1);
    assert!(a.permit.finish_cancelled());
    drain(&mut s.queue, &s.host);
    assert_eq!(s.host.usage().physical_jobs, 0);
}

#[test]
fn queued_wait_expiry_at_a_freed_slot_boundary_never_starts_the_expired_job() {
    let mut p = profile();
    p.queue_wait_ms = Some(diy_ime2_core::execution_control::Milliseconds::new(10).unwrap());
    let mut s = Setup::new(p);
    let mut la = s.ledger(roots(&["a"]), json!([]));
    let mut lb = s.ledger(roots(&["b"]), json!([]));
    let mut a = s.queue.admit(&mut la, request(1, &["a"])).unwrap();
    let sa = s.entered();
    let mut b = s.queue.admit(&mut lb, request(1, &["b"])).unwrap();
    complete(&sa);
    apply_logical(&mut s.queue, a.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    la.advance_clock(Duration::from_millis(10)).unwrap();
    lb.advance_clock(Duration::from_millis(10)).unwrap();
    let events = s.queue.dispatch_queued(Duration::from_millis(10)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].error().unwrap().code, "queue-expired");
    for event in events {
        event.apply(b.graph.as_mut().unwrap()).unwrap();
    }
    assert!(s.entered.try_recv().is_err());
    assert_eq!(s.host.usage().queued_input_bytes, 0);
    let result = b
        .graph
        .as_mut()
        .unwrap()
        .finish(&mut lb, TerminalReason::Completed)
        .unwrap()
        .snapshot
        .result
        .unwrap();
    assert_eq!(result.delivery, RunDelivery::Failed);
    assert_eq!(
        result.causes[0]["diagnostic"]["error"]["code"],
        "queue-expired"
    );
    a.graph
        .as_mut()
        .unwrap()
        .finish(&mut la, TerminalReason::Completed)
        .unwrap();
}

#[test]
fn queue_deadlines_keep_submillisecond_precision_instead_of_reusing_display_milliseconds() {
    let mut p = profile();
    p.queue_wait_ms = Some(diy_ime2_core::execution_control::Milliseconds::new(1).unwrap());
    let mut s = Setup::new(p);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    let mut ledger = s.ledger(roots(&["a"]), json!([]));
    ledger.advance_clock(Duration::from_nanos(500_000)).unwrap();
    let mut run = s.queue.admit(&mut ledger, request(1, &["a"])).unwrap();
    assert_eq!(
        s.queue.next_deadline(),
        Some(Duration::from_nanos(1_500_000))
    );
    assert!(
        s.queue
            .expire_queue(Duration::from_nanos(1_499_999))
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.queue.queued().count(), 1);
    ledger
        .advance_clock(Duration::from_nanos(1_500_000))
        .unwrap();
    let events = s
        .queue
        .expire_queue(Duration::from_nanos(1_500_000))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(s.queue.next_deadline(), None);
    for event in events {
        event.apply(run.graph.as_mut().unwrap()).unwrap();
    }
    assert!(s.entered.try_recv().is_err());
    assert_eq!(
        run.graph
            .as_mut()
            .unwrap()
            .finish(&mut ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .elapsed_ms,
        1
    );
    drop(held);
}

#[test]
fn later_ready_capacity_failure_is_an_opaque_node_error_not_a_rejected_run() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(
        json!([node("a", "test.async"), node("sink", "demo.identity")]),
        json!([edge("a", "sink")]),
    );
    let mut run = s.queue.admit(&mut ledger, request(1, &["sink"])).unwrap();
    let a = s.entered();
    complete(&a);
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            queued_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    let events = s
        .queue
        .schedule_ready(run.graph.as_mut().unwrap(), Duration::ZERO)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].error().unwrap().code, "resource-busy");
    for event in events {
        event.apply(run.graph.as_mut().unwrap()).unwrap();
    }
    assert_eq!(ledger.get_start(id(1)).unwrap(), run.receipt.key);
    drop(held);
    let result = run
        .graph
        .as_mut()
        .unwrap()
        .finish(&mut ledger, TerminalReason::Completed)
        .unwrap()
        .snapshot
        .result
        .unwrap();
    assert_eq!(result.reason, TerminalReason::Completed);
    assert_eq!(result.delivery, RunDelivery::Failed);
    assert_eq!(result.nodes[0]["status"], "executed");
}

#[test]
fn shared_packet_bytes_stay_charged_until_the_last_queued_owner_is_removed() {
    let mut p = profile();
    p.max_queued_jobs = 4;
    let packet = text(&"😀".repeat(1024));
    let payload_bytes = serde_json::to_vec(packet.as_ref()).unwrap().len() as u64;
    let metadata=serde_json::to_vec(&json!({"inputs":{"in":{"status":"data"}},"hasRuntimeInput":false,"inputRecoveries":{"in":[]},"inputErrors":null})).unwrap().len() as u64;
    p.max_queued_input_bytes = 2 * metadata + 4 + payload_bytes;
    let mut s = Setup::new(p);
    let mut ledger = s.ledger(
        json!([
            node("a", "test.async"),
            node("left", "demo.identity"),
            node("right", "demo.identity")
        ]),
        json!([edge("a", "left"), edge("a", "right")]),
    );
    let mut run = s
        .queue
        .admit(&mut ledger, request(1, &["left", "right"]))
        .unwrap();
    let a = s.entered();
    assert!(a.permit.complete(Ok(data(packet))));
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(
        s.queue
            .schedule_ready(run.graph.as_mut().unwrap(), Duration::ZERO)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.host.usage().queued_input_bytes,
        2 * metadata + 4 + payload_bytes
    );
    let stamps = s.queue.queued().cloned().collect::<Vec<_>>();
    s.queue.cancel(&stamps[0], Duration::ZERO).unwrap();
    assert_eq!(
        s.host.usage().queued_input_bytes,
        metadata + 2 + payload_bytes
    );
    s.queue.cancel(&stamps[1], Duration::ZERO).unwrap();
    assert_eq!(s.host.usage().queued_input_bytes, 0);
    drop(held);
    cancel(&mut run, &mut ledger, &mut s.queue);
}

#[test]
fn unneeded_queued_work_is_removed_while_an_independent_target_keeps_running() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(
        json!([
            node("a", "test.async"),
            node("b", "test.async"),
            node("fast", "test.observe")
        ]),
        json!([edge("b", "fast")]),
    );
    let mut run = s
        .queue
        .admit(&mut ledger, request(1, &["a", "fast"]))
        .unwrap();
    let a = s.entered();
    assert_eq!(a.node, "a");
    let mut observer = run.observers.remove("fast").unwrap();
    let graph = run.graph.as_mut().unwrap();
    let diy_ime2_core::execution_control::GraphObservation::Finished { released } = graph
        .observe(
            "fast",
            &mut observer,
            diy_ime2_core::execution_control::WakeReason::Started,
            id(10),
            Duration::ZERO,
        )
        .unwrap()
    else {
        panic!("fast control must finish")
    };
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].node_id, "b");
    s.queue
        .cancel_released(&run.receipt.key, &released, Duration::ZERO)
        .unwrap();
    assert_eq!(s.host.usage().queued_jobs, 0);
    assert_eq!(s.host.usage().queued_input_bytes, 0);
    assert!(!a.cancel.is_cancelled());
    assert!(s.queue.dispatch_queued(Duration::ZERO).unwrap().is_empty());
    assert!(s.entered.try_recv().is_err());
    complete(&a);
    apply_logical(&mut s.queue, graph);
    drain(&mut s.queue, &s.host);
    assert_eq!(
        graph
            .finish(&mut ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .result
            .unwrap()
            .delivery,
        RunDelivery::Complete
    );
}

#[test]
fn no_queue_profile_rejects_initial_fanout_but_ignores_unrequested_roots() {
    let mut p = profile();
    p.max_queued_jobs = 0;
    let mut s = Setup::new(p);
    let mut ledger = s.ledger(roots(&["a", "b"]), json!([]));
    assert_eq!(
        s.queue
            .admit(&mut ledger, request(1, &["a", "b"]))
            .err()
            .unwrap()
            .code,
        "busy"
    );
    assert!(s.entered.try_recv().is_err());
    assert_eq!(s.host.usage().active_runs, 0);
    let mut run = s.queue.admit(&mut ledger, request(1, &["a"])).unwrap();
    let a = s.entered();
    assert_eq!(a.node, "a");
    assert!(s.entered.try_recv().is_err());
    assert_eq!(s.queue.queued().count(), 0);
    complete(&a);
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    assert_eq!(
        run.graph
            .as_mut()
            .unwrap()
            .finish(&mut ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .result
            .unwrap()
            .nodes
            .len(),
        1
    );
}

#[test]
fn shared_plan_configuration_is_counted_once_across_queued_runs() {
    let mut p = profile();
    p.max_queued_jobs = 4;
    let mut s = Setup::new(p);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    let mut ledger = s.ledger(roots(&["a"]), json!([]));
    let mut a = s.queue.admit(&mut ledger, request(1, &["a"])).unwrap();
    let first = s.host.usage().queued_input_bytes;
    let mut b = s.queue.admit(&mut ledger, request(2, &["a"])).unwrap();
    assert_eq!(s.host.usage().queued_input_bytes, 2 * first - 2);
    cancel(&mut a, &mut ledger, &mut s.queue);
    assert_eq!(s.host.usage().queued_input_bytes, first);
    cancel(&mut b, &mut ledger, &mut s.queue);
    assert_eq!(s.host.usage().queued_input_bytes, 0);
    assert!(s.entered.try_recv().is_err());
    drop(held);
}

#[test]
fn runtime_input_snapshot_has_one_queued_ownership_charge_per_run() {
    let mut p = profile();
    p.max_queued_jobs = 4;
    let mut s = Setup::new(p);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    let mut ledger = s.ledger(roots(&["a", "b"]), json!([]));
    let mut req = request(1, &["a", "b"]);
    let input = RuntimeInput {
        revision: 1,
        text: "snapshot😀".into(),
        mode: InputMode::Compose,
    };
    let bytes = serde_json::to_vec(&input).unwrap().len() as u64;
    req.input = Some(input);
    let mut run = s.queue.admit(&mut ledger, req).unwrap();
    let metadata = serde_json::to_vec(
        &json!({"inputs":{},"hasRuntimeInput":true,"inputRecoveries":{},"inputErrors":null}),
    )
    .unwrap()
    .len() as u64;
    assert_eq!(s.host.usage().queued_input_bytes, 2 * metadata + 4 + bytes);
    cancel(&mut run, &mut ledger, &mut s.queue);
    assert_eq!(s.host.usage().queued_input_bytes, 0);
    drop(held);
}

#[test]
fn one_fifo_owner_per_domain_and_foreign_ledgers_are_rejected() {
    let mut a = Setup::new(profile());
    let b = Setup::new(profile());
    assert_eq!(
        DispatchQueue::new(a.host.clone()).err().unwrap().code,
        "scheduler-exists"
    );
    let mut foreign = b.ledger(roots(&["a"]), json!([]));
    assert_eq!(
        a.queue
            .admit(&mut foreign, request(1, &["a"]))
            .err()
            .unwrap()
            .code,
        "resource-domain-conflict"
    );
    assert_eq!(a.host.usage().active_runs, 0);
    assert_eq!(b.host.usage().active_runs, 0);
    let key = foreign.start_run(request(1, &["a"])).unwrap().key;
    let mut graph = foreign.graph_execution(&key).unwrap();
    assert_eq!(
        a.queue
            .schedule_ready(&mut graph, Duration::ZERO)
            .err()
            .unwrap()
            .code,
        "resource-domain-conflict"
    );
    foreign.cancel_run(&key).unwrap();
}

#[test]
fn replacing_the_queue_does_not_reset_a_noncooperative_old_workers_budget() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(roots(&["a"]), json!([]));
    let mut fresh = s.ledger(roots(&["b"]), json!([]));
    let mut run = s.queue.admit(&mut ledger, request(1, &["a"])).unwrap();
    let a = s.entered();
    cancel(&mut run, &mut ledger, &mut s.queue);
    let Setup {
        host,
        queue: old,
        entered,
        ..
    } = s;
    drop(old);
    let mut queue = DispatchQueue::new(host.clone()).unwrap();
    assert!(a.cancel.is_cancelled());
    assert_eq!(host.usage().physical_jobs, 1);
    let mut b = queue.admit(&mut fresh, request(1, &["b"])).unwrap();
    assert_eq!(queue.counts(&b.receipt.key).queued, 1);
    assert!(entered.try_recv().is_err());
    assert!(!a.permit.finish_cancelled());
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.usage().physical_jobs > 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    queue.dispatch_queued(Duration::ZERO).unwrap();
    let sb = entered.recv_timeout(Duration::from_secs(5)).unwrap();
    complete(&sb);
    apply_logical(&mut queue, b.graph.as_mut().unwrap());
    drain(&mut queue, &host);
    b.graph
        .as_mut()
        .unwrap()
        .finish(&mut fresh, TerminalReason::Completed)
        .unwrap();
}

#[test]
fn failed_new_input_admission_keeps_old_work_but_accepted_input_cancels_it() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(roots(&["a"]), json!([]));
    let mut req = request(1, &["a"]);
    req.input = Some(RuntimeInput {
        revision: 1,
        text: "old".into(),
        mode: InputMode::Compose,
    });
    let mut a = s.queue.admit(&mut ledger, req.clone()).unwrap();
    let sa = s.entered();
    req.start_key = id(2);
    let mut b = s.queue.admit(&mut ledger, req.clone()).unwrap();
    req.start_key = id(3);
    req.input.as_mut().unwrap().revision = 2;
    req.input.as_mut().unwrap().text = "new".into();
    assert_eq!(
        s.queue.admit(&mut ledger, req.clone()).err().unwrap().code,
        "busy"
    );
    assert!(!sa.cancel.is_cancelled());
    assert_eq!(ledger.latest_input_revision(), Some(1));
    cancel(&mut b, &mut ledger, &mut s.queue);
    let mut c = s.queue.admit(&mut ledger, req).unwrap();
    assert_eq!(c.cancelled_runs, [a.receipt.key.clone()]);
    assert!(sa.cancel.is_cancelled());
    assert_eq!(ledger.latest_input_revision(), Some(2));
    a.graph
        .as_mut()
        .unwrap()
        .finish(&mut ledger, TerminalReason::Cancelled)
        .unwrap();
    assert!(sa.permit.finish_cancelled());
    drain(&mut s.queue, &s.host);
    s.queue.dispatch_queued(Duration::ZERO).unwrap();
    let sc = s.entered();
    assert_eq!(sc.key, c.receipt.key);
    complete(&sc);
    apply_logical(&mut s.queue, c.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    assert_eq!(
        c.graph
            .as_mut()
            .unwrap()
            .finish(&mut ledger, TerminalReason::Completed)
            .unwrap()
            .snapshot
            .result
            .unwrap()
            .delivery,
        RunDelivery::Complete
    );
}

#[test]
fn deadline_overflow_and_backward_queue_clock_fail_without_admitting_or_removing_work() {
    let mut p = profile();
    p.queue_wait_ms = Some(diy_ime2_core::execution_control::Milliseconds::new(1).unwrap());
    let mut s = Setup::new(p);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    let mut ledger = s.ledger(roots(&["a"]), json!([]));
    ledger.advance_clock(Duration::MAX).unwrap();
    let before = s.host.usage();
    assert_eq!(
        s.queue
            .admit(&mut ledger, request(1, &["a"]))
            .err()
            .unwrap()
            .code,
        "invalid-deadline"
    );
    assert_eq!(s.host.usage(), before);
    assert_eq!(ledger.get_start(id(1)).unwrap_err().code, "start-not-seen");
    drop(held);
    s.queue.expire_queue(Duration::from_millis(5)).unwrap();
    assert_eq!(
        s.queue
            .expire_queue(Duration::from_millis(4))
            .err()
            .unwrap()
            .code,
        "invalid-clock"
    );
}

#[test]
fn deep_native_packet_can_wait_without_becoming_a_transport_error_at_queue_admission() {
    let mut s = Setup::new(profile());
    let mut ledger = s.ledger(
        json!([node("a", "test.async"), node("sink", "demo.identity")]),
        json!([edge("a", "sink")]),
    );
    let mut run = s.queue.admit(&mut ledger, request(1, &["sink"])).unwrap();
    let a = s.entered();
    let mut root = Candidate::Text {
        text: "deep".into(),
        meta: None,
    };
    for _ in 0..150 {
        root = Candidate::Sequence { items: vec![root] };
    }
    let packet = Arc::new(Packet::new(root));
    assert!(packet.to_json().is_err());
    assert!(a.permit.complete(Ok(data(packet))));
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    let held = s
        .host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(
        s.queue
            .schedule_ready(run.graph.as_mut().unwrap(), Duration::ZERO)
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.host.usage().queued_jobs, 1);
    drop(held);
    s.queue.dispatch_queued(Duration::ZERO).unwrap();
    apply_logical(&mut s.queue, run.graph.as_mut().unwrap());
    drain(&mut s.queue, &s.host);
    let result = run
        .graph
        .as_mut()
        .unwrap()
        .finish(&mut ledger, TerminalReason::Completed)
        .unwrap()
        .snapshot
        .result
        .unwrap();
    assert_eq!(result.error.unwrap().code, "report-failed");
}
