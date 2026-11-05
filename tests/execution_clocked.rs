use diy_ime2_core::{
    Candidate, Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue, RecoveryClaim},
    execution_control::{
        CancellationToken, CompletionPermit, CompletionWakeStatus, ControlDecision, ControlFrame,
        ControlInput, ControlNode, DecimalId, DeferredNode, ExecutionClock, ExecutionHost,
        ExecutionPump, Milliseconds, OwnedInvocation, ResourcePolicy, RunDelivery, RunKey,
        RunPhase, SessionEpoch, StartRequest, TerminalReason, WakeReason,
    },
    graph::{ExecutionPlan, Graph},
    node::{CoreError, InputPort, NodeDefinition, Port, Registry, RunContext},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn epoch(n: u64) -> SessionEpoch {
    SessionEpoch::new(format!("{n:032x}")).unwrap()
}
fn data() -> ErrorAwareExecution {
    named_data("data")
}
fn named_data(text: &str) -> ErrorAwareExecution {
    ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: text.into(),
                meta: None,
            }))),
        )]
        .into(),
    )
}
#[derive(Clone)]
struct Started {
    permit: CompletionPermit,
    cancel: CancellationToken,
}
struct Async(mpsc::Sender<Started>);
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
        self.0.send(Started { permit, cancel }).unwrap();
        Ok(())
    }
}
type Log = Arc<Mutex<Vec<(String, WakeReason, u64)>>>;
struct Policy {
    clock: ExecutionClock,
    log: Log,
}
impl ControlNode for Policy {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.policy",
            1,
            vec![
                InputPort::optional(Port::packet("in")),
                InputPort::optional(Port::packet("a")),
                InputPort::optional(Port::packet("b")),
            ],
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
        ctx: RunContext<'_>,
        frame: &ControlFrame,
        config: &Value,
    ) -> Result<ControlDecision, CoreError> {
        self.log
            .lock()
            .unwrap()
            .push((ctx.node_id.into(), frame.reason, frame.elapsed_ms));
        if let Some(to) = config["advanceTo"].as_u64()
            && (!config["advanceOnInput"].as_bool().unwrap_or(false)
                || frame.reason == WakeReason::Input)
            && self.clock.now()? < ms(to)
        {
            self.clock.advance_to(ms(to))?;
        }
        match config["mode"].as_str().unwrap_or("wait") {
            "wait-input" => {
                if matches!(
                    &frame.inputs["in"],
                    ControlInput::Resolved(ErrorAwareValue::Data(_))
                ) {
                    Ok(ControlDecision::Finish(named_data(ctx.node_id)))
                } else {
                    Ok(ControlDecision::Wait {
                        wake_after_ms: None,
                    })
                }
            }
            "finish" => Ok(ControlDecision::Finish(named_data(ctx.node_id))),
            "timer" if frame.reason == WakeReason::Timer => {
                Ok(ControlDecision::Finish(named_data(ctx.node_id)))
            }
            "choose" => {
                // At a tie this policy prefers b. Core must still let it observe
                // the earlier timer result a before adopting the delayed b.
                for port in ["b", "a"] {
                    if let ControlInput::Resolved(ErrorAwareValue::Data(packet)) =
                        &frame.inputs[port]
                    {
                        return Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                            [("out".into(), ErrorAwareValue::Data(Arc::clone(packet)))].into(),
                        )));
                    }
                }
                Ok(ControlDecision::Wait {
                    wake_after_ms: None,
                })
            }
            "recover" => match &frame.inputs["in"] {
                ControlInput::Resolved(ErrorAwareValue::Error(errors)) => {
                    let mut result = data();
                    result.recoveries.push(RecoveryClaim {
                        input: "in".into(),
                        output: "out".into(),
                        causes: errors
                            .causes()
                            .iter()
                            .map(|cause| cause.id.clone())
                            .collect(),
                    });
                    Ok(ControlDecision::Finish(result))
                }
                _ => Ok(ControlDecision::Finish(data())),
            },
            _ => Ok(ControlDecision::Wait {
                wake_after_ms: Some(Milliseconds::new(5).unwrap()),
            }),
        }
    }
}
fn policy(id: &str, mode: &str, jump: Option<u64>) -> Value {
    let mut config = json!({"mode":mode});
    if let Some(jump) = jump {
        config["advanceTo"] = json!(jump);
    }
    json!({"id":id,"kind":"test.policy","version":1,"config":config})
}
struct Harness {
    host: ExecutionHost,
    clock: ExecutionClock,
    pump: ExecutionPump,
    registry: Registry,
    graph: Graph,
    entered: mpsc::Receiver<Started>,
    started: Vec<Started>,
    keys: Vec<RunKey>,
    log: Log,
}
impl Harness {
    fn new(nodes: Vec<Value>, edges: Vec<Value>) -> Self {
        Self::with_observers(nodes, edges, 8)
    }
    fn with_observers(nodes: Vec<Value>, edges: Vec<Value>, observers: u64) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut profile = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        profile.retention_ms = Milliseconds::new(10).unwrap();
        profile.cancel_grace_ms = Milliseconds::new(3).unwrap();
        profile.max_observers = observers;
        if observers > 8 {
            profile.max_result_bytes_per_run = 1_048_576;
            profile.max_retained_bytes = 4_194_304;
        }
        let host = ExecutionHost::new(
            format!("clocked-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            profile,
        )
        .unwrap();
        let clock = ExecutionClock::controlled(Duration::ZERO);
        let (send, entered) = mpsc::channel();
        let log = Arc::new(Mutex::new(vec![]));
        let mut registry = Registry::new();
        registry.register_deferred(Async(send)).unwrap();
        registry
            .register_control(Policy {
                clock: clock.clone(),
                log: Arc::clone(&log),
            })
            .unwrap();
        let graph = Graph::from_json(&json!({"version":1,"nodes":nodes,"edges":edges}).to_string())
            .unwrap();
        let mut pump = ExecutionPump::with_clock(host.clone(), clock.clone()).unwrap();
        pump.open_session(
            host.clone(),
            epoch(1),
            ExecutionPlan::compile(&graph, &registry).unwrap(),
            id(1),
        )
        .unwrap();
        Self {
            host,
            clock,
            pump,
            registry,
            graph,
            entered,
            started: vec![],
            keys: vec![],
            log,
        }
    }
    fn source(mode: &str, jump: Option<u64>) -> Self {
        Self::new(
            vec![
                json!({"id":"source","kind":"test.async","version":1}),
                policy("control", mode, jump),
            ],
            vec![
                json!({"from":{"node":"source","port":"out"},"to":{"node":"control","port":"in"}}),
            ],
        )
    }
    fn request(targets: &[&str]) -> StartRequest {
        serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":targets.iter().map(|node|json!({"node":node,"port":"out"})).collect::<Vec<_>>()})).unwrap()
    }
    fn start(&mut self, request: StartRequest) -> RunKey {
        let key = self.pump.start(&epoch(1), request).unwrap().key;
        self.keys.push(key.clone());
        key
    }
    fn enter(&mut self) -> Started {
        let start = self.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        self.started.push(start.clone());
        start
    }
    fn wait_for_source_cancellation_or_reclamation(&mut self) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump.tick().unwrap();
            if let Ok(start) = self.entered.try_recv() {
                assert!(start.cancel.is_cancelled());
                self.started.push(start);
                break;
            }
            if self.host.usage().physical_jobs == 0 {
                break;
            }
            assert!(
                Instant::now() < end,
                "expired worker did not start or reclaim"
            );
            std::thread::sleep(ms(1));
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        for key in &self.keys {
            let _ = self.pump.cancel(key);
        }
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.started.extend(self.entered.try_iter());
            for start in &self.started {
                start.permit.finish_cancelled();
            }
            let _ = self.pump.tick();
            if self.host.usage().physical_jobs == 0 {
                break;
            }
            assert!(Instant::now() < end, "clocked test cleanup");
            std::thread::yield_now();
        }
    }
}

#[test]
fn native_clock_identity_and_controlled_clock_validation_are_separate() {
    let clock = ExecutionClock::native(ms(1)).unwrap();
    let first = clock.now().unwrap();
    assert!(clock.now().unwrap() >= first);
    assert!(clock.same_clock(&clock.clone()));
    assert!(!clock.same_clock(&ExecutionClock::controlled(ms(1))));
    assert_eq!(clock.advance_to(ms(2)).unwrap_err().code, "invalid-clock");
    let controlled = ExecutionClock::controlled(ms(2));
    assert_eq!(
        controlled.advance_to(ms(1)).unwrap_err().code,
        "invalid-clock"
    );
    assert_eq!(controlled.now().unwrap(), ms(2));
}
#[test]
fn clock_wake_does_not_make_state_reads_drive_expiry_or_policy() {
    let mut h = Harness::source("wait", None);
    let mut request = Harness::request(&["control"]);
    request.run_budget_ms = Some(Milliseconds::new(5).unwrap());
    let key = h.start(request);
    let _start = h.enter();
    // Drain preexisting start-return wake hints without adopting any new Data.
    h.pump.tick().unwrap();
    let _ = h.pump.wake_signal().wait(Some(Duration::ZERO)).unwrap();
    let before = serde_json::to_value(h.pump.get_run(&key).unwrap()).unwrap();
    h.clock.advance_to(ms(5)).unwrap();
    assert_eq!(
        h.pump.wake_signal().wait(Some(Duration::ZERO)).unwrap(),
        CompletionWakeStatus::Notified
    );
    assert_eq!(
        serde_json::to_value(h.pump.get_run(&key).unwrap()).unwrap(),
        before
    );
    assert_eq!(h.log.lock().unwrap().len(), 1);
    h.pump.tick().unwrap();
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Expired
    );
    assert_eq!(h.pump.advance_to(ms(6)).unwrap_err().code, "invalid-clock");
}
#[test]
fn control_finishing_after_its_run_deadline_cannot_backdate_success() {
    let mut h = Harness::source("finish", Some(10));
    let mut request = Harness::request(&["control"]);
    request.run_budget_ms = Some(Milliseconds::new(5).unwrap());
    let key = h.start(request);
    h.wait_for_source_cancellation_or_reclamation();
    let snapshot = h.pump.get_run(&key).unwrap();
    assert_eq!(snapshot.result.unwrap().reason, TerminalReason::Expired);
    assert_eq!(snapshot.elapsed_ms, 5);
    assert_eq!(h.host.usage().observers, 0);
    assert_eq!(h.pump.now(), ms(10));
}
#[test]
fn wait_is_anchored_after_callback_and_does_not_reobserve_unchanged_frames() {
    let mut h = Harness::source("wait", Some(10));
    let key = h.start(Harness::request(&["control"]));
    let _start = h.enter();
    assert_eq!(h.pump.next_deadline(), Some(ms(15)));
    assert_eq!(h.pump.get_run(&key).unwrap().state, RunPhase::Running);
    h.pump.tick().unwrap();
    assert_eq!(h.log.lock().unwrap().len(), 1);
    h.clock.advance_to(ms(14)).unwrap();
    h.pump.tick().unwrap();
    assert_eq!(h.log.lock().unwrap().len(), 1);
    h.clock.advance_to(ms(15)).unwrap();
    h.pump.tick().unwrap();
    assert_eq!(h.log.lock().unwrap()[1].1, WakeReason::Timer);
    assert_eq!(h.pump.next_deadline(), Some(ms(20)));
}
#[test]
fn a_job_expiring_during_control_preparation_replaces_the_stale_decision() {
    let mut h = Harness::source("recover", Some(10));
    let mut request = Harness::request(&["control"]);
    request.job_budgets =
        serde_json::from_value(json!([{"nodeId":"source","budgetMs":5}])).unwrap();
    let key = h.start(request);
    // The deadline may win before callback start. Observe either the real
    // producer's cancellation or its authenticated never-started reclamation.
    h.wait_for_source_cancellation_or_reclamation();
    let result = h.pump.get_run(&key).unwrap().result.unwrap();
    assert_eq!(result.delivery, RunDelivery::Recovered);
    assert_eq!(
        result.causes[0]["diagnostic"]["error"]["code"],
        "deadline-exceeded"
    );
    let log = h.log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].1, WakeReason::Started);
    assert_eq!(log[1].1, WakeReason::Input);
}
#[test]
fn another_observers_due_timer_is_processed_before_the_outer_decision_commits() {
    let mut h = Harness::new(
        vec![policy("a", "timer", None), policy("b", "finish", Some(10))],
        vec![],
    );
    let key = h.start(Harness::request(&["a", "b"]));
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Completed
    );
    assert_eq!(h.pump.get_run(&key).unwrap().elapsed_ms, 10);
    let log = h.log.lock().unwrap();
    assert_eq!(
        log.iter()
            .map(|(node, reason, time)| (node.as_str(), *reason, *time))
            .collect::<Vec<_>>(),
        vec![
            ("a", WakeReason::Started, 0),
            ("b", WakeReason::Started, 0),
            // Its timer became due at 5; the Core callback was busy until 10.
            // Native elapsed time must not pretend that policy ran in the past.
            ("a", WakeReason::Timer, 10)
        ]
    );
}
#[test]
fn nested_observer_deadlines_do_not_resurrect_a_run_expired_during_callbacks() {
    let mut h = Harness::new(
        vec![policy("a", "timer", None), policy("b", "finish", Some(10))],
        vec![],
    );
    let mut request = Harness::request(&["a", "b"]);
    request.run_budget_ms = Some(Milliseconds::new(7).unwrap());
    let key = h.start(request);
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Expired
    );
    assert_eq!(h.host.usage().observers, 0);
    assert_eq!(h.pump.now(), ms(10));
}
#[test]
fn retention_expiry_remains_a_wake_candidate_after_all_nodes_complete() {
    let mut h = Harness::new(vec![policy("only", "finish", None)], vec![]);
    let key = h.start(Harness::request(&["only"]));
    assert_eq!(h.pump.next_deadline(), Some(ms(10)));
    h.clock.advance_to(ms(10)).unwrap();
    assert!(h.pump.get_run(&key).is_ok());
    h.pump.tick().unwrap();
    assert_eq!(h.pump.get_run(&key).unwrap_err().code, "result-gone");
    assert_eq!(h.pump.next_deadline(), None);
}
#[test]
fn all_sessions_share_the_clock_without_read_side_effects() {
    let mut h = Harness::source("wait", None);
    h.pump
        .open_session(
            h.host.clone(),
            epoch(2),
            ExecutionPlan::compile(&h.graph, &h.registry).unwrap(),
            id(1),
        )
        .unwrap();
    let first = h.start(Harness::request(&["control"]));
    let _a = h.enter();
    let second = h
        .pump
        .start(&epoch(2), Harness::request(&["control"]))
        .unwrap()
        .key;
    h.keys.push(second.clone());
    let _b = h.enter();
    h.clock.advance_to(ms(3)).unwrap();
    assert_eq!(h.pump.get_run(&first).unwrap().elapsed_ms, 0);
    h.pump.tick().unwrap();
    assert_eq!(h.pump.get_run(&first).unwrap().elapsed_ms, 3);
    assert_eq!(h.pump.get_run(&second).unwrap().elapsed_ms, 3);
    assert_eq!(h.log.lock().unwrap().len(), 2);
}

#[test]
fn clock_rechecks_queue_many_prepared_observers_without_recursive_policy_calls() {
    let count = 128u64;
    let nodes = (0..count)
        .map(|n| {
            policy(
                &format!("n{n:04}"),
                if n == 0 { "timer" } else { "finish" },
                (n > 0).then_some(n * 5),
            )
        })
        .collect();
    let mut h = Harness::with_observers(nodes, vec![], count);
    let names: Vec<_> = (0..count).map(|n| format!("n{n:04}")).collect();
    let targets: Vec<_> = names.iter().map(String::as_str).collect();
    let key = h.start(Harness::request(&targets));
    assert_eq!(
        h.pump.get_run(&key).unwrap().result.unwrap().delivery,
        RunDelivery::Complete
    );
    assert_eq!(h.host.usage().observers, 0);
    assert_eq!(h.log.lock().unwrap().len(), count as usize + 1);
}

#[test]
fn a_due_timer_and_its_downstream_observer_preempt_a_delayed_prepared_result() {
    let nodes = vec![
        policy("a", "timer", None),
        policy("b", "finish", Some(10)),
        policy("choose", "choose", None),
    ];
    let edges = vec![
        json!({"from":{"node":"a","port":"out"},"to":{"node":"choose","port":"a"}}),
        json!({"from":{"node":"b","port":"out"},"to":{"node":"choose","port":"b"}}),
    ];
    let mut h = Harness::new(nodes, edges);
    let key = h.start(Harness::request(&["choose"]));
    let result = h.pump.get_run(&key).unwrap().result.unwrap();
    let value = &result
        .ports
        .iter()
        .find(|port| port.endpoint.node == "choose")
        .unwrap()
        .value;
    assert_eq!(value["packet"]["root"]["text"], "a");
}

#[test]
fn a_later_node_in_the_same_scan_still_prioritizes_its_due_timer() {
    let mut slow = policy("aSlow", "wait-input", Some(10));
    slow["config"]["advanceOnInput"] = json!(true);
    let nodes = vec![
        json!({"id":"source","kind":"test.async","version":1}),
        slow,
        policy("zTimer", "timer", None),
        policy("choose", "choose", None),
    ];
    let edges = vec![
        json!({"from":{"node":"source","port":"out"},"to":{"node":"aSlow","port":"in"}}),
        json!({"from":{"node":"zTimer","port":"out"},"to":{"node":"choose","port":"a"}}),
        json!({"from":{"node":"aSlow","port":"out"},"to":{"node":"choose","port":"b"}}),
    ];
    let mut h = Harness::new(nodes, edges);
    let key = h.start(Harness::request(&["choose"]));
    let start = h.enter();
    assert!(start.permit.complete(Ok(data())));
    h.pump.tick().unwrap();
    let result = h.pump.get_run(&key).unwrap().result.unwrap();
    assert_eq!(
        result
            .ports
            .iter()
            .find(|port| port.endpoint.node == "choose")
            .unwrap()
            .value["packet"]["root"]["text"],
        "zTimer"
    );
}
