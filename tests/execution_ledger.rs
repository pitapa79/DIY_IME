#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    execution_control::{
        DecimalId, ExecutionHost, Milliseconds, ResolvedPort, ResourcePolicy, ResourceUsage,
        RunDelivery, RunKey, RunLedger, RunPhase, SessionEpoch, StartRequest, TerminalReason,
        TerminalResult,
    },
    graph::{Endpoint, ExecutionPlan, Graph},
    node::Registry,
};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use test_nodes::register_demo_nodes;
fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
fn profile() -> ResourcePolicy {
    ResourcePolicy::from_value(serde_json::from_str::<Value>(include_str!("contracts/execution_control.json")).unwrap()["resourceProfiles"]["bounded"].clone()).unwrap()
}
fn plan() -> ExecutionPlan {
    let mut r = Registry::new();
    register_demo_nodes(&mut r).unwrap();
    let g=Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":""}}}}],"edges":[]}).to_string()).unwrap();
    ExecutionPlan::compile(&g, &r).unwrap()
}
fn host(p: ResourcePolicy) -> ExecutionHost {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    ExecutionHost::new(
        format!("ledger-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        p,
    )
    .unwrap()
}
fn ledger() -> RunLedger {
    RunLedger::new(
        host(profile()),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan(),
        id(3),
    )
}
fn request(key: u64) -> StartRequest {
    serde_json::from_value(json!({"startKey":key.to_string(),"expectedPlanVersion":"3","targets":[{"node":"source","port":"out"}],"input":{"revision":8,"text":"かな😀","mode":"compose"}})).unwrap()
}
fn done(text: &str) -> TerminalResult {
    TerminalResult::completed(
        vec![Endpoint::new("source", "out")],
        vec![ResolvedPort {
            endpoint: Endpoint::new("source", "out"),
            value: json!({"status":"data","packet":{"version":1,"root":{"type":"text","text":text}}}),
        }],
        RunDelivery::Complete,
    )
}
fn value(ledger: &RunLedger, key: &RunKey) -> Value {
    serde_json::to_value(ledger.get_run(key).unwrap()).unwrap()
}

#[test]
fn accepted_receipt_and_read_only_queries_do_not_finish_or_advance_run() {
    let mut l = ledger();
    let receipt = l.start_run(request(1)).unwrap();
    assert_eq!(receipt.state, RunPhase::Accepted);
    assert!(!receipt.replayed);
    let before = value(&l, &receipt.key);
    assert!(before.get("result").is_none());
    assert_eq!(before["state"], "accepted");
    assert_eq!(value(&l, &receipt.key), before);
    assert_eq!(l.now(), ms(0));
    assert_eq!(l.revision(), 0);
    assert_eq!(l.get_start(id(1)).unwrap(), receipt.key);
    l.mark_running(&receipt.key).unwrap();
    l.advance_clock(ms(5)).unwrap();
    assert!(l.finish_run(&receipt.key, done("結果😀")).unwrap());
    let result = value(&l, &receipt.key);
    assert_eq!(result["result"]["delivery"], "complete");
    assert_eq!(result["elapsedMs"], 5);
    assert_eq!(
        result["key"]["runId"],
        json!(receipt.key.run_id.to_string())
    );
    l.advance_clock(ms(20)).unwrap();
    assert_eq!(value(&l, &receipt.key)["elapsedMs"], 5);
    assert_eq!(l.host().usage().active_runs, 0);
    assert_eq!(l.host().usage().reserved_result_bytes, 0);
    assert_eq!(l.host().usage().retained_runs, 1);
}
#[test]
fn ledger_projection_of_formal_deadline_boundaries() {
    let fixture: Value =
        serde_json::from_str(include_str!("contracts/execution_control.json")).unwrap();
    for name in [
        "T01-before-run-deadline",
        "T02-exact-run-deadline",
        "T03-after-run-deadline",
    ] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == name)
            .unwrap();
        let mut l = ledger();
        let mut req = request(1);
        req.run_budget_ms = Some(Milliseconds::new(100).unwrap());
        let key = l.start_run(req).unwrap().key;
        l.mark_running(&key).unwrap();
        let at = case["events"][1]["atMs"].as_u64().unwrap();
        l.advance_clock(ms(at)).unwrap();
        let adopted = l.finish_run(&key, done("完了")).unwrap();
        let snapshot = l.get_run(&key).unwrap();
        assert_eq!(
            serde_json::to_value(snapshot.result.unwrap().delivery).unwrap(),
            case["expected"]["r1"]["delivery"]
        );
        assert_eq!(adopted, at < 100);
        assert_eq!(l.host().usage().active_runs, 0);
    }
}
#[test]
fn cancel_and_completion_linearize_in_both_same_time_orders() {
    for cancel_first in [false, true] {
        let mut l = ledger();
        let key = l.start_run(request(1)).unwrap().key;
        l.advance_clock(ms(2)).unwrap();
        if cancel_first {
            l.cancel_run(&key).unwrap();
            assert!(!l.finish_run(&key, done("遅延")).unwrap());
        } else {
            assert!(l.finish_run(&key, done("先着")).unwrap());
            l.cancel_run(&key).unwrap();
        }
        let snapshot = value(&l, &key);
        assert_eq!(
            snapshot["result"]["delivery"],
            if cancel_first {
                "cancelled"
            } else {
                "complete"
            }
        );
        let usage = l.host().usage();
        let duplicate = l.cancel_run(&key).unwrap();
        assert_eq!(serde_json::to_value(duplicate).unwrap(), snapshot);
        assert_eq!(l.host().usage(), usage);
    }
}
#[test]
fn replay_conflict_and_old_plan_replay_do_not_start_more_runs() {
    let mut l = ledger();
    let req = request(1);
    let first = l.start_run(req.clone()).unwrap();
    let usage = l.host().usage();
    let repeat = l.start_run(req.clone()).unwrap();
    assert!(repeat.replayed);
    assert_eq!(repeat.key, first.key);
    assert_eq!(l.host().usage(), usage);
    let mut changed = req.clone();
    changed.input.as_mut().unwrap().text = "別".into();
    assert_eq!(l.start_run(changed).unwrap_err().code, "start-conflict");
    assert_eq!(l.host().usage(), usage);
    l.finish_run(&first.key, done("確定")).unwrap();
    l.prepare(plan()).unwrap();
    l.activate_pending().unwrap();
    assert_eq!(l.plan_version(), id(4));
    let replay = l.start_run(req).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.key, first.key);
    assert!(!l.get_run(&first.key).unwrap().current);
}
#[test]
fn invalid_requests_do_not_consume_start_keys_or_change_state() {
    let mut l = ledger();
    for bad in [0, 1, 2, 3] {
        let mut req = request(1);
        match bad {
            0 => req.targets.clear(),
            1 => req.targets[0].node = "missing".into(),
            2 => req.input.as_mut().unwrap().revision = 0,
            _ => req.expected_plan_version = id(4),
        };
        assert!(l.start_run(req).is_err());
        assert_eq!(l.host().usage(), ResourceUsage::default());
        assert_eq!(l.get_start(id(1)).unwrap_err().code, "start-not-seen");
        assert_eq!(l.latest_input_revision(), None);
        assert_eq!(l.revision(), 0);
    }
    let req = request(1);
    l.start_run(req).unwrap();
}
#[test]
fn busy_admission_does_not_evict_cache_or_invalidate_old_input() {
    let mut p = profile();
    p.max_active_runs = 1;
    let mut l = RunLedger::new(
        host(p),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan(),
        id(3),
    );
    let old = l.start_run(request(1)).unwrap().key;
    l.finish_run(&old, done("保持")).unwrap();
    let active = l.start_run(request(2)).unwrap().key;
    let before = l.host().usage();
    let mut req = request(3);
    req.input.as_mut().unwrap().revision = 9;
    assert_eq!(l.start_run(req.clone()).unwrap_err().code, "busy");
    assert_eq!(l.host().usage(), before);
    assert!(l.get_run(&old).is_ok());
    assert_eq!(l.get_run(&active).unwrap().state, RunPhase::Accepted);
    assert_eq!(l.latest_input_revision(), Some(8));
    assert_eq!(l.get_start(id(3)).unwrap_err().code, "start-not-seen");
    l.cancel_run(&active).unwrap();
    let next = l.start_run(req).unwrap();
    assert!(!next.replayed);
    assert_eq!(l.latest_input_revision(), Some(9));
    assert!(!l.get_run(&old).unwrap().current);
}
#[test]
fn retention_boundary_and_retired_watermark_prevent_relaunch() {
    let mut l = ledger();
    let req = request(1);
    let key = l.start_run(req.clone()).unwrap().key;
    l.advance_clock(ms(1)).unwrap();
    l.finish_run(&key, done("結果")).unwrap();
    l.advance_clock(ms(1000)).unwrap();
    assert!(l.get_run(&key).is_ok());
    l.advance_clock(ms(1001)).unwrap();
    assert_eq!(l.get_run(&key).unwrap_err().code, "result-gone");
    assert_eq!(l.start_run(req).unwrap_err().code, "start-key-retired");
    assert_eq!(l.get_start(id(1)).unwrap_err().code, "start-key-retired");
    assert_eq!(l.get_start(id(2)).unwrap_err().code, "start-not-seen");
    assert_eq!(l.host().usage().active_runs, 0);
    assert_eq!(l.host().usage().dedupe_entries, 0);
}
#[test]
fn no_tombstones_returns_unknown_without_implying_not_executed() {
    let mut p = profile();
    p.max_tombstones = 0;
    let mut l = RunLedger::new(
        host(p),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan(),
        id(3),
    );
    let key = l.start_run(request(1)).unwrap().key;
    l.finish_run(&key, done("結果")).unwrap();
    l.advance_clock(ms(1000)).unwrap();
    assert_eq!(l.get_run(&key).unwrap_err().code, "run-unknown");
    assert_eq!(l.get_start(id(1)).unwrap_err().code, "start-key-retired");
}
#[test]
fn domain_cache_evicts_oldest_across_controllers_without_cycles() {
    let mut p = profile();
    p.max_retained_runs = 1;
    let shared = host(p);
    let mut a = RunLedger::new(
        shared.clone(),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan(),
        id(3),
    );
    let mut b = RunLedger::new(
        shared.clone(),
        SessionEpoch::new("fedcba9876543210fedcba9876543210").unwrap(),
        plan(),
        id(3),
    );
    let ak = a.start_run(request(1)).unwrap().key;
    a.finish_run(&ak, done("A")).unwrap();
    let bk = b.start_run(request(1)).unwrap().key;
    b.finish_run(&bk, done("B")).unwrap();
    assert_eq!(a.get_run(&ak).unwrap_err().code, "result-gone");
    assert_eq!(shared.usage().retained_runs, 1);
    assert!(b.get_run(&bk).is_ok());
    let domain = shared.resource_domain().to_owned();
    drop(a);
    drop(b);
    drop(shared);
    ExecutionHost::new(domain, profile()).unwrap();
}
#[test]
fn input_invalidation_keeps_fixed_runs_and_requires_consistent_binding() {
    let mut l = ledger();
    let input = l.start_run(request(1)).unwrap().key;
    let mut fixed = request(2);
    fixed.input = None;
    let fixed = l.start_run(fixed).unwrap().key;
    l.invalidate_input(id(3), 9).unwrap();
    assert_eq!(l.revision(), 0);
    assert_eq!(
        l.get_run(&input).unwrap().result.unwrap().delivery,
        RunDelivery::Cancelled
    );
    assert!(!l.get_run(&input).unwrap().current);
    assert_eq!(l.get_run(&fixed).unwrap().state, RunPhase::Accepted);
    l.finish_run(&fixed, done("固定")).unwrap();
    assert!(l.get_run(&fixed).unwrap().current);
    let mut next = request(3);
    next.input.as_mut().unwrap().revision = 9;
    l.start_run(next).unwrap();
    let mut conflict = request(4);
    conflict.input.as_mut().unwrap().revision = 9;
    conflict.input.as_mut().unwrap().text = "不一致".into();
    assert_eq!(l.start_run(conflict).unwrap_err().code, "input-conflict");
    assert_eq!(l.start_run(request(4)).unwrap_err().code, "stale-input");
}
#[test]
fn same_input_can_have_two_distinct_runs_completing_in_reverse_order() {
    let mut l = ledger();
    let a = l.start_run(request(1)).unwrap().key;
    let b = l.start_run(request(2)).unwrap().key;
    assert_ne!(a.run_id, b.run_id);
    l.finish_run(&b, done("新しい要求")).unwrap();
    l.finish_run(&a, done("前の要求")).unwrap();
    assert!(l.get_run(&a).unwrap().current);
    assert!(l.get_run(&b).unwrap().current);
    assert_eq!(
        value(&l, &a)["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "前の要求"
    );
    assert_eq!(
        value(&l, &b)["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "新しい要求"
    );
}
#[test]
fn prepare_does_not_invalidate_but_activate_and_rollback_do() {
    let mut l = ledger();
    let key = l.start_run(request(1)).unwrap().key;
    l.prepare(plan()).unwrap();
    assert_eq!(l.plan_version(), id(3));
    assert_eq!(l.get_run(&key).unwrap().state, RunPhase::Accepted);
    assert!(l.has_pending());
    l.activate_pending().unwrap();
    assert_eq!(l.plan_version(), id(4));
    assert_eq!(
        l.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Cancelled
    );
    assert!(!l.get_run(&key).unwrap().current);
    l.rollback().unwrap();
    assert_eq!(l.plan_version(), id(5));
    assert!(!l.rollback().unwrap());
    assert!(!l.get_run(&key).unwrap().current);
}
#[test]
fn malformed_result_and_keys_cannot_change_an_active_run() {
    let mut l = ledger();
    let key = l.start_run(request(1)).unwrap().key;
    let before = value(&l, &key);
    let mut bad = done("x");
    bad.delivery = RunDelivery::NoOutput;
    assert_eq!(l.finish_run(&key, bad).unwrap_err().code, "invalid-result");
    assert_eq!(value(&l, &key), before);
    let mut wrong = key.clone();
    wrong.delivery_generation = id(999);
    assert_eq!(l.get_run(&wrong).unwrap_err().code, "run-key-conflict");
    assert_eq!(l.cancel_run(&wrong).unwrap_err().code, "run-key-conflict");
    let mut other = key.clone();
    other.session_epoch = SessionEpoch::new("fedcba9876543210fedcba9876543210").unwrap();
    assert_eq!(l.get_run(&other).unwrap_err().code, "session-conflict");
    assert_eq!(value(&l, &key), before);
}
#[test]
fn result_limits_fail_explicitly_and_minimum_budget_is_checked_before_admission() {
    let mut l = ledger();
    let key = l.start_run(request(1)).unwrap().key;
    let huge = "a".repeat(20000);
    assert!(l.finish_run(&key, done(&huge)).unwrap());
    let snapshot = l.get_run(&key).unwrap();
    let result = snapshot.result.unwrap();
    assert_eq!(result.reason, TerminalReason::Failed);
    assert_eq!(result.delivery, RunDelivery::Failed);
    assert_eq!(result.error.unwrap().code, "result-limit");
    assert!(result.ports.is_empty());
    let mut p = profile();
    p.max_result_bytes_per_run = 1;
    let mut small = RunLedger::new(
        host(p),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        plan(),
        id(3),
    );
    assert_eq!(
        small.start_run(request(1)).unwrap_err().code,
        "invalid-result-budget"
    );
    assert_eq!(small.host().usage(), ResourceUsage::default());
}
#[test]
fn clock_backwards_and_duration_overflow_leave_request_state_unchanged() {
    let mut l = ledger();
    let key = l.start_run(request(1)).unwrap().key;
    l.advance_clock(ms(4)).unwrap();
    let before = value(&l, &key);
    assert_eq!(l.advance_clock(ms(3)).unwrap_err().code, "invalid-clock");
    assert_eq!(value(&l, &key), before);
    let mut overflow = ledger();
    overflow.advance_clock(Duration::MAX).unwrap();
    let mut req = request(1);
    req.run_budget_ms = Some(Milliseconds::new(10).unwrap());
    assert_eq!(
        overflow.start_run(req).unwrap_err().code,
        "invalid-deadline"
    );
    assert_eq!(overflow.host().usage(), ResourceUsage::default());
    assert_eq!(
        overflow.get_start(id(1)).unwrap_err().code,
        "start-not-seen"
    );
}

#[test]
fn owned_plan_outlives_cancel_and_two_plan_replacements() {
    let mut l = ledger();
    let key = l.start_run(request(1)).unwrap().key;
    let owned = l.plan_snapshot(&key).unwrap();
    let weak = std::sync::Arc::downgrade(&owned);
    l.prepare(plan()).unwrap();
    l.activate_pending().unwrap();
    l.prepare(plan()).unwrap();
    l.activate_pending().unwrap();
    assert!(weak.upgrade().is_some());
    assert_eq!(owned.node_order().collect::<Vec<_>>(), vec!["source"]);
    drop(owned);
    assert!(weak.upgrade().is_none());
    assert_eq!(
        l.get_run(&key).unwrap().result.unwrap().reason,
        TerminalReason::Cancelled
    );
}

#[test]
fn far_clock_jump_processes_finite_deadline_before_result_retention() {
    let mut l = ledger();
    let mut req = request(1);
    req.run_budget_ms = Some(Milliseconds::new(10).unwrap());
    let key = l.start_run(req).unwrap().key;
    l.advance_clock(Duration::MAX).unwrap();
    assert_eq!(l.get_run(&key).unwrap_err().code, "result-gone");
    assert_eq!(l.host().usage().active_runs, 0);
    assert_eq!(l.get_start(id(1)).unwrap_err().code, "start-key-retired");
}
