#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue},
    execution_control::{
        CancellationToken, CompletionPermit, DeferredNode, ExecutionClock, ExecutionHost,
        ExecutionService, OwnedInvocation, ResourcePolicy, session::SessionV3,
    },
    node::{CoreError, NodeDefinition, Port, Registry},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use test_nodes::register_builtin_nodes;

#[derive(Clone)]
struct Started {
    permit: CompletionPermit,
    cancel: CancellationToken,
    release: mpsc::Sender<()>,
}
struct Held {
    send: mpsc::Sender<Started>,
    count: Arc<AtomicUsize>,
}
impl DeferredNode for Held {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.held", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        _: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let (release, gate) = mpsc::channel();
        self.count.fetch_add(1, Ordering::SeqCst);
        self.send
            .send(Started {
                permit,
                cancel,
                release,
            })
            .unwrap();
        let _ = gate.recv();
        Ok(())
    }
}
struct Fixture {
    host: ExecutionHost,
    clock: ExecutionClock,
    service: ExecutionService,
    send: mpsc::Sender<Started>,
    entered: mpsc::Receiver<Started>,
    count: Arc<AtomicUsize>,
    started: Vec<Started>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let policy =
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone();
        let mut policy = ResourcePolicy::from_value(policy).unwrap();
        policy.max_physical_jobs = 1;
        policy.max_queued_jobs = 0;
        let host = ExecutionHost::new(
            format!("v3-methods-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            policy,
        )
        .unwrap();
        let clock = ExecutionClock::controlled(Duration::ZERO);
        let service = ExecutionService::with_clock(host.clone(), clock.clone()).unwrap();
        let (send, entered) = mpsc::channel();
        Self {
            host,
            clock,
            service,
            send,
            entered,
            count: Arc::new(AtomicUsize::new(0)),
            started: vec![],
        }
    }
    fn session(&self) -> SessionV3 {
        let mut registry = Registry::new();
        register_builtin_nodes(&mut registry).unwrap();
        registry
            .register_deferred(Held {
                send: self.send.clone(),
                count: self.count.clone(),
            })
            .unwrap();
        SessionV3::new(&self.service, registry).unwrap()
    }
    fn enter(&mut self) -> Started {
        let start = self.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        self.started.push(start.clone());
        start
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        until(|| {
            self.started.extend(self.entered.try_iter());
            for start in &self.started {
                start.permit.finish_cancelled();
                let _ = start.release.send(());
            }
            let _ = self.clock.advance_to(
                self.clock
                    .now()
                    .unwrap()
                    .saturating_add(Duration::from_millis(1000)),
            );
            self.host.usage().physical_jobs == 0 && self.host.usage().active_runs == 0
        });
    }
}
fn until(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "v3 method progress");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn graph() -> Value {
    json!({"version":1,"nodes":[{"id":"source","kind":"test.held","version":1}],"edges":[]})
}
fn send(session: &mut SessionV3, request: Value) -> Value {
    let frame =
        serde_json::to_vec(&json!({"protocol":3,"id":"18446744073709551615","request":request}))
            .unwrap();
    let reply: Value = serde_json::from_slice(&session.reply(&frame).unwrap()).unwrap();
    assert_eq!(reply["id"], "18446744073709551615");
    assert_eq!(reply["protocol"], 3);
    assert_eq!(reply["state"].as_object().unwrap().len(), 6);
    reply
}
fn ok(session: &mut SessionV3, request: Value) -> Value {
    let reply = send(session, request);
    assert_eq!(reply["ok"], true, "{reply}");
    reply
}
fn error(session: &mut SessionV3, request: Value, code: &str) -> Value {
    let reply = send(session, request);
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], code, "{reply}");
    reply
}
fn activate(session: &mut SessionV3, graph: Value) -> Value {
    let prepared = ok(session, json!({"method":"prepare","graph":graph}));
    ok(
        session,
        json!({"method":"activate","token":prepared["data"]["token"]}),
    )
}
fn start(key: &str) -> Value {
    json!({"method":"start-run","startKey":key,"expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}]})
}

#[test]
fn hello_definitions_validation_and_plan_history_match_the_actual_core_state() {
    let h = Fixture::new();
    let mut s = h.session();
    let hello = ok(&mut s, json!({"method":"hello"}));
    assert_eq!(hello["data"]["resourceDomain"], h.host.resource_domain());
    assert_eq!(
        hello["data"]["resourcePolicy"],
        serde_json::to_value(h.host.policy()).unwrap()
    );
    assert_eq!(
        hello["data"]["capabilities"],
        json!(["execution-control-v0.2"])
    );
    assert_eq!(hello["state"]["revision"], "0");
    assert!(hello["state"]["activePlanVersion"].is_null());
    let initial = ok(&mut s, json!({"method":"status"}));
    assert_eq!(
        initial["data"],
        json!({"activeGraph":null,"preparedGraph":null,"preparedOrder":[]})
    );
    let definitions = ok(&mut s, json!({"method":"definitions"}));
    let entries = definitions["data"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .all(|entry| entry["version"] == 2 && entry.get("errorInputs").is_some())
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["definition"]["kind"] == "test.held"
                && entry["executionKind"] == "deferred")
    );
    let validated = ok(&mut s, json!({"method":"validate","graph":graph()}));
    assert_eq!(validated["data"], json!({"valid":true,"order":["source"]}));
    assert_eq!(validated["state"], initial["state"]);
    let first = ok(&mut s, json!({"method":"prepare","graph":graph()}));
    let prepared = ok(&mut s, json!({"method":"status"}));
    assert_eq!(prepared["data"]["preparedGraph"], graph());
    assert_eq!(prepared["data"]["preparedOrder"], json!(["source"]));
    assert_eq!(prepared["state"]["preparedToken"], first["data"]["token"]);
    assert_eq!(prepared["state"]["revision"], "1");
    let failed = error(
        &mut s,
        json!({"method":"prepare","graph":{"version":1,"nodes":[{"id":"bad","kind":"missing","version":1}],"edges":[]}}),
        "unknown-kind",
    );
    assert_eq!(failed["state"], prepared["state"]);
    let second = ok(&mut s, json!({"method":"prepare","graph":graph()}));
    assert_ne!(second["data"]["token"], first["data"]["token"]);
    assert_eq!(
        error(
            &mut s,
            json!({"method":"activate","token":first["data"]["token"]}),
            "stale-preparation"
        )["state"],
        second["state"]
    );
    let active = ok(
        &mut s,
        json!({"method":"activate","token":second["data"]["token"]}),
    );
    assert_eq!(active["state"]["activePlanVersion"], "1");
    assert_eq!(active["state"]["revision"], "3");
    assert_eq!(active["state"]["canRollback"], false);
    error(
        &mut s,
        json!({"method":"rollback","expectedPlanVersion":"1"}),
        "no-rollback",
    );
    let other = json!({"version":1,"nodes":[],"edges":[]});
    activate(&mut s, other.clone());
    assert_eq!(
        ok(&mut s, json!({"method":"status"}))["data"]["activeGraph"],
        other
    );
    error(
        &mut s,
        json!({"method":"rollback","expectedPlanVersion":"1"}),
        "plan-conflict",
    );
    let restored = ok(
        &mut s,
        json!({"method":"rollback","expectedPlanVersion":"2"}),
    );
    assert_eq!(restored["state"]["activePlanVersion"], "3");
    assert_eq!(restored["state"]["canRollback"], false);
    assert_eq!(
        ok(&mut s, json!({"method":"status"}))["data"]["activeGraph"],
        graph()
    );
    assert_eq!(h.count.load(Ordering::SeqCst), 0);
}

#[test]
fn lost_acceptance_is_queryable_and_replay_after_switch_does_not_restart_a_worker() {
    let mut h = Fixture::new();
    let mut s = h.session();
    activate(&mut s, graph());
    let frame = serde_json::to_vec(&json!({"protocol":3,"id":"1","request":start("1")})).unwrap();
    drop(s.reply(&frame).unwrap()); // Deliberately lose the original acceptance.
    let producer = h.enter();
    let found = ok(&mut s, json!({"method":"get-start","startKey":"1"}));
    let key = found["data"]["key"].clone();
    let running = ok(&mut s, json!({"method":"get-run","key":key}));
    assert_eq!(running["data"]["state"], "running");
    assert!(running["data"].get("result").is_none());
    assert_eq!(running["data"]["physical"]["running"], 1);
    let replay = ok(&mut s, start("1"));
    assert_eq!(replay["data"]["state"], "accepted");
    assert_eq!(replay["data"]["key"], key);
    assert_eq!(replay["data"]["replayed"], true);
    let mut changed = start("1");
    changed["runBudgetMs"] = json!(20);
    error(&mut s, changed, "start-conflict");
    activate(&mut s, graph());
    assert!(producer.cancel.is_cancelled());
    assert_eq!(ok(&mut s, start("1"))["data"]["key"], key);
    let cancelled = ok(&mut s, json!({"method":"cancel-run","key":key}));
    assert_eq!(cancelled["data"]["result"]["reason"], "cancelled");
    assert_eq!(cancelled["data"]["physical"]["cancelRequested"], 1);
    let mut wrong = key.clone();
    wrong["deliveryGeneration"] = json!("999");
    error(
        &mut s,
        json!({"method":"get-run","key":wrong}),
        "run-key-conflict",
    );
    assert!(producer.permit.finish_cancelled());
    assert_eq!(h.host.usage().physical_jobs, 1);
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    let ended = ok(&mut s, json!({"method":"get-run","key":key}));
    assert_eq!(ended["data"]["result"], cancelled["data"]["result"]);
    assert_eq!(ended["data"]["elapsedMs"], cancelled["data"]["elapsedMs"]);
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[test]
fn a_new_method_session_rejects_old_keys_and_cannot_reset_the_shared_budget() {
    let mut h = Fixture::new();
    let mut old = h.session();
    activate(&mut old, graph());
    let key = ok(&mut old, start("1"))["data"]["key"].clone();
    let producer = h.enter();
    drop(old);
    until(|| producer.cancel.is_cancelled());
    let mut next = h.session();
    activate(&mut next, graph());
    error(
        &mut next,
        json!({"method":"get-run","key":key}),
        "session-conflict",
    );
    error(&mut next, start("1"), "busy");
    assert_eq!(h.host.usage().physical_jobs, 1);
    producer.permit.finish_cancelled();
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    ok(&mut next, start("1"));
    h.enter();
    assert_eq!(h.count.load(Ordering::SeqCst), 2);
}

#[test]
fn invalid_methods_fields_inputs_and_budgets_do_not_admit_any_work() {
    let mut h = Fixture::new();
    let mut s = h.session();
    activate(&mut s, graph());
    let initial = ok(&mut s, json!({"method":"status"}))["state"].clone();
    let usage = h.host.usage();
    let epoch = ok(&mut s, json!({"method":"hello"}))["data"]["sessionEpoch"].clone();
    let key = json!({"sessionEpoch":epoch,"planVersion":"1","runId":"1","deliveryGeneration":"1"});
    // Supply every required field: a missing-field error must not mask silently
    // ignored unknown fields or allow prepare/start/invalidate side effects.
    for mut request in [
        json!({"method":"hello"}),
        json!({"method":"definitions"}),
        json!({"method":"validate","graph":graph()}),
        json!({"method":"prepare","graph":graph()}),
        json!({"method":"activate","token":"1"}),
        json!({"method":"rollback","expectedPlanVersion":"1"}),
        json!({"method":"status"}),
        start("1"),
        json!({"method":"get-start","startKey":"1"}),
        json!({"method":"get-run","key":key}),
        json!({"method":"cancel-run","key":key}),
        json!({"method":"invalidate-input","expectedPlanVersion":"1","revision":1}),
        json!({"method":"unknown"}),
    ] {
        request["extra"] = json!(true);
        let reply = error(&mut s, request, "protocol-request");
        assert_eq!(reply["state"], initial);
    }
    for budget in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("1"),
        json!(9_007_199_254_740_992u64),
    ] {
        let mut request = start("1");
        request["runBudgetMs"] = budget;
        error(&mut s, request, "invalid-deadline");
    }
    for budgets in [
        Value::Null,
        json!({}),
        json!([{"nodeId":"source"}]),
        json!([{"nodeId":"source","budgetMs":0}]),
        json!([{"nodeId":"source","budgetMs":null},{"nodeId":"source","budgetMs":null}]),
        json!([{"nodeId":"unknown","budgetMs":null}]),
    ] {
        let mut request = start("1");
        request["jobBudgets"] = budgets;
        error(&mut s, request, "invalid-deadline");
    }
    for input in [
        Value::Null,
        json!({"revision":0,"text":"x","mode":"compose"}),
        json!({"revision":1,"text":"x","mode":"bad"}),
        json!({"revision":1,"text":"x","mode":"compose","extra":true}),
    ] {
        let mut request = start("1");
        request["input"] = input;
        error(&mut s, request, "invalid-runtime-input");
    }
    let mut request = start("1");
    request["startKey"] = json!(1);
    error(&mut s, request, "protocol-request");
    assert_eq!(ok(&mut s, json!({"method":"status"}))["state"], initial);
    assert_eq!(h.host.usage(), usage);
    assert_eq!(h.count.load(Ordering::SeqCst), 0);
    let mut request = start("1");
    request["runBudgetMs"] = Value::Null;
    request["jobBudgets"] = json!([{"nodeId":"source","budgetMs":null}]);
    ok(&mut s, request);
    h.enter();
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[test]
fn invalidate_changes_input_generation_without_touching_plan_preparation() {
    let mut h = Fixture::new();
    let mut s = h.session();
    activate(&mut s, graph());
    let mut first = start("1");
    first["input"] = json!({"revision":1,"text":"k","mode":"compose"});
    let key = ok(&mut s, first)["data"]["key"].clone();
    let producer = h.enter();
    let prepared = ok(&mut s, json!({"method":"prepare","graph":graph()}));
    let invalidated = ok(
        &mut s,
        json!({"method":"invalidate-input","expectedPlanVersion":"1","revision":2}),
    );
    assert_eq!(
        invalidated["state"]["revision"],
        prepared["state"]["revision"]
    );
    assert_eq!(
        invalidated["state"]["preparedToken"],
        prepared["state"]["preparedToken"]
    );
    assert_eq!(invalidated["state"]["latestInputRevision"], 2);
    assert!(producer.cancel.is_cancelled());
    assert_eq!(
        ok(&mut s, json!({"method":"get-run","key":key}))["data"]["current"],
        false
    );
    let mut next = start("2");
    next["input"] = json!({"revision":2,"text":"ka","mode":"compose"});
    error(&mut s, next.clone(), "busy");
    producer.permit.finish_cancelled();
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    ok(&mut s, next);
    h.enter();
    let mut stale = start("3");
    stale["input"] = json!({"revision":1,"text":"k","mode":"compose"});
    error(&mut s, stale, "stale-input");
}

#[test]
fn real_completion_is_reported_and_retention_expiry_does_not_relaunch_the_key() {
    let mut h = Fixture::new();
    let mut s = h.session();
    activate(&mut s, graph());
    let key = ok(&mut s, start("1"))["data"]["key"].clone();
    let producer = h.enter();
    let output = ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: "結果😀".into(),
                meta: None,
            }))),
        )]
        .into(),
    );
    assert!(producer.permit.complete(Ok(output)));
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0 && h.host.usage().active_runs == 0);
    let result = ok(&mut s, json!({"method":"get-run","key":key}));
    assert_eq!(result["data"]["result"]["delivery"], "complete");
    assert_eq!(
        result["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "結果😀"
    );
    assert_eq!(result["data"]["pollAfterMs"], 0);
    let ttl = h.host.policy().retention_ms.duration();
    h.clock.advance_to(ttl).unwrap();
    until(|| h.host.usage().retained_runs == 0);
    error(&mut s, json!({"method":"get-run","key":key}), "result-gone");
    error(
        &mut s,
        json!({"method":"get-start","startKey":"1"}),
        "start-key-retired",
    );
    error(&mut s, start("1"), "start-key-retired");
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[test]
fn existing_runtime_input_and_ordinary_nodes_use_the_same_v3_worker_path() {
    let h = Fixture::new();
    let mut session = h.session();
    let graph =
        json!({"version":1,"nodes":[{"id":"source","kind":"input.text","version":1}],"edges":[]});
    activate(&mut session, graph);
    let mut request = start("1");
    request["input"] = json!({"revision":7,"text":"kya😀","mode":"compose"});
    let accepted = ok(&mut session, request);
    let key = accepted["data"]["key"].clone();
    until(|| h.host.usage().physical_jobs == 0 && h.host.usage().active_runs == 0);
    let completed = ok(&mut session, json!({"method":"get-run","key":key}));
    assert_eq!(
        completed["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "kya😀"
    );
    assert_eq!(completed["state"]["revision"], "2");
    assert_eq!(completed["state"]["latestInputRevision"], 7);
    assert_eq!(completed["state"]["activePlanVersion"], "1");
    assert_eq!(completed["data"]["key"]["inputRevision"], 7);
    assert_eq!(h.count.load(Ordering::SeqCst), 0);
}

#[test]
fn security_wire_cannot_forge_completion_or_release_a_real_worker_lease() {
    let mut h = Fixture::new();
    let mut s = h.session();
    activate(&mut s, graph());
    let key = ok(&mut s, start("1"))["data"]["key"].clone();
    let producer = h.enter();
    let state = ok(&mut s, json!({"method":"status"}))["state"].clone();
    let usage = h.host.usage();
    for method in ["complete", "finish-job", "producer-ended", "release-job"] {
        let reply = error(
            &mut s,
            json!({"method":method,"key":key,"jobId":"1","result":{"status":"success"}}),
            "protocol-request",
        );
        assert_eq!(reply["state"], state);
        assert_eq!(h.host.usage(), usage);
        let running = ok(&mut s, json!({"method":"get-run","key":key}));
        assert_eq!(running["data"]["state"], "running");
        assert!(running["data"].get("result").is_none());
    }
    assert!(!producer.permit.finish_cancelled());
    let output = || {
        ErrorAwareExecution::new(
            [(
                "out".into(),
                ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                    text: "authenticated-canary".into(),
                    meta: None,
                }))),
            )]
            .into(),
        )
    };
    assert!(producer.permit.complete(Ok(output())));
    assert!(!producer.permit.complete(Ok(output())));
    until(|| ok(&mut s, json!({"method":"get-run","key":key}))["data"]["state"] == "terminal");
    let completed = ok(&mut s, json!({"method":"get-run","key":key}));
    assert_eq!(completed["data"]["result"]["delivery"], "complete");
    assert_eq!(h.host.usage().physical_jobs, 1);
    let unchanged = ok(&mut s, json!({"method":"cancel-run","key":key}));
    assert_eq!(unchanged["data"]["result"], completed["data"]["result"]);
    assert!(!producer.cancel.is_cancelled());
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[test]
fn security_foreign_and_mutated_keys_cannot_cancel_or_adopt_another_sessions_result() {
    let mut h = Fixture::new();
    let mut a = h.session();
    let mut b = h.session();
    activate(&mut a, graph());
    activate(&mut b, graph());
    let key = ok(&mut a, start("1"))["data"]["key"].clone();
    let producer = h.enter();
    let a_state = ok(&mut a, json!({"method":"status"}))["state"].clone();
    let b_state = ok(&mut b, json!({"method":"status"}))["state"].clone();
    for method in ["get-run", "cancel-run"] {
        assert_eq!(
            error(
                &mut b,
                json!({"method":method,"key":key}),
                "session-conflict"
            )["state"],
            b_state
        );
        for field in ["planVersion", "deliveryGeneration"] {
            let mut forged = key.clone();
            forged[field] = json!("999");
            assert_eq!(
                error(
                    &mut a,
                    json!({"method":method,"key":forged}),
                    "run-key-conflict"
                )["state"],
                a_state
            );
        }
    }
    assert!(!producer.cancel.is_cancelled());
    assert_eq!(h.host.usage().physical_jobs, 1);
    let cancelled = ok(&mut a, json!({"method":"cancel-run","key":key}));
    until(|| producer.cancel.is_cancelled());
    let late = ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: "late-cancelled-canary".into(),
                meta: None,
            }))),
        )]
        .into(),
    );
    assert!(producer.permit.complete(Ok(late)));
    let ended = ok(&mut a, json!({"method":"get-run","key":key}));
    assert_eq!(ended["data"]["result"], cancelled["data"]["result"]);
    assert!(
        !ended["data"]["result"]
            .to_string()
            .contains("late-cancelled-canary")
    );
    assert_eq!(h.host.usage().physical_jobs, 1);
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0);
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[path = "../examples/security_support/input_gate.rs"]
mod security_input_gate;
#[test]
fn s04_gate_uses_real_v3_invalidation_and_survives_plan_switches_without_replaying_input() {
    use security_input_gate::{Denial, InputGate};
    let mut h = Fixture::new();
    let mut s = h.session();
    activate(&mut s, graph());
    let mut gate = InputGate::default();
    let mut first = start("1");
    first["input"] = json!({"revision":1,"text":"old-normal-canary","mode":"compose"});
    let (old_ticket, accepted) = gate.dispatch(true, || ok(&mut s, first)).unwrap();
    let key = accepted["data"]["key"].clone();
    let producer = h.enter();
    gate.set_manual(true, || {
        ok(
            &mut s,
            json!({"method":"invalidate-input","expectedPlanVersion":"1","revision":2}),
        );
        Ok::<_, ()>(())
    })
    .unwrap();
    assert!(gate.manual());
    assert!(producer.cancel.is_cancelled());
    gate.set_automatic(true, || {
        ok(
            &mut s,
            json!({"method":"invalidate-input","expectedPlanVersion":"1","revision":3}),
        );
        Ok::<_, ()>(())
    })
    .unwrap();
    gate.set_manual(false, || {
        ok(
            &mut s,
            json!({"method":"invalidate-input","expectedPlanVersion":"1","revision":4}),
        );
        Ok::<_, ()>(())
    })
    .unwrap();
    assert!(gate.automatic() && gate.blocked());
    assert!(!gate.accepts(&old_ticket));
    for _ in 0..32 {
        assert!(matches!(
            gate.dispatch(true, || panic!(
                "protected body/key/context must not reach start-run or debug/learning/API sinks"
            )),
            Err(Denial::Protected)
        ));
    }
    let changed = activate(&mut s, graph());
    assert_eq!(changed["state"]["activePlanVersion"], "2");
    assert!(gate.blocked());
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
    let late = ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: "late-old-canary".into(),
                meta: None,
            }))),
        )]
        .into(),
    );
    assert!(producer.permit.complete(Ok(late)));
    producer.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0 && h.host.usage().active_runs == 0);
    assert_eq!(
        ok(&mut s, json!({"method":"get-run","key":key}))["data"]["current"],
        false
    );
    gate.set_automatic(false, || {
        ok(
            &mut s,
            json!({"method":"invalidate-input","expectedPlanVersion":"2","revision":5}),
        );
        Ok::<_, ()>(())
    })
    .unwrap();
    assert!(!gate.blocked());
    assert!(!gate.accepts(&old_ticket));
    // No stored/queued protected input is replayed by either the gate or Session.
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
    let fresh = json!({"method":"start-run","startKey":"2","expectedPlanVersion":"2","targets":[{"node":"source","port":"out"}],"input":{"revision":6,"text":"fresh-normal-canary","mode":"compose"}});
    let (ticket, new) = gate.dispatch(true, || ok(&mut s, fresh)).unwrap();
    let current = h.enter();
    assert!(gate.accepts(&ticket));
    assert_eq!(h.count.load(Ordering::SeqCst), 2);
    let output = ErrorAwareExecution::new(
        [(
            "out".into(),
            ErrorAwareValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: "fresh-normal-canary".into(),
                meta: None,
            }))),
        )]
        .into(),
    );
    assert!(current.permit.complete(Ok(output)));
    current.release.send(()).unwrap();
    until(|| h.host.usage().physical_jobs == 0 && h.host.usage().active_runs == 0);
    let done = ok(&mut s, json!({"method":"get-run","key":new["data"]["key"]}));
    assert_eq!(done["data"]["current"], true);
    assert_eq!(
        done["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "fresh-normal-canary"
    );
}
