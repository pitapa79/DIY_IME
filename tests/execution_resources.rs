use diy_ime2_core::execution_control::{
    CancellationToken, DecimalId, ExecutionHost, Milliseconds, ResourcePolicy, ResourceUsage,
    RunBudgets, RunKey, SessionEpoch,
};
use serde_json::{Value, json};
use std::{str::FromStr, sync::mpsc, thread, time::Duration};
fn profile_value() -> Value {
    serde_json::from_str::<Value>(include_str!("contracts/execution_control.json")).unwrap()["resourceProfiles"]["bounded"].clone()
}
fn profile() -> ResourcePolicy {
    ResourcePolicy::from_value(profile_value()).unwrap()
}
fn host() -> ExecutionHost {
    static NEXT_DOMAIN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_DOMAIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ExecutionHost::new(format!("test-resource-domain-{id}"), profile()).unwrap()
}
fn jobs(n: u64) -> ResourceUsage {
    ResourceUsage {
        physical_jobs: n,
        ..Default::default()
    }
}

#[test]
fn decimal_ids_roundtrip_exactly_and_reject_other_json_types() {
    for s in ["1", "9007199254740993", "18446744073709551615"] {
        let id = DecimalId::from_str(s).unwrap();
        assert_eq!(id.to_string(), s);
        let encoded = serde_json::to_value(id).unwrap();
        assert_eq!(encoded, json!(s));
        assert_eq!(serde_json::from_value::<DecimalId>(encoded).unwrap(), id);
    }
    for s in [
        "",
        "0",
        "00",
        "01",
        "+1",
        "-1",
        "1.0",
        "1e3",
        " 1",
        "1\n",
        "１２",
        "18446744073709551616",
    ] {
        assert!(DecimalId::from_str(s).is_err(), "{s}");
    }
    for value in [json!(1), json!(1.0), json!(null), json!(true), json!([])] {
        assert!(serde_json::from_value::<DecimalId>(value).is_err());
    }
    assert!(DecimalId::new(0).is_err());
    assert_eq!(
        DecimalId::new(u64::MAX)
            .unwrap()
            .checked_next()
            .unwrap_err()
            .code,
        "id-exhausted"
    );
}
#[test]
fn epoch_validates_format_without_generating_randomness() {
    let value = "0123456789abcdef0123456789abcdef";
    let epoch = SessionEpoch::new(value).unwrap();
    assert_eq!(epoch.as_str(), value);
    assert_eq!(
        serde_json::from_value::<SessionEpoch>(json!(value)).unwrap(),
        epoch
    );
    for value in [
        "",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcdeg",
        "0123456789abcdef0123456789abcde",
        "0123456789abcdef0123456789abcdef0",
    ] {
        assert!(SessionEpoch::new(value).is_err());
    }
}
#[test]
fn durations_are_safe_integers_and_overflow_is_checked() {
    for n in [1, 2_147_483_648, 9_007_199_254_740_991] {
        let d = Milliseconds::new(n).unwrap();
        assert_eq!(d.get(), n);
        assert_eq!(serde_json::to_value(d).unwrap(), json!(n));
    }
    for value in [
        json!(0),
        json!(-1),
        json!(0.5),
        json!("1"),
        json!(null),
        json!(9_007_199_254_740_992_u64),
    ] {
        assert!(serde_json::from_value::<Milliseconds>(value).is_err());
    }
    let d = Milliseconds::new(1).unwrap();
    assert_eq!(
        d.checked_deadline(Duration::from_millis(99)).unwrap(),
        Duration::from_millis(100)
    );
    assert_eq!(
        d.checked_deadline(Duration::MAX).unwrap_err().code,
        "invalid-deadline"
    );
}
#[test]
fn run_key_preserves_optional_revision_and_rejects_invalid_snapshots() {
    let key = json!({"sessionEpoch":"0123456789abcdef0123456789abcdef","planVersion":"3","runId":"9007199254740993","deliveryGeneration":"2"});
    let fixed: RunKey = serde_json::from_value(key.clone()).unwrap();
    assert!(fixed.input_revision.is_none());
    assert_eq!(serde_json::to_value(&fixed).unwrap(), key);
    let mut with_input = key.clone();
    with_input["inputRevision"] = json!(8);
    let parsed: RunKey = serde_json::from_value(with_input).unwrap();
    assert_eq!(parsed.input_revision, Some(8));
    assert_ne!(parsed, fixed);
    for revision in [json!(null), json!(0), json!(9_007_199_254_740_992_u64)] {
        let mut bad = key.clone();
        bad["inputRevision"] = revision;
        assert!(serde_json::from_value::<RunKey>(bad).is_err());
    }
    let mut bad = key;
    bad["runId"] = json!(1);
    assert!(serde_json::from_value::<RunKey>(bad).is_err());
}
#[test]
fn policy_requires_every_field_and_valid_ranges_before_host_creation() {
    let fixtures: Value =
        serde_json::from_str(include_str!("contracts/execution_control.json")).unwrap();
    for value in fixtures["resourceProfiles"].as_object().unwrap().values() {
        let p = ResourcePolicy::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), *value);
        ExecutionHost::new("tests", p).unwrap();
    }
    let valid = profile_value();
    for key in valid.as_object().unwrap().keys() {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            ResourcePolicy::from_value(missing).is_err(),
            "missing {key}"
        );
    }
    for (field, value) in [
        ("maxPhysicalJobs", json!(0)),
        ("maxActiveRuns", json!(-1)),
        ("maxObservers", json!(1.5)),
        ("maxQueuedJobs", json!(-1)),
        ("maxRetainedBytes", json!("1000")),
        ("maxTombstones", json!(9_007_199_254_740_992_u64)),
        ("retentionMs", json!(null)),
        ("queueWaitMs", json!(0)),
        ("extra", json!(true)),
    ] {
        let mut bad = valid.clone();
        bad[field] = value;
        assert_eq!(
            ResourcePolicy::from_value(bad).unwrap_err().code,
            "invalid-resource-policy"
        );
    }
    let mut bad = profile();
    bad.max_physical_jobs = u64::MAX;
    assert_eq!(
        ExecutionHost::new("tests", bad).unwrap_err().code,
        "invalid-resource-policy"
    );
    let mut small = profile();
    small.max_retained_bytes = 1;
    assert!(small.validate().is_err());
    assert!(ExecutionHost::new(" ", profile()).is_err());
}
#[test]
fn run_budget_defaults_duplicate_nodes_and_missing_budget_are_checked() {
    let defaults: RunBudgets = serde_json::from_value(json!({})).unwrap();
    assert!(defaults.run_budget_ms.is_none());
    assert!(defaults.job_budgets.is_empty());
    let explicit: RunBudgets = serde_json::from_value(
        json!({"runBudgetMs":null,"jobBudgets":[{"nodeId":"a","budgetMs":null}]}),
    )
    .unwrap();
    explicit.validate().unwrap();
    for bad in [
        json!({"jobBudgets":[{"nodeId":"a"}]}),
        json!({"runBudgetMs":"100"}),
        json!({"runBudgetMs":0}),
        json!({"jobBudgets":null}),
        json!({"unexpected":1}),
    ] {
        assert!(serde_json::from_value::<RunBudgets>(bad).is_err());
    }
    for entries in [
        json!([{"nodeId":"","budgetMs":1}]),
        json!([{"nodeId":"a","budgetMs":1},{"nodeId":"a","budgetMs":null}]),
    ] {
        let parsed: RunBudgets = serde_json::from_value(json!({"jobBudgets":entries})).unwrap();
        assert_eq!(parsed.validate().unwrap_err().code, "invalid-deadline");
    }
}
#[test]
fn batch_reservation_is_atomic_and_overflow_does_not_change_usage() {
    let host = host();
    let occupied = host.try_reserve(jobs(2)).unwrap();
    let before = host.usage();
    assert_eq!(
        host.try_reserve(ResourceUsage {
            physical_jobs: 1,
            active_runs: 1,
            reserved_result_bytes: 100,
            ..Default::default()
        })
        .unwrap_err()
        .code,
        "busy"
    );
    assert_eq!(host.usage(), before);
    assert!(host.try_reserve(jobs(u64::MAX)).is_err());
    assert_eq!(host.usage(), before);
    drop(occupied);
    assert_eq!(host.usage(), ResourceUsage::default());
}
#[test]
fn split_shared_ownership_and_replace_keep_exact_balanced_counters() {
    let host = host();
    let mut reservation = host
        .try_reserve(ResourceUsage {
            active_runs: 1,
            physical_jobs: 2,
            reserved_result_bytes: 100,
            dedupe_entries: 1,
            ..Default::default()
        })
        .unwrap();
    let mut worker = reservation.split(jobs(1)).unwrap();
    let snapshot = host.usage();
    assert!(reservation.split(jobs(2)).is_err());
    assert_eq!(host.usage(), snapshot);
    let another = worker.clone();
    assert_eq!(worker.split(jobs(1)).unwrap_err().code, "shared-lease");
    assert_eq!(worker.replace(jobs(0)).unwrap_err().code, "shared-lease");
    drop(worker);
    assert_eq!(host.usage().physical_jobs, 2);
    drop(another);
    assert_eq!(host.usage().physical_jobs, 1);
    reservation
        .replace(ResourceUsage {
            retained_runs: 1,
            retained_bytes: 80,
            dedupe_entries: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(host.usage().active_runs, 0);
    assert_eq!(host.usage().reserved_result_bytes, 0);
    assert_eq!(host.usage().retained_bytes, 80);
    assert_eq!(host.usage().physical_jobs, 0);
    let before = host.usage();
    assert!(
        reservation
            .replace(ResourceUsage {
                retained_bytes: u64::MAX,
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(host.usage(), before);
    drop(reservation);
    assert_eq!(host.usage(), ResourceUsage::default());
}
#[test]
fn reserved_and_retained_result_bytes_share_one_budget() {
    let host = host();
    let max = host.policy().max_retained_bytes;
    let reserved = host
        .try_reserve(ResourceUsage {
            reserved_result_bytes: max - 1,
            ..Default::default()
        })
        .unwrap();
    let retained = host
        .try_reserve(ResourceUsage {
            retained_bytes: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(
        host.try_reserve(ResourceUsage {
            reserved_result_bytes: 1,
            ..Default::default()
        })
        .is_err()
    );
    drop(retained);
    assert!(
        host.try_reserve(ResourceUsage {
            retained_bytes: 2,
            ..Default::default()
        })
        .is_err()
    );
    drop(reserved);
    assert_eq!(host.usage(), ResourceUsage::default());
}
#[test]
fn every_limit_is_enforced_independently() {
    let host = host();
    let p = host.policy();
    let limits = [
        ResourceUsage {
            active_runs: p.max_active_runs + 1,
            ..Default::default()
        },
        ResourceUsage {
            physical_jobs: p.max_physical_jobs + 1,
            ..Default::default()
        },
        ResourceUsage {
            observers: p.max_observers + 1,
            ..Default::default()
        },
        ResourceUsage {
            queued_jobs: p.max_queued_jobs + 1,
            ..Default::default()
        },
        ResourceUsage {
            queued_input_bytes: p.max_queued_input_bytes + 1,
            ..Default::default()
        },
        ResourceUsage {
            retained_runs: p.max_retained_runs + 1,
            ..Default::default()
        },
        ResourceUsage {
            retained_bytes: p.max_retained_bytes + 1,
            ..Default::default()
        },
        ResourceUsage {
            dedupe_entries: p.max_dedupe_entries + 1,
            ..Default::default()
        },
        ResourceUsage {
            tombstones: p.max_tombstones + 1,
            ..Default::default()
        },
    ];
    for usage in limits {
        assert!(host.try_reserve(usage).is_err());
        assert_eq!(host.usage(), ResourceUsage::default());
    }
}
#[test]
fn real_cancelled_worker_keeps_lease_after_controller_handle_drops() {
    let mut p = profile();
    p.max_physical_jobs = 1;
    let host = ExecutionHost::new("shared-domain", p).unwrap();
    let other = host.clone();
    let lease = host.try_reserve(jobs(1)).unwrap();
    let worker_lease = lease.clone();
    let token = CancellationToken::default();
    let worker_token = token.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        assert!(worker_token.is_cancelled());
        drop(worker_lease);
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    token.cancel();
    drop(lease);
    drop(host);
    assert!(other.try_reserve(jobs(1)).is_err());
    assert_eq!(other.usage().physical_jobs, 1);
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(other.usage().physical_jobs, 0);
    other.try_reserve(jobs(1)).unwrap();
}
#[test]
fn concurrent_admission_never_exceeds_physical_capacity() {
    let host = host();
    let (result_tx, result_rx) = mpsc::channel();
    let mut releases = vec![];
    let mut threads = vec![];
    for _ in 0..8 {
        let host = host.clone();
        let tx = result_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        releases.push(release_tx);
        threads.push(thread::spawn(move || {
            let lease = host.try_reserve(jobs(1));
            tx.send(lease.is_ok()).unwrap();
            release_rx.recv().unwrap();
            drop(lease);
        }));
    }
    let successes = (0..8)
        .filter(|_| result_rx.recv_timeout(Duration::from_secs(10)).unwrap())
        .count();
    let in_use = host.usage().physical_jobs;
    for release in releases {
        release.send(()).unwrap();
    }
    for worker in threads {
        worker.join().unwrap();
    }
    assert_eq!(successes, 2);
    assert_eq!(in_use, 2);
    assert_eq!(host.usage(), ResourceUsage::default());
}

#[test]
fn dropping_host_handles_does_not_allow_resetting_a_still_owned_domain() {
    let host = ExecutionHost::new("reset-regression-domain", profile()).unwrap();
    let worker = host.try_reserve(jobs(1)).unwrap();
    drop(host);
    assert_eq!(
        ExecutionHost::new("reset-regression-domain", profile())
            .unwrap_err()
            .code,
        "invalid-resource-policy"
    );
    drop(worker);
    let replacement = ExecutionHost::new("reset-regression-domain", profile()).unwrap();
    assert_eq!(replacement.usage(), ResourceUsage::default());
}

#[test]
fn exchanging_leases_is_atomic_and_rejects_other_domains_or_shared_owners() {
    let a = host();
    let b = host();
    let mut owned = a.try_reserve(jobs(2)).unwrap();
    let mut foreign = b.try_reserve(jobs(1)).unwrap();
    assert_eq!(
        a.try_reserve_replacing(&mut [&mut foreign], jobs(1))
            .unwrap_err()
            .code,
        "shared-lease"
    );
    assert_eq!(a.usage().physical_jobs, 2);
    assert_eq!(b.usage().physical_jobs, 1);
    let clone = owned.clone();
    assert_eq!(
        a.try_reserve_replacing(&mut [&mut owned], jobs(0))
            .unwrap_err()
            .code,
        "shared-lease"
    );
    drop(clone);
    assert!(a.try_reserve_replacing(&mut [&mut owned], jobs(3)).is_err());
    assert_eq!(owned.usage().physical_jobs, 2);
    assert_eq!(a.usage().physical_jobs, 2);
    let replacement = a.try_reserve_replacing(&mut [&mut owned], jobs(1)).unwrap();
    assert_eq!(owned.usage(), ResourceUsage::default());
    drop(owned);
    assert_eq!(a.usage().physical_jobs, 1);
    drop(replacement);
    assert_eq!(a.usage(), ResourceUsage::default());
}

#[test]
fn security_rejected_batch_flood_preserves_held_resources_and_remaining_capacity() {
    let host = host();
    let bytes = host.policy().max_retained_bytes;
    let held = host
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            reserved_result_bytes: bytes - 1,
            ..Default::default()
        })
        .unwrap();
    let worker = held.clone();
    let client = host.clone();
    let before = host.usage();
    // The job alone would fit, but the combined byte demand must fail atomically.
    for _ in 0..256 {
        assert_eq!(
            client
                .try_reserve(ResourceUsage {
                    physical_jobs: 1,
                    reserved_result_bytes: 2,
                    ..Default::default()
                })
                .unwrap_err()
                .code,
            "busy"
        );
        assert_eq!(client.usage(), before);
    }
    assert!(ExecutionHost::new(host.resource_domain(), host.policy().clone()).is_err());
    drop(held);
    assert_eq!(client.usage(), before);
    let allowed = client
        .try_reserve(ResourceUsage {
            physical_jobs: 1,
            reserved_result_bytes: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(client.usage().physical_jobs, 2);
    assert_eq!(client.usage().reserved_result_bytes, bytes);
    drop(allowed);
    assert_eq!(client.usage(), before);
    drop(worker);
    assert_eq!(client.usage(), ResourceUsage::default());
    let normal = client.try_reserve(jobs(2)).unwrap();
    drop(normal);
    assert_eq!(client.usage(), ResourceUsage::default());
}
