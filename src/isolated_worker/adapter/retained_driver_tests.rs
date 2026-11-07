//! Real bounded driver, not the ordinary Host or an IME benchmark.
use super::retained_tests::{fixture_empty, fixture_host, fixture_key, fixture_scope};
use super::*;
use crate::{
    execution_control::{ExecutionHost, ResourceUsage},
    isolated_worker::{
        DriverPolicy, DriverStop, RetainedBinding, RetainedClient, RetainedReservation,
        execute_retained, runtime,
    },
};
use std::{sync::mpsc, thread};

struct Fixture {
    client: Option<RetainedClient>,
    pin: Option<RetainedReservation>,
    host: ExecutionHost,
    gate: ExecutionGate,
    target: GrantTarget,
    source: Arc<[u8]>,
    ended: mpsc::Receiver<Result<ExitReceipt, String>>,
    owner: Option<thread::JoinHandle<()>>,
    unconfirmed: Arc<std::sync::atomic::AtomicU64>,
    lifetime_cancel: CancellationToken,
    receipt: Option<ExitReceipt>,
}
fn policy() -> DriverPolicy {
    DriverPolicy {
        idle_timeout: Duration::from_secs(5),
        input_timeout: Duration::from_secs(3),
        max_calls: 64,
        max_input_bytes: 4096,
        max_reply_bytes: 4096,
    }
}
impl Fixture {
    fn start(policy: DriverPolicy) -> Self {
        Self::start_with_counter_resources(policy, false)
    }
    fn start_with_counter_resources(policy: DriverPolicy, counter_resources: bool) -> Self {
        let required = |name| {
            PathBuf::from(std::env::var_os(name).expect("explicit retained driver fixture asset"))
        };
        let worker = required("DIY_RANGE_WORKER_BINARY");
        let dll = required("DIY_RANGE_RUNTIME_DLL");
        let sdk = required("DIY_RESOURCE_SDK_CONSUMER_WASM");
        let host = fixture_host("retained-driver");
        let gate = ExecutionGate::default();
        let key = fixture_key(1);
        let target = GrantTarget::new(
            "retained-driver-fixture".into(),
            &key.session_epoch,
            key.plan_version,
            "fixture.resource".into(),
        );
        gate.approve(target.clone());
        let source: Arc<[u8]> = Arc::from(&b"abcdefgh"[..]);
        let control = RunControl {
            walltime: Some(Duration::from_secs(30)),
            termination_grace: Duration::ZERO,
            ..RunControl::default()
        };
        let lifetime_cancel = control.cancel.clone();
        let pin = RetainedReservation::reserve(
            &host,
            vec![
                RetainedBinding {
                    module_slot: 0,
                    scope: if counter_resources {
                        fixture_scope(&gate, &target, 1, control.cancel.clone(), source.clone())
                    } else {
                        fixture_empty(&gate, &target, 1, control.cancel.clone())
                    },
                },
                RetainedBinding {
                    module_slot: 1,
                    scope: fixture_scope(&gate, &target, 1, control.cancel.clone(), source.clone()),
                },
            ],
            &control.cancel,
        )
        .unwrap();
        let calls = policy.max_calls;
        let (client, request) =
            RetainedClient::channel(host.clone(), pin.clone(), vec![0, 1], policy).unwrap();
        let cleanup_pin = pin.clone();
        let owner_host = host.clone();
        let unconfirmed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let owner_unconfirmed = unconfirmed.clone();
        let (send, ended) = mpsc::sync_channel(1);
        let owner = thread::spawn(move || {
            let result = (|| -> Result<ExitReceipt, String> {
                let cleanup_pin = cleanup_pin;
                let mut profile = Profile::new()?;
                let job = Job::with_memory_limit(Some(512 * 1024 * 1024))?;
                let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
                let program = cache.copy(&worker, "worker.exe")?;
                let runtime_path = cache.copy(&dll, "wasmtime.dll")?;
                let state = runtime::state_fixture(&dll);
                let module0 = cache.write("counter.wasm", &state)?;
                let sdk = std::fs::read(sdk).map_err(|e| e.to_string())?;
                let module1 = cache.write("consumer.wasm", &sdk)?;
                cache.allow_read_execute(&profile)?;
                let args = vec![
                    "--retained-resources-v1".into(),
                    runtime_path.into_os_string(),
                    calls.to_string().into(),
                    "5000000000".into(),
                    "134217728".into(),
                    module0.into_os_string(),
                    state.len().to_string().into(),
                    module1.into_os_string(),
                    sdk.len().to_string().into(),
                ];
                let result = {
                    match execute_retained(
                        &profile, &job, &cache, &program, &args, request, &control,
                    ) {
                        Err(error) => Err(error),
                        Ok(mut outcome) => {
                            let mut original_pid = None;
                            loop {
                                match outcome {
                                    Outcome::Exited(exit) => {
                                        if let Some(pid) = original_pid {
                                            assert_eq!(exit.process_id(), pid);
                                        }
                                        break Ok(exit);
                                    }
                                    Outcome::Unconfirmed(owner) => {
                                        assert_eq!(owner_host.usage().physical_jobs, 1);
                                        assert!(owner_host.usage().retained_bytes >= 8);
                                        if let Some(pid) = original_pid {
                                            assert_eq!(owner.process_id(), pid);
                                        }
                                        original_pid = Some(owner.process_id());
                                        owner_unconfirmed
                                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                                        outcome = owner.retry(Duration::from_secs(1))
                                    }
                                    _ => break Err("retained driver did not start".into()),
                                }
                            }
                        }
                    }
                };
                cache.revoke_read_execute(&profile)?;
                profile.delete()?;
                std::fs::remove_dir_all(cache.path()).map_err(|e| e.to_string())?;
                drop(cleanup_pin);
                result
            })();
            let _ = send.send(result);
        });
        Self {
            client: Some(client),
            pin: Some(pin),
            host,
            gate,
            target,
            source,
            ended,
            owner: Some(owner),
            unconfirmed,
            lifetime_cancel,
            receipt: None,
        }
    }
    fn close(&mut self) -> ExitReceipt {
        drop(self.client.take());
        let result = self.receipt.take().map_or_else(
            || {
                self.ended
                    .recv_timeout(Duration::from_secs(15))
                    .expect("original retained driver exit")
            },
            Ok,
        );
        self.owner.take().unwrap().join().unwrap();
        let exit = result.unwrap();
        assert!(self.pin.as_ref().unwrap().is_closed());
        drop(self.pin.take());
        assert_eq!(self.host.usage(), ResourceUsage::default());
        eprintln!(
            "retained original-process unconfirmed retries: {}",
            self.unconfirmed.load(std::sync::atomic::Ordering::Acquire)
        );
        exit
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.client.take());
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
        drop(self.pin.take());
    }
}
fn sdk_request() -> Vec<u8> {
    serde_json::to_vec(&json!({"apiVersion":1,"node":{"kind":"fixture.resource","version":1},"operation":{"method":"executePrepared","config":{},"artifact":{"resource":{"id":1,"length":8},"format":"fixture.sdk-bytes","version":1},"inputs":{}}})).unwrap()
}
fn complete_input(fixture: &mut Fixture, run: u64) -> Value {
    let cancel = CancellationToken::default();
    let empty = fixture_empty(&fixture.gate, &fixture.target, run, cancel.clone());
    let resource = fixture_scope(
        &fixture.gate,
        &fixture.target,
        run,
        cancel.clone(),
        fixture.source.clone(),
    );
    let mut input = fixture
        .client
        .as_mut()
        .unwrap()
        .begin_input(fixture_key(run), cancel)
        .unwrap();
    assert_eq!(
        input.call(b"{}".to_vec(), empty).unwrap(),
        json!({"counter":1,"memory":1})
    );
    let data = input.call(sdk_request(), resource).unwrap();
    input.finish(|| Ok(data)).unwrap()
}

#[test]
#[ignore = "requires actual native Worker, pinned DLL and public SDK consumer"]
fn real_retained_driver_faults() {
    for case in [
        "cancel-before",
        "cancel-between",
        "abandon",
        "wrong-scope",
        "wrong-context",
        "input-limit",
        "bad-reply",
        "peer-trap",
        "call-budget",
        "idle",
        "protect-publish",
        "exit-before-publish",
    ] {
        let mut p = policy();
        if case == "call-budget" {
            p.max_calls = 2;
        }
        if case == "idle" {
            p.idle_timeout = Duration::from_millis(100);
        }
        let mut fixture = Fixture::start(p);
        fixture
            .client
            .as_mut()
            .unwrap()
            .wait_ready(Duration::from_secs(10))
            .unwrap();
        let accepted = if matches!(
            case,
            "bad-reply" | "peer-trap" | "call-budget" | "exit-before-publish"
        ) {
            Some(complete_input(&mut fixture, 2))
        } else {
            None
        };
        let run = 3;
        let cancel = CancellationToken::default();
        match case {
            "cancel-before" => {
                cancel.cancel();
                assert_eq!(
                    fixture
                        .client
                        .as_mut()
                        .unwrap()
                        .begin_input(fixture_key(run), cancel)
                        .err()
                        .unwrap()
                        .code,
                    "worker-cancelled"
                );
            }
            "wrong-context" => {
                let mut key = serde_json::to_value(fixture_key(run)).unwrap();
                key["planVersion"] = json!("2");
                assert_eq!(
                    fixture
                        .client
                        .as_mut()
                        .unwrap()
                        .begin_input(serde_json::from_value(key).unwrap(), cancel)
                        .err()
                        .unwrap()
                        .code,
                    "retained-scope"
                );
            }
            "call-budget" => {
                assert_eq!(
                    fixture
                        .client
                        .as_mut()
                        .unwrap()
                        .begin_input(fixture_key(run), cancel)
                        .err()
                        .unwrap()
                        .code,
                    "retained-call-limit"
                );
            }
            "idle" => {
                thread::sleep(Duration::from_millis(250));
                assert!(fixture.pin.as_ref().unwrap().is_closed());
                assert_eq!(
                    fixture.client.as_ref().unwrap().stop_cause(),
                    Some(DriverStop::Idle)
                );
            }
            _ => {
                let empty = fixture_empty(&fixture.gate, &fixture.target, run, cancel.clone());
                let resource = fixture_scope(
                    &fixture.gate,
                    &fixture.target,
                    run,
                    cancel.clone(),
                    fixture.source.clone(),
                );
                let mut input = fixture
                    .client
                    .as_mut()
                    .unwrap()
                    .begin_input(fixture_key(run), cancel.clone())
                    .unwrap();
                match case {
                    "wrong-scope" => {
                        assert_eq!(
                            input.call(b"{}".to_vec(), resource).unwrap_err().code,
                            "retained-scope"
                        );
                    }
                    "input-limit" => {
                        assert_eq!(
                            input.call(vec![b'x'; 4097], empty).unwrap_err().code,
                            "retained-input-limit"
                        );
                    }
                    "bad-reply" => {
                        assert_eq!(
                            input.call(b"?".to_vec(), empty).unwrap_err().code,
                            "worker-reply"
                        );
                    }
                    "peer-trap" => {
                        assert!(input.call(b"~".to_vec(), empty).is_err());
                    }
                    "cancel-between" => {
                        input.call(b"{}".to_vec(), empty).unwrap();
                        cancel.cancel();
                        thread::sleep(Duration::from_millis(80));
                        assert!(fixture.pin.as_ref().unwrap().is_closed());
                    }
                    "abandon" => {
                        input.call(b"{}".to_vec(), empty).unwrap();
                    }
                    "protect-publish" | "exit-before-publish" => {
                        input.call(b"{}".to_vec(), empty).unwrap();
                        input.call(sdk_request(), resource).unwrap();
                        if case == "protect-publish" {
                            fixture.gate.set_protection(true, false);
                        } else {
                            fixture.lifetime_cancel.cancel();
                            fixture.receipt = Some(
                                fixture
                                    .ended
                                    .recv_timeout(Duration::from_secs(15))
                                    .expect("original exit before publication")
                                    .unwrap(),
                            );
                        }
                        let mut published = false;
                        assert!(
                            input
                                .finish(|| {
                                    published = true;
                                    Ok(())
                                })
                                .is_err()
                        );
                        assert!(!published);
                        assert!(fixture.pin.as_ref().unwrap().is_closed());
                        let exit = fixture.close();
                        assert_ne!(exit.process_id(), 0);
                        if let Some(value) = accepted {
                            assert_eq!(value, json!({"copied":[97,98,99,100,101,102,103,104]}));
                        }
                        eprintln!(
                            "bounded retained fault passed: {case}; original exit {:?}/{:?}",
                            exit.reason(),
                            exit.exit_code()
                        );
                        continue;
                    }
                    _ => unreachable!(),
                }
                drop(input);
            }
        }
        assert_eq!(
            fixture.host.usage().physical_jobs,
            1,
            "{case}: logical failure freed a physical pin"
        );
        let exit = fixture.close();
        assert_ne!(exit.process_id(), 0);
        if case == "peer-trap" {
            assert_eq!(exit.exit_code(), Some(1));
        }
        if let Some(value) = accepted {
            assert_eq!(
                value,
                json!({"copied":[97,98,99,100,101,102,103,104]}),
                "{case}: later failure rewrote prior result"
            );
        }
        eprintln!(
            "bounded retained fault passed: {case}; original exit {:?}/{:?}",
            exit.reason(),
            exit.exit_code()
        );
    }
}

#[test]
#[ignore = "requires actual native Worker, pinned DLL and public SDK consumer"]
fn real_retained_driver_inflight_cancel_and_deadline() {
    for deadline in [false, true] {
        let mut p = policy();
        if deadline {
            p.input_timeout = Duration::from_millis(250);
        }
        let mut fixture = Fixture::start_with_counter_resources(p, true);
        fixture
            .client
            .as_mut()
            .unwrap()
            .wait_ready(Duration::from_secs(10))
            .unwrap();
        let cancel = CancellationToken::default();
        let run = 2;
        let (started_send, started) = mpsc::sync_channel(1);
        let mut scope = fixture_scope(
            &fixture.gate,
            &fixture.target,
            run,
            cancel.clone(),
            fixture.source.clone(),
        );
        scope.after_read = Some(Box::new(move || {
            let _ = started_send.try_send(());
        }));
        let (canceller, deadline_started) = if !deadline {
            let cancel = cancel.clone();
            (
                Some(thread::spawn(move || {
                    started
                        .recv_timeout(Duration::from_secs(10))
                        .expect("Wasm must enter its resource callback");
                    thread::sleep(Duration::from_millis(10));
                    cancel.cancel();
                })),
                None,
            )
        } else {
            (None, Some(started))
        };
        let mut input = fixture
            .client
            .as_mut()
            .unwrap()
            .begin_input(fixture_key(run), cancel)
            .unwrap();
        let result = input.call(b"!".to_vec(), scope);
        assert!(result.is_err());
        drop(input);
        if let Some(canceller) = canceller {
            canceller.join().unwrap();
        }
        if let Some(started) = deadline_started {
            started
                .try_recv()
                .expect("deadline must interrupt an entered Wasm call");
        }
        assert_eq!(
            fixture.client.as_ref().unwrap().stop_cause(),
            Some(if deadline {
                DriverStop::InputDeadline
            } else {
                DriverStop::InputCancelled
            })
        );
        assert_eq!(fixture.host.usage().physical_jobs, 1);
        let exit = fixture.close();
        assert_ne!(exit.process_id(), 0);
        eprintln!(
            "bounded retained in-flight {} passed with actual exit {:?}",
            if deadline { "deadline" } else { "cancellation" },
            exit.exit_code()
        );
    }
}

#[test]
#[ignore = "requires actual native Worker, pinned DLL and public SDK consumer"]
fn real_bounded_retained_driver() {
    let mut fixture = Fixture::start(policy());
    fixture
        .client
        .as_mut()
        .unwrap()
        .wait_ready(Duration::from_secs(10))
        .unwrap();
    let mut accepted = Vec::new();
    for run in 2..12 {
        let cancel = CancellationToken::default();
        let empty = fixture_empty(&fixture.gate, &fixture.target, run, cancel.clone());
        let resource = fixture_scope(
            &fixture.gate,
            &fixture.target,
            run,
            cancel.clone(),
            fixture.source.clone(),
        );
        let mut input = fixture
            .client
            .as_mut()
            .unwrap()
            .begin_input(fixture_key(run), cancel)
            .unwrap();
        assert_eq!(
            input.call(b"{}".to_vec(), empty).unwrap(),
            json!({"counter":1,"memory":1})
        );
        let data = input.call(sdk_request(), resource).unwrap();
        assert_eq!(data, json!({"copied":[97,98,99,100,101,102,103,104]}));
        accepted.push(input.finish(|| Ok(data)).unwrap());
        assert_eq!(fixture.host.usage().physical_jobs, 1);
        assert_eq!(fixture.host.usage().queued_jobs, 0);
        assert_eq!(fixture.host.usage().reserved_result_bytes, 0);
    }
    assert_eq!(accepted.len(), 10);
    assert!(fixture.ended.try_recv().is_err());
    fixture.client.as_ref().unwrap().stop(DriverStop::Closed);
    let exit = fixture.close();
    assert_ne!(exit.process_id(), 0);
    assert_eq!(
        accepted.len(),
        10,
        "later lifetime close cannot rewrite accepted results"
    );
    eprintln!(
        "bounded retained driver: 10 inputs / 20 stages, strict replies, scoped publication, buffers released per input, physical pin until exit"
    );
}
