//! Real Worker and lifetime pin proof. Bounded driver/Host route still pending.
use super::*;
use crate::{
    execution_control::{ExecutionHost, ResourcePolicy, ResourceUsage, RunKey},
    isolated_worker::{
        RetainedBinding, RetainedReservation, RetainedWorkerRequest, execute_retained,
        resource_transport, runtime,
    },
};
use std::{
    io::{Read, Write},
    sync::mpsc,
    thread,
};

pub(super) fn fixture_host(name: &str) -> ExecutionHost {
    let value: Value = serde_json::from_str(include_str!(
        "../../../tests/contracts/execution_control.json"
    ))
    .unwrap();
    let mut policy = value["resourceProfiles"]["bounded"].clone();
    policy["maxPhysicalJobs"] = json!(1);
    ExecutionHost::new(
        format!(
            "{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        ResourcePolicy::from_value(policy).unwrap(),
    )
    .unwrap()
}
pub(super) fn fixture_key(run: u64) -> RunKey {
    serde_json::from_value(json!({"sessionEpoch":"11111111111111111111111111111111","planVersion":"1","runId":run.to_string(),"deliveryGeneration":"1"})).unwrap()
}
pub(super) fn fixture_empty(
    gate: &ExecutionGate,
    target: &GrantTarget,
    run: u64,
    cancel: CancellationToken,
) -> ResourceRangeSession {
    ResourceRangeSession::open_empty(
        gate.clone(),
        target.clone(),
        fixture_key(run),
        cancel,
        ResourceRangeLimits {
            max_resources: 1,
            max_snapshot_bytes: 8,
            max_read_bytes: 8,
            max_total_read_bytes: 8,
            max_reads: 1,
            budget: Duration::from_secs(30),
        },
    )
    .unwrap()
}
pub(super) fn fixture_scope(
    gate: &ExecutionGate,
    target: &GrantTarget,
    run: u64,
    cancel: CancellationToken,
    data: Arc<[u8]>,
) -> ResourceRangeSession {
    ResourceRangeSession::open(
        gate.clone(),
        target.clone(),
        fixture_key(run),
        cancel,
        [("prepared.bin".into(), data)].into(),
        ResourceRangeLimits {
            max_resources: 1,
            max_snapshot_bytes: 8,
            max_read_bytes: 8,
            max_total_read_bytes: 8,
            max_reads: 1,
            budget: Duration::from_secs(30),
        },
    )
    .unwrap()
}
#[test]
fn execution_only_scope_needs_a_grant_and_cannot_read_any_resource() {
    let gate = ExecutionGate::default();
    let key = fixture_key(1);
    let target = GrantTarget::new(
        "code-plan".into(),
        &key.session_epoch,
        key.plan_version,
        "pure-node".into(),
    );
    let limits = ResourceRangeLimits {
        max_resources: 1,
        max_snapshot_bytes: 8,
        max_read_bytes: 8,
        max_total_read_bytes: 8,
        max_reads: 1,
        budget: Duration::from_secs(30),
    };
    assert!(
        ResourceRangeSession::open_empty(
            gate.clone(),
            target.clone(),
            key.clone(),
            CancellationToken::default(),
            limits.clone()
        )
        .is_err()
    );
    gate.approve(target.clone());
    assert!(
        ResourceRangeSession::open(
            gate.clone(),
            target.clone(),
            key,
            CancellationToken::default(),
            Default::default(),
            limits
        )
        .is_err()
    );
    let mut scope = fixture_empty(&gate, &target, 2, CancellationToken::default());
    assert!(scope.descriptions().unwrap().is_empty());
    assert_eq!(scope.snapshot_bytes().unwrap(), 0);
    assert!(
        scope
            .read(&ResourceRangeRequest {
                request_id: fixture_key(1).run_id,
                resource_id: 1,
                offset: 0,
                length: 1
            })
            .is_err()
    );
}
#[test]
fn lifetime_reservation_rejects_other_allocations_slots_and_grants_without_returning_the_charge() {
    for case in 0..4 {
        let host = fixture_host("retained-scope-negative");
        let gate = ExecutionGate::default();
        let key = fixture_key(1);
        let target = GrantTarget::new(
            "code-plan".into(),
            &key.session_epoch,
            key.plan_version,
            "node".into(),
        );
        gate.approve(target.clone());
        let source: Arc<[u8]> = Arc::from(&b"abcdefgh"[..]);
        let cancel = CancellationToken::default();
        let pin = RetainedReservation::reserve(
            &host,
            vec![RetainedBinding {
                module_slot: 1,
                scope: fixture_scope(&gate, &target, 1, cancel.clone(), source.clone()),
            }],
            &cancel,
        )
        .unwrap();
        let other_gate = ExecutionGate::default();
        other_gate.approve(target.clone());
        let data = if case == 0 {
            Arc::from(source.to_vec())
        } else {
            source.clone()
        };
        let mut input = fixture_scope(
            if case == 1 { &other_gate } else { &gate },
            &target,
            2,
            CancellationToken::default(),
            data,
        );
        if case == 3 {
            gate.approve(target.clone());
        }
        assert!(
            pin.authorize(if case == 2 { 0 } else { 1 }, &mut input)
                .is_err()
        );
        assert!(cancel.is_cancelled());
        assert_eq!(host.usage().physical_jobs, 1);
        assert_eq!(host.usage().retained_bytes, 8);
        drop(pin);
        assert_eq!(host.usage(), ResourceUsage::default());
    }
}
#[test]
fn lifetime_grants_allow_new_input_keys_but_reject_wrong_lifetime_tokens_before_reserving() {
    let host = fixture_host("retained-scope-positive");
    let gate = ExecutionGate::default();
    let key = fixture_key(1);
    let target = GrantTarget::new(
        "code-plan".into(),
        &key.session_epoch,
        key.plan_version,
        "node".into(),
    );
    gate.approve(target.clone());
    let source: Arc<[u8]> = Arc::from(&b"abcdefgh"[..]);
    let cancel = CancellationToken::default();
    let mismatch = RetainedReservation::reserve(
        &host,
        vec![RetainedBinding {
            module_slot: 1,
            scope: fixture_scope(
                &gate,
                &target,
                1,
                CancellationToken::default(),
                source.clone(),
            ),
        }],
        &cancel,
    );
    assert!(mismatch.is_err());
    assert_eq!(host.usage(), ResourceUsage::default());
    let pin = RetainedReservation::reserve(
        &host,
        vec![RetainedBinding {
            module_slot: 1,
            scope: fixture_scope(&gate, &target, 1, cancel.clone(), source.clone()),
        }],
        &cancel,
    )
    .unwrap();
    let mut input = fixture_scope(&gate, &target, 99, CancellationToken::default(), source);
    pin.authorize(1, &mut input).unwrap();
    assert_eq!(pin.usage().physical_jobs, 1);
    let second = pin.clone();
    drop(pin);
    assert_eq!(host.usage().physical_jobs, 1);
    drop(second);
    assert_eq!(host.usage(), ResourceUsage::default());
}

#[test]
#[ignore = "requires native Worker, pinned DLL and compiled public SDK consumer fixture"]
fn real_retained_module_loop() {
    let required =
        |name| PathBuf::from(std::env::var_os(name).expect("explicit retained fixture asset"));
    let worker = required("DIY_RANGE_WORKER_BINARY");
    let dll = required("DIY_RANGE_RUNTIME_DLL");
    let sdk = required("DIY_RESOURCE_SDK_CONSUMER_WASM");
    let policy: Value = serde_json::from_str(include_str!(
        "../../../tests/contracts/execution_control.json"
    ))
    .unwrap();
    let mut policy = policy["resourceProfiles"]["bounded"].clone();
    policy["maxPhysicalJobs"] = json!(1);
    let domain = format!(
        "retained-native-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let host = ExecutionHost::new(domain, ResourcePolicy::from_value(policy).unwrap()).unwrap();
    let owner_host = host.clone();
    let (response_send, response) = mpsc::sync_channel(1);
    let (release_send, release) = mpsc::sync_channel(1);
    let (ended_send, ended) = mpsc::sync_channel(1);
    let owner = thread::spawn(move || {
        let source: Arc<[u8]> = Arc::from(&[0, 255, 0xe6, 0xbc, 0xa2, 10, 13, 42][..]);
        let control = RunControl {
            walltime: Some(Duration::from_secs(30)),
            termination_grace: Duration::ZERO,
            ..RunControl::default()
        };
        let gate = ExecutionGate::default();
        let key:RunKey=serde_json::from_value(json!({"sessionEpoch":"11111111111111111111111111111111","planVersion":"1","runId":"1","deliveryGeneration":"1"})).unwrap();
        let target = GrantTarget::new(
            "retained-fixture".into(),
            &key.session_epoch,
            key.plan_version,
            "fixture.resource".into(),
        );
        gate.approve(target.clone());
        let bindings = (0..2)
            .map(|slot| RetainedBinding {
                module_slot: slot,
                scope: if slot == 0 {
                    fixture_empty(&gate, &target, 1, control.cancel.clone())
                } else {
                    ResourceRangeSession::open(
                        gate.clone(),
                        target.clone(),
                        key.clone(),
                        control.cancel.clone(),
                        [("prepared.bin".into(), source.clone())].into(),
                        ResourceRangeLimits {
                            max_resources: 1,
                            max_snapshot_bytes: 8,
                            max_read_bytes: 8,
                            max_total_read_bytes: 8,
                            max_reads: 1,
                            budget: Duration::from_secs(30),
                        },
                    )
                    .unwrap()
                },
            })
            .collect();
        // An outer cleanup pin plus Running's independent pin own the same charge.
        let retention =
            RetainedReservation::reserve(&owner_host, bindings, &control.cancel).unwrap();
        let driver_retention = retention.clone();
        let driver_control = control.clone();
        let mut profile = Profile::new().unwrap();
        let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
        let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
        let program = cache.copy(&worker, "worker.exe").unwrap();
        let runtime_path = cache.copy(&dll, "wasmtime.dll").unwrap();
        let state = runtime::state_fixture(&dll);
        let module0 = cache.write("counter.wasm", &state).unwrap();
        let sdk = std::fs::read(sdk).unwrap();
        let module1 = cache.write("consumer.wasm", &sdk).unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let args = vec![
            "--retained-resources-v1".into(),
            runtime_path.into_os_string(),
            "21".into(),
            "50000000".into(),
            "134217728".into(),
            module0.into_os_string(),
            state.len().to_string().into(),
            module1.into_os_string(),
            sdk.len().to_string().into(),
        ];
        let exchange = Box::new(move |mut output: std::fs::File, mut input: std::fs::File| {
            let result = (|| -> Result<Vec<u8>, String> {
                let mut results = Vec::new();
                let mut ready = [0; 12];
                output.read_exact(&mut ready).map_err(|e| e.to_string())?;
                if &ready[..8] != b"DIYRDY01"
                    || u32::from_le_bytes(ready[8..].try_into().unwrap()) != 2
                {
                    return Err("invalid retained readiness".into());
                }
                for sequence in 1u64..=20 {
                    let slot = ((sequence - 1) % 2) as u32;
                    let mut resources = if slot == 0 {
                        fixture_empty(&gate, &target, sequence, CancellationToken::default())
                    } else {
                        fixture_scope(
                            &gate,
                            &target,
                            sequence,
                            CancellationToken::default(),
                            source.clone(),
                        )
                    };
                    driver_retention
                        .authorize(slot, &mut resources)
                        .map_err(|error| error.code)?;
                    let mut header = b"DIYRET01".to_vec();
                    header.extend(sequence.to_le_bytes());
                    header.extend(slot.to_le_bytes());
                    input.write_all(&header).map_err(|e| e.to_string())?;
                    input.flush().map_err(|e| e.to_string())?;
                    let mut ack = [0; 20];
                    output.read_exact(&mut ack).map_err(|e| e.to_string())?;
                    if ack.as_slice() != header {
                        return Err("retained native identity mismatch".into());
                    }
                    let request = if slot == 0 {
                        json!({})
                    } else {
                        json!({"apiVersion":1,"node":{"kind":"fixture.resource","version":1},"operation":{"method":"executePrepared","config":{},"artifact":{"resource":{"id":1,"length":8},"format":"fixture.sdk-bytes","version":1},"inputs":{}}})
                    };
                    let bytes = resource_transport::serve_retained(
                        &mut output,
                        &mut input,
                        &serde_json::to_vec(&request).unwrap(),
                        &mut resources,
                        16384,
                    )
                    .map_err(|e| e.to_string())?;
                    results.push(wire::reply(&bytes).map_err(|e| e.code)?);
                }
                response_send
                    .send((results, driver_retention.clone(), driver_control))
                    .map_err(|e| e.to_string())?;
                release
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|e| e.to_string())?;
                Ok(Vec::new()) // dropping stdin requests genuine Worker shutdown.
            })();
            if let Err(error) = &result {
                eprintln!("retained exchange: {error}");
            }
            result
        });
        let exit = {
            let mut outcome = execute_retained(
                &profile,
                &job,
                &cache,
                &program,
                &args,
                RetainedWorkerRequest {
                    reservation: retention.clone(),
                    exchange,
                },
                &control,
            )
            .unwrap();
            loop {
                match outcome {
                    Outcome::Exited(exit) => break exit,
                    Outcome::Unconfirmed(owner) => outcome = owner.retry(Duration::from_secs(1)),
                    _ => panic!("retained fixture did not start"),
                }
            }
        };
        assert_eq!(exit.reason(), EndReason::Exited);
        assert_eq!(exit.exit_code(), Some(0));
        assert!(exit.input_delivered());
        cache.revoke_read_execute(&profile).unwrap();
        profile.delete().unwrap();
        std::fs::remove_dir_all(cache.path()).unwrap();
        drop(retention);
        ended_send.send(exit.process_id()).unwrap();
    });
    let (results, reused_pin, reused_control) = match response.recv_timeout(Duration::from_secs(30))
    {
        Ok(results) => results,
        Err(error) => {
            drop(release_send);
            let joined = owner.join();
            panic!("retained response failed: {error}; owner joined: {joined:?}");
        }
    };
    assert_eq!(results.len(), 20);
    for (index, result) in results.iter().enumerate() {
        if index % 2 == 0 {
            assert_eq!(*result, json!({"counter":1,"memory":1}));
        } else {
            assert_eq!(*result, json!({"copied":[0,255,230,188,162,10,13,42]}));
        }
    }
    assert!(
        ended.try_recv().is_err(),
        "logical replies must precede actual ending"
    );
    assert_eq!(host.usage().physical_jobs, 1);
    assert_eq!(host.usage().retained_bytes, 8);
    assert!(!reused_pin.is_closed());
    {
        let mut extra_profile = Profile::new().unwrap();
        let extra_job = Job::new().unwrap();
        let extra_cache =
            Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &extra_profile).unwrap();
        let error = execute_retained(
            &extra_profile,
            &extra_job,
            &extra_cache,
            &extra_cache.path().join("never-created.exe"),
            &["--retained-resources-v1".into()],
            RetainedWorkerRequest {
                reservation: reused_pin.clone(),
                exchange: Box::new(|_, _| panic!("duplicate reservation executed a driver")),
            },
            &reused_control,
        )
        .err()
        .expect("one reservation launched twice");
        assert_eq!(error, "retained reservation already launched");
        extra_profile.delete().unwrap();
        std::fs::remove_dir_all(extra_cache.path()).unwrap();
    }
    assert!(
        host.try_reserve(ResourceUsage {
            physical_jobs: 1,
            ..ResourceUsage::default()
        })
        .is_err()
    );
    release_send.send(()).unwrap();
    let pid = ended.recv_timeout(Duration::from_secs(10)).unwrap();
    owner.join().unwrap();
    assert_ne!(pid, 0);
    assert!(reused_pin.is_closed());
    assert_eq!(
        host.usage().physical_jobs,
        1,
        "cleanup pins are charged even after an exit receipt"
    );
    drop(reused_pin);
    assert_eq!(host.usage(), ResourceUsage::default());
    eprintln!(
        "retained native loop: 20 calls / 2 modules, clean Stores, logical replies before exit; original physical reservation released after true exit {pid}"
    );
}
