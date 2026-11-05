//! Direct execution of an explicitly bounded subset of the adopted JSON cases.
//! Unknown operations/expected fields fail; the remaining cases are documented.
use diy_ime2_core::{
    Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue, RecoveryClaim},
    execution_control::{
        CancellationToken, CompletionPermit, ControlDecision, ControlFrame, ControlInput,
        ControlNode, DecimalId, DeferredNode, ExecutionHost, ExecutionPump, Milliseconds,
        OwnedInvocation, ResourcePolicy, RunBudgets, RunKey, SessionEpoch, StartRequest,
        WakeReason,
    },
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RunContext,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

macro_rules! direct_cases {
    ($($name:ident => $id:literal),+ $(,)?) => {
        const DIRECT: &[&str] = &[$($id),+];
        $(#[test] fn $name() { run_case($id); })+
    };
}
direct_cases! {
    a01_normal => "A01-normal-completion",
    a02_pending => "A02-pending-is-not-no-output",
    a03_first_data => "A03-first-data-cancels-unneeded",
    a04_all_inputs => "A04-normal-merge-waits-all",
    a05_shared_demand => "A05-shared-job-keeps-owner",
    a06_timer => "A06-wake-without-new-input",
    a07_pure_poll => "A07-poll-does-not-reobserve",
    t01_before => "T01-before-run-deadline",
    t02_exact => "T02-exact-run-deadline",
    t03_after => "T03-after-run-deadline",
    t04_no_backdate => "T04-worker-timestamp-does-not-backdate",
    t05_recovery => "T05-job-deadline-can-recover",
    t06_unlimited => "T06-unlimited-does-not-expire",
    c01_cancel_first => "C01-cancel-before-completion-same-time",
    c02_complete_first => "C02-complete-before-cancel-same-time",
    c03_repeat_cancel => "C03-repeat-cancel-no-double-release",
    c04_ack_is_not_end => "C04-ack-of-receipt-is-not-termination",
    a08_no_output => "A08-node-no-output-is-a-resolved-result",
    g10_wrong_key => "G10-get-run-does-not-retarget-a-key",
    g11_distinct_runs => "G11-same-input-keeps-separate-runs",
    g01_invalidate => "G01-input-invalidation-rejects-old-result",
    g02_prepare => "G02-prepare-does-not-invalidate",
    g03_activate => "G03-activate-invalidates-but-reclaims-old-job",
    g04_rollback => "G04-rollback-never-resurrects-version",
    g06_duplicate_completion => "G06-duplicate-completion-reclaims-once",
    g07_input_conflict => "G07-revision-content-conflict",
    g08_fixed_run => "G08-fixed-run-survives-input-update",
    b01_replay => "B01-same-start-is-idempotent",
    b02_replay_conflict => "B02-start-key-with-changed-payload",
    b04_retired_start => "B04-key-after-retention-cannot-relaunch",
    b05_result_gone => "B05-expired-result-is-explicitly-gone",
    b06_result_unknown => "B06-tombstone-loss-does-not-prove-not-started",
    b10_replay_after_activation => "B10-replay-is-not-revalidated-against-new-plan",
    t10_backward_clock => "T10-clock-backwards-does-not-change-state",
    r04_invalid_profile => "R04-invalid-profile-refuses-new-controller",
    b09_exact_ids => "B09-u64-ids-stay-strings",
    t08_invalid_budgets => "T08-invalid-budget-is-pre-admission",
    t09_checked_add_limit => "T09-duration-overflow-is-rejected",
    r01_cancelled_capacity => "R01-cancelled-work-keeps-capacity",
    r02_queue_expiry => "R02-queue-expiry-before-same-time-slot-return",
    r03_result_limit => "R03-oversized-result-is-not-success",
    r05_no_queue => "R05-zero-queue-refuses-before-admission",
    r06_later_fanout => "R06-later-ready-work-may-fail-without-unaccepting-run",
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("contracts/execution_control.json")).unwrap()
}
fn id(text: &str) -> DecimalId {
    serde_json::from_value(json!(text)).unwrap()
}
fn output(value: &Value) -> ErrorAwareExecution {
    let value = match value["status"].as_str().unwrap() {
        "data" => ErrorAwareValue::Data(Arc::new(
            serde_json::from_value::<Packet>(value["packet"].clone()).unwrap(),
        )),
        "no-output" => ErrorAwareValue::NoOutput,
        other => panic!("unsupported fixture value {other}"),
    };
    ErrorAwareExecution::new([("out".into(), value)].into())
}
#[derive(Clone)]
struct Started {
    key: RunKey,
    node: String,
    work: TestWork,
}
#[derive(Clone)]
enum TestWork {
    Deferred(CompletionPermit, CancellationToken),
    Sync(Arc<SyncReply>),
}
struct SyncReply {
    sender: mpsc::Sender<Outputs>,
    sent: AtomicBool,
}
impl SyncReply {
    fn send(&self, outputs: Outputs) -> bool {
        !self.sent.swap(true, Ordering::AcqRel) && self.sender.send(outputs).is_ok()
    }
}
impl Started {
    fn complete(&self, value: &Value) -> bool {
        let result = output(value);
        match &self.work {
            TestWork::Deferred(permit, _) => permit.complete(Ok(result)),
            TestWork::Sync(reply) => reply.send(
                result
                    .outputs
                    .into_iter()
                    .map(|(port, value)| {
                        (
                            port,
                            match value {
                                ErrorAwareValue::Data(packet) => OutputValue::Data(packet),
                                ErrorAwareValue::NoOutput => OutputValue::NoOutput,
                                ErrorAwareValue::Error(_) => {
                                    panic!("fixture sync output is not legacy data")
                                }
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }
    fn cancelled(&self) -> bool {
        matches!(&self.work,TestWork::Deferred(_,cancel) if cancel.is_cancelled())
    }
    fn acknowledge_finish(&self) -> bool {
        match &self.work {
            TestWork::Deferred(permit, _) => permit.finish_cancelled(),
            TestWork::Sync(_) => {
                panic!("sync jobs do not expose producer cancellation acknowledgements")
            }
        }
    }
    fn release(&self) {
        match &self.work {
            TestWork::Deferred(permit, _) => {
                permit.finish_cancelled();
            }
            TestWork::Sync(reply) => {
                reply.send([("out".into(), OutputValue::NoOutput)].into());
            }
        }
    }
}
type RunKeys = Arc<Mutex<BTreeMap<u64, RunKey>>>;
struct SyncStep {
    send: mpsc::Sender<Started>,
    keys: RunKeys,
}
impl Node for SyncStep {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "fixture.sync-step",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        _: &Inputs,
        _: &Value,
    ) -> Result<Outputs, CoreError> {
        let key = self.keys.lock().unwrap()[&context.run_id].clone();
        let (sender, gate) = mpsc::channel();
        self.send
            .send(Started {
                key,
                node: context.node_id.into(),
                work: TestWork::Sync(Arc::new(SyncReply {
                    sender,
                    sent: AtomicBool::new(false),
                })),
            })
            .unwrap();
        gate.recv()
            .map_err(|_| CoreError::new("test-gate-closed", "sync test producer dropped"))
    }
}
struct Deferred(mpsc::Sender<Started>);
impl DeferredNode for Deferred {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "fixture.delay",
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
        self.0
            .send(Started {
                key: inv.key().clone(),
                node: inv.context().node_id.into(),
                work: TestWork::Deferred(permit, cancel),
            })
            .unwrap();
        Ok(())
    }
}
struct Join;
impl Node for Join {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "fixture.join",
            1,
            vec![
                InputPort::required(Port::packet("a")),
                InputPort::required(Port::packet("b")),
            ],
            vec![Port::packet("out")],
        )
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        panic!("the all-input join must not start while one input is Pending")
    }
}
struct Policy(Arc<Mutex<Vec<(WakeReason, u64)>>>);
impl ControlNode for Policy {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "fixture.policy",
            1,
            vec![
                InputPort::optional(Port::packet("a")),
                InputPort::optional(Port::packet("b")),
            ],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec!["a".into(), "b".into()]
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if config == &json!({})
            || matches!(
                config["mode"].as_str(),
                Some("first" | "fallback" | "observer")
            )
        {
            Ok(())
        } else {
            Err(CoreError::new(
                "invalid-config",
                "unsupported fixture policy",
            ))
        }
    }
    fn observe(
        &self,
        _: RunContext<'_>,
        frame: &ControlFrame,
        config: &Value,
    ) -> Result<ControlDecision, CoreError> {
        self.0
            .lock()
            .unwrap()
            .push((frame.reason, frame.elapsed_ms));
        let mode = config["mode"].as_str().unwrap();
        if mode == "observer" {
            return Ok(ControlDecision::Wait {
                wake_after_ms: config["wake"]
                    .as_u64()
                    .map(|n| Milliseconds::new(n).unwrap()),
            });
        }
        if mode == "fallback" {
            if let (
                ControlInput::Resolved(ErrorAwareValue::Error(errors)),
                ControlInput::Resolved(ErrorAwareValue::Data(packet)),
            ) = (&frame.inputs["a"], &frame.inputs["b"])
            {
                let mut result = ErrorAwareExecution::new(
                    [("out".into(), ErrorAwareValue::Data(Arc::clone(packet)))].into(),
                );
                result.recoveries.push(RecoveryClaim {
                    input: "a".into(),
                    output: "out".into(),
                    causes: errors
                        .causes()
                        .iter()
                        .map(|cause| cause.id.clone())
                        .collect(),
                });
                return Ok(ControlDecision::Finish(result));
            }
        } else {
            // This is the fixture's test-only first-data policy, not Core meaning.
            for value in frame.inputs.values() {
                if let ControlInput::Resolved(ErrorAwareValue::Data(_)) = value {
                    return Ok(ControlDecision::Finish(ErrorAwareExecution::new(
                        [(
                            "out".into(),
                            match value {
                                ControlInput::Resolved(value) => value.clone(),
                                _ => unreachable!(),
                            },
                        )]
                        .into(),
                    )));
                }
            }
        }
        Ok(ControlDecision::Wait {
            wake_after_ms: None,
        })
    }
}
struct Runner {
    host: ExecutionHost,
    pump: ExecutionPump,
    epoch: SessionEpoch,
    keys: BTreeMap<String, RunKey>,
    jobs: Vec<Started>,
    entered: mpsc::Receiver<Started>,
    calls: Arc<Mutex<Vec<(WakeReason, u64)>>>,
    packets: BTreeMap<String, Value>,
    errors: Vec<String>,
    accepted: u64,
    reclaimed: u64,
    initial_calls: usize,
    initial_revision: u64,
    initial_version: DecimalId,
    initial_prepared: bool,
    cancel_reply: Option<String>,
    job_count: usize,
    registry: Registry,
    requests: BTreeMap<String, StartRequest>,
    replayed: Option<bool>,
    same_key: Vec<bool>,
    clock_base: Duration,
    accepted_values: Vec<Value>,
    rejected_values: Vec<Value>,
    run_keys: RunKeys,
}
impl Runner {
    fn new(case: &Value, fixture: &Value) -> Self {
        let initial = &case["initial"];
        let host = ExecutionHost::new(
            format!("direct-{}", case["id"].as_str().unwrap()),
            ResourcePolicy::from_value(
                fixture["resourceProfiles"][initial["resourceProfile"].as_str().unwrap()].clone(),
            )
            .unwrap(),
        )
        .unwrap();
        let epoch = SessionEpoch::new(initial["sessionEpoch"].as_str().unwrap()).unwrap();
        let workload = initial["workload"].as_str().unwrap();
        let specification = &fixture["workloads"][workload];
        let jobs = specification["jobs"].as_array().unwrap();
        let mut nodes: Vec<Value> = jobs
            .iter()
            .map(|name| json!({"id":name,"kind":if workload=="fanout"&&name!="seed"{"fixture.sync-step"}else{"fixture.delay"},"version":1}))
            .collect();
        let mut edges = vec![];
        let mut connect = |from: &str, to: &str, port: &str| {
            edges.push(json!({"from":{"node":from,"port":"out"},"to":{"node":to,"port":port}}))
        };
        match workload {
            "single" => {}
            "race" | "shared" => {
                nodes.push(json!({"id":"selection","kind":"fixture.policy","version":1,"config":{"mode":"first"}}));
                connect("fast", "selection", "a");
                connect(
                    if workload == "shared" {
                        "shared"
                    } else {
                        "slow"
                    },
                    "selection",
                    "b",
                );
            }
            "waitBoth" => {
                nodes.push(json!({"id":"joined","kind":"fixture.join","version":1}));
                connect("fast", "joined", "a");
                connect("slow", "joined", "b");
            }
            "fanout" => {
                nodes.push(json!({"id":"joined","kind":"fixture.join","version":1}));
                connect("seed", "a", "in");
                connect("seed", "b", "in");
                connect("a", "joined", "a");
                connect("b", "joined", "b");
            }
            "fallback" => {
                nodes.push(json!({"id":"recovery","kind":"fixture.policy","version":1,"config":{"mode":"fallback"}}));
                connect("main", "recovery", "a");
                connect("original", "recovery", "b");
            }
            "observer" => {
                let wake = case["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|event| event["op"] == "wait-observer")
                    .map(|event| event["wakeAfterMs"].clone())
                    .unwrap_or(Value::Null);
                nodes.push(json!({"id":"observer","kind":"fixture.policy","version":1,"config":{"mode":"observer","wake":wake}}));
                connect("main", "observer", "a");
            }
            other => panic!("unsupported workload {other}"),
        }
        let (send, entered) = mpsc::channel();
        let run_keys = Arc::new(Mutex::new(BTreeMap::new()));
        let calls = Arc::new(Mutex::new(vec![]));
        let mut registry = Registry::new();
        registry
            .register(SyncStep {
                send: send.clone(),
                keys: Arc::clone(&run_keys),
            })
            .unwrap();
        registry.register_deferred(Deferred(send)).unwrap();
        registry.register(Join).unwrap();
        registry
            .register_control(Policy(Arc::clone(&calls)))
            .unwrap();
        let graph = Graph::from_json(&json!({"version":1,"nodes":nodes,"edges":edges}).to_string())
            .unwrap();
        let plan = ExecutionPlan::compile(&graph, &registry).unwrap();
        let mut pump = ExecutionPump::new(host.clone()).unwrap();
        pump.open_session(
            host.clone(),
            epoch.clone(),
            plan,
            id(initial["planVersion"].as_str().unwrap()),
        )
        .unwrap();
        if let Some(revision) = initial["latestInputRevision"].as_u64() {
            pump.invalidate_input(
                &epoch,
                id(initial["planVersion"].as_str().unwrap()),
                revision,
            )
            .unwrap();
        }
        let clock_base = if initial["clockDomain"] == "checked-add-overflow-test" {
            Duration::MAX
                .checked_sub(Duration::from_millis(
                    initial["nativeRemainingMs"].as_u64().unwrap(),
                ))
                .unwrap()
        } else {
            Duration::ZERO
        };
        pump.advance_to(clock_base).unwrap();
        let initial_revision = pump.session_revision(&epoch).unwrap();
        let initial_version = pump.plan_version(&epoch).unwrap();
        let initial_prepared = pump.session_has_pending(&epoch).unwrap();
        Self {
            host,
            pump,
            epoch,
            keys: BTreeMap::new(),
            jobs: vec![],
            entered,
            calls,
            packets: BTreeMap::new(),
            errors: vec![],
            accepted: 0,
            reclaimed: 0,
            initial_calls: 0,
            initial_revision,
            initial_version,
            initial_prepared,
            cancel_reply: None,
            job_count: jobs.len(),
            registry,
            requests: BTreeMap::new(),
            replayed: None,
            same_key: vec![],
            clock_base,
            accepted_values: vec![],
            rejected_values: vec![],
            run_keys,
        }
    }
    fn tick(&mut self) {
        self.pump.advance_to(self.pump.now()).unwrap();
        self.jobs.extend(self.entered.try_iter());
    }
    fn settle(&mut self, mut predicate: impl FnMut(&Self) -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.tick();
            if predicate(self) {
                return;
            }
            assert!(Instant::now() < until, "fixture did not settle");
            let _ = self
                .pump
                .wake_signal()
                .wait(Some(Duration::from_millis(1)))
                .unwrap();
        }
    }
    fn job(&self, run: &str, node: &str) -> Started {
        self.jobs
            .iter()
            .find(|job| job.key == self.keys[run] && job.node == node)
            .unwrap()
            .clone()
    }
    fn event(&mut self, event: &Value, case: &Value, fixture: &Value) {
        let at = self
            .clock_base
            .checked_add(Duration::from_millis(event["atMs"].as_u64().unwrap()))
            .unwrap();
        let previous_now = self.pump.now();
        let previous_usage = self.host.usage();
        let previous_revision = self.pump.session_revision(&self.epoch).unwrap();
        let previous_snapshots = (at < previous_now).then(|| {
            self.keys
                .values()
                .map(|key| serde_json::to_value(self.pump.get_run(key).unwrap()).unwrap())
                .collect::<Vec<_>>()
        });
        if let Err(error) = self.pump.advance_to(at) {
            assert_eq!(event["op"], "advance-clock");
            assert_eq!(case["initial"]["allowClockBackwardsAttempt"], true);
            assert!(at < previous_now);
            self.errors.push(error.code);
            assert_eq!(self.pump.now(), previous_now);
            assert_eq!(self.host.usage(), previous_usage);
            assert_eq!(
                self.pump.session_revision(&self.epoch).unwrap(),
                previous_revision
            );
            assert_eq!(
                self.keys
                    .values()
                    .map(|key| serde_json::to_value(self.pump.get_run(key).unwrap()).unwrap())
                    .collect::<Vec<_>>(),
                previous_snapshots.unwrap()
            );
            return;
        }
        let run = event["run"].as_str().unwrap_or("");
        match event["op"].as_str().unwrap() {
            "start" => {
                let targets =
                    fixture["workloads"][case["initial"]["workload"].as_str().unwrap()]["targets"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|target| {
                            let (node, port) = target.as_str().unwrap().split_once('.').unwrap();
                            json!({"node":node,"port":port})
                        })
                        .collect::<Vec<_>>();
                let mut request = json!({"startKey":event["startKey"],"expectedPlanVersion":case["initial"]["planVersion"],"targets":targets,"runBudgetMs":event["runBudgetMs"],"jobBudgets":event["jobBudgets"]});
                if event["fixed"] == true {
                    assert!(event.get("input").is_none());
                } else {
                    request["input"] = event["input"].clone();
                }
                let request: StartRequest = serde_json::from_value(request).unwrap();
                self.requests.insert(run.into(), request.clone());
                let receipt = match self.pump.start(&self.epoch, request) {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        if case["initial"]["clockDomain"] == "checked-add-overflow-test" {
                            assert_eq!(self.host.usage(), previous_usage);
                            assert_eq!(
                                self.pump.session_revision(&self.epoch).unwrap(),
                                previous_revision
                            );
                            assert_eq!(
                                self.pump
                                    .get_start(&self.epoch, id(event["startKey"].as_str().unwrap()))
                                    .unwrap_err()
                                    .code,
                                "start-not-seen"
                            );
                        }
                        self.errors.push(error.code);
                        self.tick();
                        return;
                    }
                };
                self.accepted += u64::from(!receipt.replayed);
                self.keys.insert(run.into(), receipt.key.clone());
                self.run_keys
                    .lock()
                    .unwrap()
                    .insert(receipt.key.run_id.get(), receipt.key.clone());
                let physical = self.pump.get_run(&receipt.key).unwrap().physical;
                let expected =
                    (physical.running + physical.cancel_requested + physical.stalled) as usize;
                self.settle(|h| {
                    h.jobs.iter().filter(|job| job.key == receipt.key).count() >= expected
                });
                self.initial_calls = self.calls.lock().unwrap().len();
            }
            "complete" | "complete-oversized" => {
                let node = event["job"].as_str().unwrap();
                let job = self.job(run, node);
                let before = self.host.usage().physical_jobs;
                let before_jobs = self.jobs.len();
                let value = if event["op"] == "complete-oversized" {
                    json!({"status":"data","packet":{"version":1,"root":{"type":"text","text":"x".repeat(event["bytes"].as_u64().unwrap() as usize)}}})
                } else {
                    event["value"].clone()
                };
                let first = job.complete(&value);
                self.reclaimed += u64::from(first);
                if value["status"] == "data" {
                    self.packets
                        .insert(format!("{run}/{node}"), value["packet"].clone());
                }
                if first {
                    self.settle(|h| {
                        h.host.usage().physical_jobs < before || h.jobs.len() > before_jobs
                    });
                } else {
                    self.tick();
                    assert_eq!(self.host.usage().physical_jobs, before);
                }
            }
            "cancel" => {
                let key = &self.keys[run];
                let before = self.pump.get_run(key).unwrap().result.is_some();
                self.pump.cancel(key).unwrap();
                self.cancel_reply = Some(
                    if before {
                        "already-terminal"
                    } else {
                        "cancelled"
                    }
                    .into(),
                );
            }
            "cancel-ack" => {
                let job = self.job(run, event["job"].as_str().unwrap());
                if event["physicallyFinished"] == true {
                    let before = self.host.usage().physical_jobs;
                    let first = job.acknowledge_finish();
                    self.reclaimed += u64::from(first);
                    if first {
                        self.settle(|h| h.host.usage().physical_jobs < before);
                    }
                } else {
                    assert!(job.cancelled());
                }
            }
            "poll" => {
                let mut key = serde_json::to_value(&self.keys[run]).unwrap();
                if let Some(overrides) = event.get("keyOverride") {
                    for (field, value) in overrides.as_object().unwrap() {
                        key[field] = value.clone();
                    }
                }
                let key = serde_json::from_value(key).unwrap();
                if let Err(error) = self.pump.get_run(&key) {
                    self.errors.push(error.code);
                }
            }
            "advance-clock" => {}
            "validate-id" => match serde_json::from_value::<DecimalId>(event["value"].clone()) {
                Ok(value) => self
                    .accepted_values
                    .push(serde_json::to_value(value).unwrap()),
                Err(_) => self.rejected_values.push(event["value"].clone()),
            },
            "validate-budgets" => {
                for value in event["values"].as_array().unwrap() {
                    match RunBudgets::from_value(json!({"runBudgetMs":value,"jobBudgets":[]})) {
                        Ok(_) => self.accepted_values.push(value.clone()),
                        Err(error) => {
                            self.errors.push(error.code);
                            self.rejected_values.push(value.clone());
                        }
                    }
                }
                assert_eq!(self.host.usage(), previous_usage);
            }
            "invalidate-input" => {
                self.pump
                    .invalidate_input(
                        &self.epoch,
                        self.pump.plan_version(&self.epoch).unwrap(),
                        event["revision"].as_u64().unwrap(),
                    )
                    .unwrap();
            }
            "prepare" => {
                assert_eq!(event["planAlias"], "next");
                // The new plan intentionally has no old `main.out` target. A
                // replay must use its recorded payload before current validation.
                let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":"replacement","kind":"fixture.delay","version":1}],"edges":[]}).to_string()).unwrap();
                let plan = ExecutionPlan::compile(&graph, &self.registry).unwrap();
                self.pump.prepare(&self.epoch, plan).unwrap();
            }
            "activate" => {
                assert_eq!(event["planAlias"], "next");
                assert!(self.pump.activate(&self.epoch).unwrap());
            }
            "rollback" => {
                assert!(self.pump.rollback(&self.epoch).unwrap());
            }
            "repeat-start" => {
                let mut request = self.requests[run].clone();
                request.start_key = id(event["startKey"].as_str().unwrap());
                if event["samePayload"] == false {
                    request.input.as_mut().unwrap().text.push_str("changed");
                } else {
                    assert_eq!(event["samePayload"], true);
                }
                match self.pump.start(&self.epoch, request) {
                    Ok(receipt) => {
                        self.replayed = Some(receipt.replayed);
                        self.accepted += u64::from(!receipt.replayed);
                        self.same_key.push(receipt.key == self.keys[run]);
                    }
                    Err(error) => self.errors.push(error.code),
                }
            }
            "wait-observer" => {
                assert!(
                    self.calls
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|(reason, _)| *reason == WakeReason::Started)
                );
                assert!(self.pump.next_deadline().is_some_and(|deadline| deadline
                    == at + Duration::from_millis(event["wakeAfterMs"].as_u64().unwrap())));
            }
            other => panic!("unsupported event {other}"),
        }
        self.tick();
    }
    fn expected(&self, expected: &Value) {
        for (name, want) in expected.as_object().unwrap() {
            if let Some(key) = self.keys.get(name) {
                let snapshot = self.pump.get_run(key).unwrap();
                let value = serde_json::to_value(&snapshot).unwrap();
                for (field, want) in want.as_object().unwrap() {
                    match field.as_str() {
                        "state" | "current" => assert_eq!(&value[field], want, "{name}.{field}"),
                        "reason" | "delivery" => {
                            assert_eq!(&value["result"][field], want, "{name}.{field}")
                        }
                        "hasResult" => assert_eq!(json!(snapshot.result.is_some()), *want),
                        "selectedJob" => {
                            let packet =
                                &self.packets[&format!("{name}/{}", want.as_str().unwrap())];
                            assert!(
                                snapshot
                                    .result
                                    .as_ref()
                                    .unwrap()
                                    .ports
                                    .iter()
                                    .any(|port| port.value["packet"] == *packet)
                            );
                        }
                        other => panic!("unsupported run expectation {other}"),
                    }
                }
                continue;
            }
            match name.as_str() {
                "physicalJobs" => assert_eq!(json!(self.host.usage().physical_jobs), *want),
                "queuedJobs" => assert_eq!(json!(self.host.usage().queued_jobs), *want),
                "startedJobs" => assert_eq!(json!(self.jobs.len()), *want),
                "r2StartedJobs" => assert_eq!(
                    json!(
                        self.jobs
                            .iter()
                            .filter(|job| job.key == self.keys["r2"])
                            .count()
                    ),
                    *want
                ),
                "latestInputRevision" => assert_eq!(
                    json!(
                        self.pump
                            .session_latest_input_revision(&self.epoch)
                            .unwrap()
                    ),
                    *want
                ),
                "acceptedRuns" => assert_eq!(json!(self.accepted), *want),
                "acceptedValues" => assert_eq!(json!(self.accepted_values), *want),
                "rejectedValues" => assert_eq!(json!(self.rejected_values), *want),
                "activePlanVersion" => assert_eq!(
                    serde_json::to_value(self.pump.plan_version(&self.epoch).unwrap()).unwrap(),
                    *want
                ),
                "clockMs" => assert_eq!(json!(self.pump.now().as_millis()), *want),
                "replayed" => assert_eq!(json!(self.replayed), *want),
                "sameRunKey" => {
                    assert!(!self.same_key.is_empty());
                    assert_eq!(json!(self.same_key.iter().all(|same| *same)), *want);
                }
                "automaticResubmissions" => assert_eq!(
                    json!(
                        self.jobs
                            .len()
                            .checked_sub(self.accepted as usize * self.job_count)
                            .unwrap()
                    ),
                    *want
                ),
                "reclaimedJobs" => assert_eq!(json!(self.reclaimed), *want),
                "cancelReply" => assert_eq!(json!(self.cancel_reply), *want),
                "cancelRequested" | "stalledJobs" => {
                    let mut actual = vec![];
                    for job in &self.jobs {
                        if job.cancelled() {
                            let alias = self
                                .keys
                                .iter()
                                .find(|(_, key)| *key == &job.key)
                                .unwrap()
                                .0;
                            if name == "cancelRequested"
                                || self.pump.get_run(&job.key).unwrap().physical.stalled > 0
                            {
                                actual.push(format!("{alias}/{}", job.node));
                            }
                        }
                    }
                    actual.sort();
                    let mut wanted: Vec<String> = serde_json::from_value(want.clone()).unwrap();
                    wanted.sort();
                    assert_eq!(actual, wanted);
                }
                "completionAdopted" => {
                    assert_eq!(*want, json!(false));
                    for (alias, packet) in &self.packets {
                        let run = alias.split_once('/').unwrap().0;
                        let snapshot = self.pump.get_run(&self.keys[run]).unwrap();
                        assert!(
                            !snapshot
                                .result
                                .as_ref()
                                .unwrap()
                                .ports
                                .iter()
                                .any(|port| port.value["packet"] == *packet)
                        );
                    }
                }
                "causeCodes" => {
                    let codes: Vec<Value> = self
                        .keys
                        .values()
                        .flat_map(|key| {
                            self.pump
                                .get_run(key)
                                .unwrap()
                                .result
                                .into_iter()
                                .flat_map(|result| result.causes)
                                .map(|cause| cause["diagnostic"]["error"]["code"].clone())
                        })
                        .collect();
                    assert_eq!(
                        codes
                            .iter()
                            .map(|value| value.as_str().unwrap())
                            .collect::<BTreeSet<_>>(),
                        want.as_array()
                            .unwrap()
                            .iter()
                            .map(|value| value.as_str().unwrap())
                            .collect::<BTreeSet<_>>()
                    );
                }
                "timerFrames" => assert_eq!(
                    json!(
                        self.calls
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|(reason, _)| *reason == WakeReason::Timer)
                            .count()
                    ),
                    *want
                ),
                "observerWakeTimesMs" => assert_eq!(
                    json!(
                        self.calls
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|(reason, _)| *reason == WakeReason::Timer)
                            .map(|(_, at)| *at)
                            .collect::<Vec<_>>()
                    ),
                    *want
                ),
                "additionalObserverCalls" => assert_eq!(
                    json!(self.calls.lock().unwrap().len() - self.initial_calls),
                    *want
                ),
                "planRevisionUnchanged" => assert_eq!(
                    json!(
                        self.pump.session_revision(&self.epoch).unwrap() == self.initial_revision
                            && self.pump.plan_version(&self.epoch).unwrap() == self.initial_version
                            && self.pump.session_has_pending(&self.epoch).unwrap()
                                == self.initial_prepared
                    ),
                    *want
                ),
                "differentRunKeys" => assert_eq!(
                    json!(
                        self.keys
                            .values()
                            .map(|key| serde_json::to_string(key).unwrap())
                            .collect::<BTreeSet<_>>()
                            .len()
                            == self.keys.len()
                    ),
                    *want
                ),
                "errors" => assert_eq!(
                    self.errors
                        .clone()
                        .into_iter()
                        .chain(self.keys.values().filter_map(|key| {
                            self.pump
                                .get_run(key)
                                .ok()?
                                .result?
                                .error
                                .map(|error| error.code)
                        }))
                        .collect::<BTreeSet<_>>(),
                    want.as_array()
                        .unwrap()
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect::<BTreeSet<_>>()
                ),
                other => panic!("unsupported expected field {other}"),
            }
        }
    }
}
impl Drop for Runner {
    fn drop(&mut self) {
        for key in self.keys.values() {
            let _ = self.pump.cancel(key);
        }
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.jobs.extend(self.entered.try_iter());
            for job in &self.jobs {
                job.release();
            }
            let _ = self.pump.advance_to(self.pump.now());
            if self.host.usage().physical_jobs == 0 {
                break;
            }
            assert!(Instant::now() < end, "fixture cleanup");
            std::thread::yield_now();
        }
    }
}

fn run_case(name: &str) {
    let fixture = fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == name)
        .unwrap();
    eprintln!("direct fixture: {name}");
    if case["events"][0]["op"] == "configure" {
        let mut profile = fixture["resourceProfiles"]
            [case["initial"]["resourceProfile"].as_str().unwrap()]
        .clone();
        assert_eq!(case["events"].as_array().unwrap().len(), 1);
        for (field, value) in case["events"][0]["overrides"].as_object().unwrap() {
            profile[field] = value.clone();
        }
        let result = ResourcePolicy::from_value(profile)
            .and_then(|policy| ExecutionHost::new(format!("direct-{name}"), policy))
            .and_then(diy_ime2_core::execution_control::ExecutionService::new);
        for (field, value) in case["expected"].as_object().unwrap() {
            match field.as_str() {
                "controllerCreated" => assert_eq!(json!(result.is_ok()), *value),
                "errors" => assert_eq!(json!([result.as_ref().unwrap_err().code]), *value),
                "acceptedRuns" => {
                    assert!(result.is_err());
                    assert_eq!(*value, json!(0));
                }
                other => panic!("unsupported constructor expectation {other}"),
            }
        }
        return;
    }
    let mut runner = Runner::new(case, &fixture);
    for event in case["events"].as_array().unwrap() {
        runner.event(event, case, &fixture);
    }
    runner.expected(&case["expected"]);
}

#[test]
fn the_direct_case_list_is_unique_and_bound_to_the_adopted_fixture() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 54);
    assert_eq!(DIRECT.len(), 43);
    assert_eq!(DIRECT.iter().collect::<BTreeSet<_>>().len(), DIRECT.len());
    for id in DIRECT {
        assert_eq!(cases.iter().filter(|case| case["id"] == *id).count(), 1);
    }
}

#[test]
fn budget_value_decoding_keeps_defaults_and_checks_declared_job_ids() {
    assert_eq!(
        RunBudgets::from_value(json!({})).unwrap(),
        RunBudgets::default()
    );
    let parsed = RunBudgets::from_value(
        json!({"runBudgetMs":1,"jobBudgets":[{"nodeId":"main","budgetMs":null}]}),
    )
    .unwrap();
    assert_eq!(parsed.run_budget_ms.unwrap().get(), 1);
    assert_eq!(parsed.job_budgets[0].budget_ms, None);
    for value in [
        json!({"unknown":1}),
        json!({"jobBudgets":[{"nodeId":"main","budgetMs":1},{"nodeId":"main","budgetMs":2}]}),
    ] {
        assert_eq!(
            RunBudgets::from_value(value).unwrap_err().code,
            "invalid-deadline"
        );
    }
}

#[test]
fn coverage_names_every_formal_case_and_existing_evidence_without_promoting_related_tests() {
    let fixture = fixture();
    let coverage: Value = serde_json::from_str(include_str!("execution_coverage.json")).unwrap();
    let formal: BTreeSet<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    let entries = coverage["cases"].as_array().unwrap();
    let actual: BTreeSet<_> = entries
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(entries.len(), actual.len());
    assert_eq!(actual, formal);
    let direct: BTreeSet<_> = entries
        .iter()
        .filter(|entry| entry["status"] == "direct-core")
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(direct, DIRECT.iter().copied().collect());
    let private: Vec<String> =
        serde_json::from_str(include_str!("execution_private_cases.json")).unwrap();
    let prestart: Vec<String> =
        serde_json::from_str(include_str!("execution_prestart_cases.json")).unwrap();
    assert_eq!(prestart.len(), 1);
    let clock: Vec<String> =
        serde_json::from_str(include_str!("execution_clock_cases.json")).unwrap();
    assert_eq!(clock.len(), 1);
    let private_ids: BTreeSet<_> = private
        .iter()
        .chain(&prestart)
        .chain(&clock)
        .map(String::as_str)
        .collect();
    assert_eq!(private.len(), 6);
    assert_eq!(
        private.len() + prestart.len() + clock.len(),
        private_ids.len()
    );
    assert!(private_ids.is_disjoint(&direct));
    assert_eq!(
        private_ids,
        entries
            .iter()
            .filter(|entry| entry["status"] == "direct-private-core")
            .map(|entry| entry["id"].as_str().unwrap())
            .collect()
    );
    for entry in entries {
        assert!(matches!(
            entry["status"].as_str(),
            Some("direct-core" | "direct-private-core" | "related-only")
        ));
        assert!(!entry["remaining"].as_str().unwrap().trim().is_empty());
        let evidence = entry["evidence"].as_array().unwrap();
        assert!(!evidence.is_empty());
        for reference in evidence {
            let (file, name) = reference.as_str().unwrap().split_once('#').unwrap();
            let source = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file),
            )
            .unwrap();
            assert!(
                source.contains(&format!("fn {name}(")) || source.contains(&format!("{name} =>")),
                "missing evidence {reference}"
            );
        }
    }
}
