#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue, RecoveryClaim},
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlInput,
        ControlNode, DecimalId, DeferredNode, ExecutionClock, ExecutionController, ExecutionHost,
        ExecutionService, Milliseconds, OwnedInvocation, ResourcePolicy, RunDelivery, RunPhase,
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
                text: "value😀".into(),
                meta: None,
            }))),
        )]
        .into(),
    )
}
fn until(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            Instant::now() < end,
            "service did not make independent progress"
        );
        std::thread::sleep(ms(1));
    }
}
#[derive(Clone)]
struct Started {
    permit: CompletionPermit,
    cancel: CancellationToken,
    release: Option<mpsc::Sender<()>>,
}
struct Async {
    send: mpsc::Sender<Started>,
    gated: bool,
}
impl DeferredNode for Async {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.async", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        _: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        if self.gated {
            let (release, gate) = mpsc::channel();
            self.send
                .send(Started {
                    permit,
                    cancel,
                    release: Some(release),
                })
                .unwrap();
            gate.recv().unwrap();
        } else {
            self.send
                .send(Started {
                    permit,
                    cancel,
                    release: None,
                })
                .unwrap();
        }
        Ok(())
    }
}
struct Policy(mpsc::Sender<(WakeReason, u64)>);

#[test]
fn worker_local_intervals_keep_their_clock_and_obey_recording_lifetime() {
    use diy_ime2_core::execution_control::WorkerTracePhase;
    let mut h = Harness::new(false, 1, 0, None, false);
    let (send, receive) = mpsc::channel();
    h.registry.register_deferred(TracedHost(send)).unwrap();
    h.graph=Graph::from_json(r#"{"version":1,"nodes":[{"id":"source","kind":"test.traced-host","version":1}],"edges":[]}"#).unwrap();
    let c = h.controller();
    c.start_detailed_trace().unwrap();
    let key = c.start_run(Harness::request("source")).unwrap().key;
    let (invocation, permit) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    invocation.trace_worker_interval(2, WorkerTracePhase::Compile, ms(15), Some(ms(37)));
    invocation.trace_worker_interval(2, WorkerTracePhase::GuestCall, ms(40), None);
    let trace = c.read_trace(0, 256).unwrap();
    let intervals: Vec<_> = trace
        .events
        .iter()
        .filter_map(|e| e.worker.as_ref())
        .collect();
    assert_eq!(intervals.len(), 2);
    assert_eq!(intervals[0].instance, 2);
    assert_eq!(intervals[0].started_us, "15000");
    assert_eq!(intervals[0].finished_us.as_deref(), Some("37000"));
    assert!(intervals[1].finished_us.is_none());
    let before = trace.dropped;
    invocation.trace_worker_interval(0, WorkerTracePhase::Compile, ms(1), Some(ms(2)));
    assert!(c.read_trace(0, 256).unwrap().dropped > before);
    c.clear_trace();
    c.start_detailed_trace().unwrap();
    invocation.trace_worker_interval(2, WorkerTracePhase::GuestCall, ms(40), Some(ms(50)));
    invocation.trace_worker_diagnostics_lost();
    let fresh = c.read_trace(0, 256).unwrap();
    assert!(fresh.events.is_empty());
    assert_eq!(fresh.dropped, 0);
    assert!(permit.complete(Ok(data())));
    until(|| h.host.usage().physical_jobs == 0);
    c.get_run(&key).unwrap();
}

// Retains a Host-owned invocation so recording transitions can be tested while
// an asynchronous operation is still alive, independently of Core completion.
struct TracedHost(mpsc::Sender<(OwnedInvocation, CompletionPermit)>);
impl DeferredNode for TracedHost {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.traced-host", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        invocation: OwnedInvocation,
        permit: CompletionPermit,
        _: CancellationToken,
    ) -> Result<(), CoreError> {
        self.0.send((invocation, permit)).unwrap();
        Ok(())
    }
}

#[test]
fn detailed_host_intervals_are_opt_in_and_old_spans_cannot_resurrect_recordings() {
    use diy_ime2_core::execution_control::{HostTracePhase, TraceKind};
    let mut h = Harness::new(false, 1, 0, None, false);
    let (send, receive) = mpsc::channel();
    h.registry.register_deferred(TracedHost(send)).unwrap();
    h.graph = Graph::from_json(r#"{"version":1,"nodes":[{"id":"source","kind":"test.traced-host","version":1}],"edges":[]}"#).unwrap();
    let c = h.controller();
    for mode in 0..3 {
        match mode {
            0 => (),
            1 => c.start_trace().unwrap(),
            _ => c.start_detailed_trace().unwrap(),
        }
        let mut request = Harness::request("source");
        request.start_key = id(mode + 1);
        let key = c.start_run(request).unwrap().key;
        let (invocation, permit) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        {
            let _request = invocation.trace_host_phase(HostTracePhase::Request);
            let _process = invocation.trace_host_phase(HostTracePhase::WorkerProcess);
        }
        let events = c.read_trace(0, 256).unwrap().events;
        let details: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e.event,
                    TraceKind::HostRequestStarted
                        | TraceKind::HostRequestFinished
                        | TraceKind::WorkerProcessStarted
                        | TraceKind::WorkerProcessFinished
                )
            })
            .collect();
        if mode < 2 {
            assert!(details.is_empty());
        } else {
            assert_eq!(
                details.iter().map(|e| e.event).collect::<Vec<_>>(),
                vec![
                    TraceKind::HostRequestStarted,
                    TraceKind::WorkerProcessStarted,
                    TraceKind::WorkerProcessFinished,
                    TraceKind::HostRequestFinished
                ]
            );
            assert!(details.iter().all(|e| e.node.is_some() && e.call.is_none()));
            let pending = invocation.trace_host_phase(HostTracePhase::WorkerCleanup);
            let old = invocation.trace_host_phase(HostTracePhase::BrokerExchange);
            c.stop_trace();
            let frozen = serde_json::to_string(&c.read_trace(0, 256).unwrap()).unwrap();
            drop(pending);
            assert_eq!(
                serde_json::to_string(&c.read_trace(0, 256).unwrap()).unwrap(),
                frozen
            );
            c.clear_trace();
            c.start_detailed_trace().unwrap();
            drop(old);
            drop(invocation.trace_host_phase(HostTracePhase::Request));
            assert!(c.read_trace(0, 256).unwrap().events.is_empty());
        }
        assert!(permit.complete(Ok(data())));
        until(|| h.host.usage().physical_jobs == 0);
        c.get_run(&key).unwrap();
    }
}

impl ControlNode for Policy {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.policy",
            1,
            vec![InputPort::optional(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec!["in".into()]
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if config.is_object() {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "test object"))
        }
    }
    fn observe(
        &self,
        _: RunContext<'_>,
        frame: &ControlFrame,
        config: &Value,
    ) -> Result<ControlDecision, CoreError> {
        self.0.send((frame.reason, frame.elapsed_ms)).unwrap();
        if config["mode"] == "timer" {
            if frame.reason == WakeReason::Timer {
                return Ok(ControlDecision::Finish(data()));
            }
            return Ok(ControlDecision::Wait {
                wake_after_ms: Some(Milliseconds::new(5).unwrap()),
            });
        }
        if config["mode"] == "recover"
            && let ControlInput::Resolved(ErrorAwareValue::Error(errors)) = &frame.inputs["in"]
        {
            let mut output = data();
            output.recoveries.push(RecoveryClaim {
                input: "in".into(),
                output: "out".into(),
                causes: errors
                    .causes()
                    .iter()
                    .map(|cause| cause.id.clone())
                    .collect(),
            });
            return Ok(ControlDecision::Finish(output));
        }
        Ok(ControlDecision::Wait {
            wake_after_ms: None,
        })
    }
}
struct Harness {
    native: bool,
    host: ExecutionHost,
    clock: ExecutionClock,
    registry: Registry,
    graph: Graph,
    entered: mpsc::Receiver<Started>,
    started: Vec<Started>,
    observed: mpsc::Receiver<(WakeReason, u64)>,
}
impl Harness {
    fn new(native: bool, slots: u64, queued: u64, mode: Option<&str>, gated: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut profile = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        profile.max_physical_jobs = slots;
        profile.max_queued_jobs = queued;
        profile.retention_ms = Milliseconds::new(if native { 1000 } else { 10 }).unwrap();
        profile.cancel_grace_ms = Milliseconds::new(3).unwrap();
        let host = ExecutionHost::new(
            format!("service-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            profile,
        )
        .unwrap();
        let clock = if native {
            ExecutionClock::native(Duration::ZERO).unwrap()
        } else {
            ExecutionClock::controlled(Duration::ZERO)
        };
        let (send, entered) = mpsc::channel();
        let (seen, observed) = mpsc::channel();
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        registry.register_deferred(Async { send, gated }).unwrap();
        registry.register_control(Policy(seen)).unwrap();
        let mut nodes = vec![json!({"id":"source","kind":"test.async","version":1})];
        let mut edges = vec![];
        if let Some(mode) = mode {
            nodes.push(
                json!({"id":"control","kind":"test.policy","version":1,"config":{"mode":mode}}),
            );
            edges.push(
                json!({"from":{"node":"source","port":"out"},"to":{"node":"control","port":"in"}}),
            );
        }
        let graph = Graph::from_json(&json!({"version":1,"nodes":nodes,"edges":edges}).to_string())
            .unwrap();
        Self {
            native,
            host,
            clock,
            registry,
            graph,
            entered,
            started: vec![],
            observed,
        }
    }
    fn service(&self) -> ExecutionService {
        ExecutionService::with_clock(self.host.clone(), self.clock.clone()).unwrap()
    }
    fn plan(&self) -> ExecutionPlan {
        ExecutionPlan::compile(&self.graph, &self.registry).unwrap()
    }
    fn controller(&self) -> ExecutionController {
        self.service().open_ready(self.plan(), id(1)).unwrap()
    }
    fn request(target: &str) -> StartRequest {
        serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":target,"port":"out"}]})).unwrap()
    }
    fn enter(&mut self) -> Started {
        let start = self.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        self.started.push(start.clone());
        start
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        // All local Controller values are dropped before their Harness. Session close
        // is asynchronous; let this test's own producers acknowledge actual ending.
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.started.extend(self.entered.try_iter());
            for start in &self.started {
                start.permit.finish_cancelled();
                if let Some(release) = &start.release {
                    let _ = release.send(());
                }
            }
            if !self.native {
                let current = self.clock.now().unwrap();
                let _ = self.clock.advance_to(current.saturating_add(ms(20)));
            }
            let used = self.host.usage();
            if used.physical_jobs == 0 && used.active_runs == 0 && used.observers == 0 {
                break;
            }
            assert!(Instant::now() < end, "service test cleanup");
            std::thread::sleep(ms(1));
        }
    }
}

#[test]
fn worker_completion_is_adopted_without_any_status_polling() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("source"))
        .unwrap()
        .key;
    let start = h.enter();
    assert!(start.permit.complete(Ok(data())));
    until(|| h.host.usage().physical_jobs == 0);
    let result = controller.get_run(&key).unwrap().result.unwrap();
    assert_eq!(result.delivery, RunDelivery::Complete);
    assert_eq!(result.ports[0].value["packet"]["root"]["text"], "value😀");
}
#[test]
fn retained_results_expire_without_status_polling() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("source"))
        .unwrap()
        .key;
    let start = h.enter();
    assert!(start.permit.complete(Ok(data())));
    until(|| h.host.usage().physical_jobs == 0 && h.host.usage().active_runs == 0);
    assert_eq!(h.host.usage().retained_runs, 1);
    h.clock.advance_to(ms(10)).unwrap();
    until(|| h.host.usage().retained_runs == 0);
    assert_eq!(controller.get_run(&key).unwrap_err().code, "result-gone");
}
#[test]
fn native_getters_report_elapsed_time_without_changing_the_observation() {
    let mut h = Harness::new(true, 1, 0, Some("wait"), false);
    h.graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":"control","kind":"test.policy","version":1,"config":{"mode":"wait"}}],"edges":[]}).to_string()).unwrap();
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("control"))
        .unwrap()
        .key;
    assert_eq!(
        h.observed.recv_timeout(Duration::from_secs(5)).unwrap().0,
        WakeReason::Started
    );
    let before = controller.get_run(&key).unwrap();
    std::thread::sleep(ms(10));
    let after = controller.get_run(&key).unwrap();
    assert!(after.elapsed_ms > before.elapsed_ms);
    assert_eq!(after.event_seq, before.event_seq);
    assert!(h.observed.try_recv().is_err());
    controller.cancel_run(&key).unwrap();
}
#[test]
fn native_observer_timer_runs_without_get_run_or_manual_tick() {
    let mut h = Harness::new(true, 1, 0, Some("timer"), false);
    h.graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":"control","kind":"test.policy","version":1,"config":{"mode":"timer"}}],"edges":[]}).to_string()).unwrap();
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("control"))
        .unwrap()
        .key;
    assert_eq!(
        h.observed.recv_timeout(Duration::from_secs(5)).unwrap().0,
        WakeReason::Started
    );
    assert_eq!(
        h.observed.recv_timeout(Duration::from_secs(5)).unwrap().0,
        WakeReason::Timer
    );
    let snapshot = controller.get_run(&key).unwrap();
    assert_eq!(snapshot.result.unwrap().delivery, RunDelivery::Complete);
    assert_eq!(h.host.usage().physical_jobs, 0);
}
#[test]
fn controlled_observer_timer_cancels_a_producer_that_has_really_started() {
    let mut h = Harness::new(false, 1, 0, Some("timer"), false);
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("control"))
        .unwrap()
        .key;
    let start = h.enter();
    assert_eq!(
        h.observed.recv_timeout(Duration::from_secs(5)).unwrap().0,
        WakeReason::Started
    );
    h.clock.advance_to(ms(5)).unwrap();
    assert_eq!(
        h.observed.recv_timeout(Duration::from_secs(5)).unwrap().0,
        WakeReason::Timer
    );
    until(|| start.cancel.is_cancelled());
    assert_eq!(
        controller.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
    assert_eq!(h.host.usage().physical_jobs, 1);
}
#[test]
fn controlled_clock_expiry_and_physical_cleanup_are_independent_of_polling() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let controller = h.controller();
    let mut request = Harness::request("source");
    request.run_budget_ms = Some(Milliseconds::new(5).unwrap());
    let key = controller.start_run(request).unwrap().key;
    let start = h.enter();
    h.clock.advance_to(ms(5)).unwrap();
    until(|| start.cancel.is_cancelled());
    let expired = controller.get_run(&key).unwrap();
    assert_eq!(expired.result.unwrap().reason, TerminalReason::Expired);
    assert_eq!(expired.elapsed_ms, 5);
    assert_eq!(h.host.usage().physical_jobs, 1);
    h.clock.advance_to(ms(8)).unwrap();
    assert_eq!(controller.get_run(&key).unwrap().physical.stalled, 1);
    assert!(start.permit.finish_cancelled());
    until(|| h.host.usage().physical_jobs == 0);
    assert_eq!(controller.get_run(&key).unwrap().physical.stalled, 0);
}
#[test]
fn cancelling_a_blocked_start_is_responsive_and_permit_ack_does_not_end_the_thread() {
    let mut h = Harness::new(false, 1, 0, None, true);
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("source"))
        .unwrap()
        .key;
    let start = h.enter();
    let cancelled = controller.cancel_run(&key).unwrap();
    assert_eq!(cancelled.result.unwrap().reason, TerminalReason::Cancelled);
    assert_eq!(cancelled.physical.cancel_requested, 1);
    assert!(start.permit.finish_cancelled());
    assert_eq!(h.host.usage().physical_jobs, 1);
    assert_eq!(
        controller.get_run(&key).unwrap().physical.cancel_requested,
        1
    );
    start.release.as_ref().unwrap().send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
}
#[test]
fn only_the_last_controller_clone_closes_the_session() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let controller = h.controller();
    let clone = controller.clone();
    let key = controller
        .start_run(Harness::request("source"))
        .unwrap()
        .key;
    let start = h.enter();
    drop(controller);
    assert!(!start.cancel.is_cancelled());
    assert_eq!(clone.get_run(&key).unwrap().state, RunPhase::Running);
    drop(clone);
    until(|| start.cancel.is_cancelled());
    assert!(start.permit.finish_cancelled());
    until(|| h.host.usage().physical_jobs == 0);
}
#[test]
fn dropping_all_handles_then_reopening_reuses_the_old_budget_and_completion_receiver() {
    let mut h = Harness::new(false, 1, 0, None, true);
    let service = h.service();
    let controller = service.open_ready(h.plan(), id(1)).unwrap();
    let old_epoch = controller.session_epoch().clone();
    controller.start_run(Harness::request("source")).unwrap();
    let start = h.enter();
    drop(controller);
    drop(service);
    until(|| start.cancel.is_cancelled());
    let service = h.service();
    let next = service.open_ready(h.plan(), id(1)).unwrap();
    assert_ne!(next.session_epoch(), &old_epoch);
    assert_eq!(
        next.start_run(Harness::request("source")).unwrap_err().code,
        "busy"
    );
    assert_eq!(h.host.usage().physical_jobs, 1);
    assert!(start.permit.finish_cancelled());
    assert_eq!(h.host.usage().physical_jobs, 1);
    start.release.as_ref().unwrap().send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    let key = next.start_run(Harness::request("source")).unwrap().key;
    let running = h.enter();
    assert_eq!(running.permit.key(), &key);
}
#[test]
fn four_sessions_are_shared_across_service_handles_and_drop_frees_a_session_slot() {
    let h = Harness::new(false, 1, 0, None, false);
    let first = h.service();
    let second = h.service();
    assert!(first.same_service(&second));
    let mut controllers = vec![];
    for _ in 0..4 {
        controllers.push(first.open_ready(h.plan(), id(1)).unwrap());
    }
    assert_eq!(second.open_ready(h.plan(), id(1)).unwrap_err().code, "busy");
    controllers.pop();
    let next = second.open_ready(h.plan(), id(1)).unwrap();
    assert!(
        controllers
            .iter()
            .all(|controller| controller.session_epoch() != next.session_epoch())
    );
}
#[test]
fn another_controllers_epoch_and_wrong_full_keys_are_rejected_without_cancelling_work() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let service = h.service();
    let a = service.open_ready(h.plan(), id(1)).unwrap();
    let b = service.open_ready(h.plan(), id(1)).unwrap();
    let key = a.start_run(Harness::request("source")).unwrap().key;
    let start = h.enter();
    assert_eq!(b.get_run(&key).unwrap_err().code, "session-conflict");
    assert_eq!(b.cancel_run(&key).unwrap_err().code, "session-conflict");
    let mut wrong = key.clone();
    wrong.delivery_generation = id(99);
    assert_eq!(a.cancel_run(&wrong).unwrap_err().code, "run-key-conflict");
    assert!(!start.cancel.is_cancelled());
}
#[test]
fn read_requests_do_not_change_sequence_or_observe_an_unchanged_pending_frame() {
    let mut h = Harness::new(false, 1, 0, Some("wait"), false);
    let controller = h.controller();
    let key = controller
        .start_run(Harness::request("control"))
        .unwrap()
        .key;
    let _start = h.enter();
    assert_eq!(
        h.observed.recv_timeout(Duration::from_secs(5)).unwrap().0,
        WakeReason::Started
    );
    let sequence = controller.get_run(&key).unwrap().event_seq;
    let state = controller.control_state().unwrap();
    for _ in 0..100 {
        assert_eq!(controller.get_run(&key).unwrap().event_seq, sequence);
        assert_eq!(controller.control_state().unwrap(), state);
    }
    assert!(h.observed.try_recv().is_err());
}

#[test]
fn empty_session_prepares_inertly_and_activates_its_first_plan_without_fabricated_history() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let service = h.service();
    let controller = service.open_empty().unwrap();
    let epoch = controller.session_epoch().clone();
    let usage = h.host.usage();
    let initial = controller.control_state().unwrap();
    assert_eq!(initial.revision, 0);
    assert_eq!(initial.active_plan_version, None);
    assert!(!initial.has_prepared && !initial.can_rollback);
    assert_eq!(initial.latest_input_revision, None);
    assert_eq!(initial.delivery_generation, id(1));
    assert_eq!(
        controller.plan_version().unwrap_err().code,
        "no-active-plan"
    );
    assert_eq!(
        controller
            .start_run(Harness::request("source"))
            .unwrap_err()
            .code,
        "no-active-plan"
    );
    assert_eq!(
        controller.invalidate_input(id(1), 1).unwrap_err().code,
        "no-active-plan"
    );
    assert_eq!(
        controller.get_start(id(1)).unwrap_err().code,
        "start-not-seen"
    );
    let key = diy_ime2_core::execution_control::RunKey {
        session_epoch: epoch.clone(),
        plan_version: id(1),
        run_id: id(1),
        input_revision: None,
        delivery_generation: id(1),
    };
    assert_eq!(controller.get_run(&key).unwrap_err().code, "run-unknown");
    assert_eq!(controller.cancel_run(&key).unwrap_err().code, "run-unknown");
    let other = service.open_empty().unwrap();
    assert_eq!(other.get_run(&key).unwrap_err().code, "session-conflict");
    assert_eq!(other.cancel_run(&key).unwrap_err().code, "session-conflict");
    assert!(!controller.activate().unwrap());
    assert!(!controller.rollback().unwrap());
    assert_eq!(controller.control_state().unwrap(), initial);
    controller.prepare(h.plan()).unwrap();
    let prepared = controller.control_state().unwrap();
    assert_eq!(prepared.revision, 1);
    assert!(prepared.has_prepared);
    assert_eq!(prepared.active_plan_version, None);
    assert!(!controller.rollback().unwrap());
    assert_eq!(controller.control_state().unwrap(), prepared);
    assert_eq!(h.host.usage(), usage);
    assert!(h.entered.try_recv().is_err());
    assert!(controller.activate().unwrap());
    assert_eq!(controller.session_epoch(), &epoch);
    let activated = controller.control_state().unwrap();
    assert_eq!(activated.revision, 2);
    assert_eq!(activated.active_plan_version, Some(id(1)));
    assert!(!activated.has_prepared && !activated.can_rollback);
    assert!(!controller.rollback().unwrap());
    assert_eq!(controller.control_state().unwrap(), activated);
    // A rejection before activation did not accept key 1. Run IDs are allocated
    // process-wide, so concurrent tests may have already consumed other IDs.
    let key = controller
        .start_run(Harness::request("source"))
        .unwrap()
        .key;
    assert_eq!(controller.get_start(id(1)).unwrap(), key);
    let start = h.enter();
    controller.prepare(h.plan()).unwrap();
    assert!(!start.cancel.is_cancelled());
    assert!(controller.activate().unwrap());
    assert!(start.cancel.is_cancelled());
    let switched = controller.control_state().unwrap();
    assert_eq!(switched.revision, 4);
    assert_eq!(switched.active_plan_version, Some(id(2)));
    assert!(switched.can_rollback);
    assert!(controller.rollback().unwrap());
    let restored = controller.control_state().unwrap();
    assert_eq!(restored.revision, 5);
    assert_eq!(restored.active_plan_version, Some(id(3)));
    assert_eq!(
        restored.delivery_generation,
        key.delivery_generation
            .checked_next()
            .unwrap()
            .checked_next()
            .unwrap()
    );
    assert!(!restored.can_rollback);
    assert_eq!(
        controller.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Cancelled
    );
}

#[test]
fn empty_session_activation_cannot_bypass_an_old_producers_shared_budget() {
    let mut h = Harness::new(false, 1, 0, None, true);
    let service = h.service();
    let first = service.open_ready(h.plan(), id(1)).unwrap();
    first.start_run(Harness::request("source")).unwrap();
    let start = h.enter();
    drop(first);
    drop(service);
    until(|| start.cancel.is_cancelled());
    let service = h.service();
    assert!(service.clock().same_clock(&h.clock));
    assert_eq!(service.resource_domain(), h.host.resource_domain());
    let next = service.open_empty().unwrap();
    next.prepare(h.plan()).unwrap();
    next.activate().unwrap();
    assert_eq!(
        next.start_run(Harness::request("source")).unwrap_err().code,
        "busy"
    );
    assert_eq!(h.host.usage().physical_jobs, 1);
    assert!(start.permit.finish_cancelled());
    assert_eq!(h.host.usage().physical_jobs, 1);
    start.release.as_ref().unwrap().send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    let key = next.start_run(Harness::request("source")).unwrap().key;
    assert_eq!(h.enter().permit.key(), &key);
}

#[test]
fn empty_and_ready_sessions_share_four_slots_and_last_clone_releases_the_slot() {
    let h = Harness::new(false, 1, 0, None, false);
    let service = h.service();
    let mut empty = vec![];
    for _ in 0..4 {
        empty.push(service.open_empty().unwrap());
    }
    assert_eq!(service.open_empty().unwrap_err().code, "busy");
    assert_eq!(
        service.open_ready(h.plan(), id(1)).unwrap_err().code,
        "busy"
    );
    let clone = empty.pop().unwrap();
    let last = clone.clone();
    drop(clone);
    assert_eq!(service.open_empty().unwrap_err().code, "busy");
    drop(last);
    let ready = service.open_ready(h.plan(), id(1)).unwrap();
    assert_eq!(ready.control_state().unwrap().revision, 0);
    assert_eq!(service.open_empty().unwrap_err().code, "busy");
}
#[test]
fn input_replays_and_plan_switches_keep_the_same_service_contract() {
    let mut h = Harness::new(false, 2, 2, None, false);
    let controller = h.controller();
    let request = Harness::request("source");
    let key = controller.start_run(request.clone()).unwrap().key;
    let start = h.enter();
    let replay = controller.start_run(request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.key, key);
    assert_eq!(controller.get_start(id(1)).unwrap(), key);
    assert!(h.entered.try_recv().is_err());
    controller.prepare(h.plan()).unwrap();
    assert!(!start.cancel.is_cancelled());
    assert!(controller.activate().unwrap());
    assert!(start.cancel.is_cancelled());
    assert_eq!(
        controller.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Cancelled
    );
    assert!(controller.rollback().unwrap());
    assert_eq!(controller.plan_version().unwrap(), id(3));
}
#[test]
fn an_existing_service_rejects_a_different_clock_instead_of_resetting_its_domain() {
    let h = Harness::new(false, 1, 0, None, false);
    let service = h.service();
    assert_eq!(
        ExecutionService::with_clock(h.host.clone(), ExecutionClock::controlled(Duration::ZERO))
            .unwrap_err()
            .code,
        "clock-conflict"
    );
    let reused = ExecutionService::new(h.host.clone()).unwrap();
    assert!(service.same_service(&reused));
}
#[test]
fn os_epochs_are_canonical_independent_and_start_key_issuance_is_shared_by_clones() {
    let h = Harness::new(false, 1, 0, None, false);
    let service = h.service();
    let first = service.open_ready(h.plan(), id(1)).unwrap();
    let other = service.open_ready(h.plan(), id(1)).unwrap();
    assert_eq!(first.session_epoch().as_str().len(), 32);
    assert!(
        first
            .session_epoch()
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_ne!(first.session_epoch(), other.session_epoch());
    let clone = first.clone();
    assert_eq!(first.next_start_key().unwrap(), id(1));
    assert_eq!(clone.next_start_key().unwrap(), id(2));
    assert_eq!(other.next_start_key().unwrap(), id(1));
}

#[test]
fn timeline_distinguishes_result_ready_from_authenticated_physical_ending() {
    use diy_ime2_core::execution_control::TraceKind;
    let mut h = Harness::new(false, 1, 0, None, true);
    let controller = h.controller();
    assert!(!controller.read_trace(0, 256).unwrap().recording);
    controller.start_trace().unwrap();
    let key = controller
        .start_run(Harness::request("source"))
        .unwrap()
        .key;
    let start = h.enter();
    assert!(start.permit.complete(Ok(data())));
    until(|| controller.get_run(&key).unwrap().state == RunPhase::Terminal);
    let before = controller.read_trace(0, 256).unwrap();
    assert!(
        before
            .events
            .iter()
            .any(|e| e.event == TraceKind::CoreResultReady)
    );
    assert!(
        !before
            .events
            .iter()
            .any(|e| matches!(e.event, TraceKind::CallEnded | TraceKind::PhysicalEnded))
    );
    start.release.unwrap().send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    // Serialize behind the owner so its physical-count confirmation is complete.
    controller.get_run(&key).unwrap();
    let after = controller.read_trace(0, 256).unwrap();
    let kinds: Vec<_> = after.events.iter().map(|e| e.event).collect();
    assert!(kinds.contains(&TraceKind::NodeStarted));
    assert!(kinds.contains(&TraceKind::NodeResolved));
    assert!(kinds.contains(&TraceKind::CallEnded));
    assert!(kinds.contains(&TraceKind::PhysicalEnded));
    let ready = kinds
        .iter()
        .position(|e| *e == TraceKind::CoreResultReady)
        .unwrap();
    let ended = kinds
        .iter()
        .position(|e| *e == TraceKind::PhysicalEnded)
        .unwrap();
    assert!(ready < ended);
    let json = serde_json::to_string(&after).unwrap();
    assert!(!json.contains("value😀"));
    assert!(!json.contains(key.session_epoch.as_str()));
}
#[test]
fn timeline_clear_cannot_be_resurrected_by_running_work_and_does_not_cancel_it() {
    let mut h = Harness::new(false, 1, 0, None, false);
    let c = h.controller();
    c.start_trace().unwrap();
    let key = c.start_run(Harness::request("source")).unwrap().key;
    let job = h.enter();
    c.clear_trace();
    assert!(!job.cancel.is_cancelled());
    assert!(job.permit.complete(Ok(data())));
    until(|| h.host.usage().physical_jobs == 0);
    assert_eq!(
        c.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
    let r = c.read_trace(0, 256).unwrap();
    assert!(r.events.is_empty());
    assert!(r.recording_id.is_none());
}

#[test]
fn timeline_replayed_start_is_not_recorded_as_a_new_run_and_overlaps_remain_separate() {
    use diy_ime2_core::execution_control::TraceKind;
    let mut h = Harness::new(false, 2, 0, None, false);
    let c = h.controller();
    c.start_trace().unwrap();
    let one = c.start_run(Harness::request("source")).unwrap().key;
    let first = h.enter();
    assert_eq!(c.start_run(Harness::request("source")).unwrap().key, one);
    let mut request = Harness::request("source");
    request.start_key = id(2);
    let two = c.start_run(request).unwrap().key;
    let second = h.enter();
    assert!(second.permit.complete(Ok(data())));
    until(|| c.get_run(&two).unwrap().state == RunPhase::Terminal);
    assert!(first.permit.complete(Ok(data())));
    until(|| h.host.usage().physical_jobs == 0);
    c.get_run(&one).unwrap();
    let r = c.read_trace(0, 256).unwrap();
    let runs: Vec<_> = r
        .events
        .iter()
        .filter(|e| e.event == TraceKind::RunAccepted)
        .map(|e| e.run)
        .collect();
    assert_eq!(runs, vec![1, 2]);
    let calls: Vec<_> = r
        .events
        .iter()
        .filter(|e| e.event == TraceKind::NodeStarted)
        .map(|e| (e.run, e.call))
        .collect();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].1, calls[1].1);
    assert!(
        r.events
            .iter()
            .any(|e| e.run == 1 && e.event == TraceKind::CoreResultReady)
    );
}
#[test]
fn timeline_retention_expiry_does_not_erase_pending_physical_ending() {
    use diy_ime2_core::execution_control::TraceKind;
    let mut h = Harness::new(false, 1, 0, None, true);
    let c = h.controller();
    c.start_trace().unwrap();
    let key = c.start_run(Harness::request("source")).unwrap().key;
    let job = h.enter();
    assert!(job.permit.complete(Ok(data())));
    until(|| c.get_run(&key).unwrap().state == RunPhase::Terminal);
    h.clock.advance_to(ms(20)).unwrap();
    until(|| h.host.usage().retained_runs == 0);
    assert_eq!(c.get_run(&key).unwrap_err().code, "result-gone");
    job.release.unwrap().send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    let _ = c.get_run(&key);
    let r = c.read_trace(0, 256).unwrap();
    assert!(r.events.iter().any(|e| e.event == TraceKind::CallEnded));
    assert!(r.events.iter().any(|e| e.event == TraceKind::PhysicalEnded));
}
