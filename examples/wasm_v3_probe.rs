//! Real v3 Service + real AppContainer Wasm integration. Fake input only.
#[cfg(all(windows, target_arch = "x86_64"))]
mod probe {
    use diy_ime2_core::{
        Candidate, Packet,
        execution_control::{
            CancellationToken, DecimalId, DeferredNode, ExecutionController, ExecutionHost,
            ExecutionService, ResourcePolicy, RunKey, RunPhase, StartRequest,
        },
        graph::{ExecutionPlan, Graph},
        isolated_worker::adapter::{ExecutionGate, WasmArtifact, WasmNode, WorkerPolicy},
        node::{CoreError, Node, NodeDefinition, OutputValue, Port, Registry, RunContext},
    };
    use serde_json::{Value, json};
    use std::{
        path::Path,
        sync::Arc,
        time::{Duration, Instant},
    };
    struct Fixed {
        no_output: bool,
    }
    impl Node for Fixed {
        fn definition(&self) -> NodeDefinition {
            NodeDefinition::new(
                "fixture.fixed",
                1,
                vec![],
                vec![
                    Port::packet("out")
                        .with_language("ja-Latn")
                        .with_root(diy_ime2_core::node::RootConstraint::Text),
                ],
            )
        }
        fn execute(
            &self,
            _: RunContext<'_>,
            _: &diy_ime2_core::node::Inputs,
            _: &Value,
        ) -> Result<diy_ime2_core::node::Outputs, CoreError> {
            if self.no_output {
                return Ok([("out".into(), OutputValue::NoOutput)].into());
            }
            Ok([(
                "out".into(),
                OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                    text: "kya".into(),
                    meta: None,
                }))),
            )]
            .into())
        }
    }
    fn id(n: u64) -> DecimalId {
        DecimalId::new(n).unwrap()
    }
    fn wait(mut done: impl FnMut() -> bool) -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs(10);
        while !done() {
            if Instant::now() > end {
                return Err("v3 Worker did not settle".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
    struct Harness {
        host: ExecutionHost,
        controller: Option<ExecutionController>,
    }
    impl Harness {
        fn new(node: &WasmNode, config: Value) -> Result<Self, String> {
            Self::with_input(node, config, false)
        }
        fn with_input(node: &WasmNode, config: Value, no_output: bool) -> Result<Self, String> {
            let mut registry = Registry::new();
            registry
                .register(Fixed { no_output })
                .map_err(|e| e.to_string())?;
            registry
                .register_deferred(node.clone())
                .map_err(|e| e.to_string())?;
            let graph = graph(&node.definition().kind, config);
            let plan = ExecutionPlan::compile(&graph, &registry).map_err(|e| e.to_string())?;
            let mut policy = ResourcePolicy::from_value(
                serde_json::from_str::<Value>(include_str!(
                    "../tests/contracts/execution_control.json"
                ))
                .unwrap()["resourceProfiles"]["bounded"]
                    .clone(),
            )
            .unwrap();
            policy.max_physical_jobs = 1;
            policy.max_queued_jobs = 0;
            let host = ExecutionHost::new(
                format!("wasm-v3-fixture-{}", {
                    static NEXT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(1);
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                }),
                policy,
            )
            .map_err(|e| e.to_string())?;
            let service = ExecutionService::new(host.clone()).map_err(|e| e.to_string())?;
            let controller = service.open_ready(plan, id(1)).map_err(|e| e.to_string())?;
            Ok(Self {
                host,
                controller: Some(controller),
            })
        }
        fn c(&self) -> &ExecutionController {
            self.controller.as_ref().unwrap()
        }
        fn start(&self, key: u64) -> Result<RunKey, String> {
            let request:StartRequest=serde_json::from_value(json!({"startKey":key.to_string(),"expectedPlanVersion":"1","targets":[{"node":"worker","port":"out"}],"input":{"revision":key,"text":"private-full-input-canary","mode":"finalize"}})).unwrap();
            self.c()
                .start_run(request)
                .map(|r| r.key)
                .map_err(|e| e.to_string())
        }
        fn terminal(&self, key: &RunKey) -> Result<Value, String> {
            wait(|| {
                self.c()
                    .get_run(key)
                    .is_ok_and(|s| s.state == RunPhase::Terminal)
            })?;
            serde_json::to_value(self.c().get_run(key).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        fn drained(&self) -> Result<(), String> {
            wait(|| self.host.usage().physical_jobs == 0)
        }
    }
    impl Drop for Harness {
        fn drop(&mut self) {
            self.controller.take();
            let _ =
                wait(|| self.host.usage().physical_jobs == 0 && self.host.usage().active_runs == 0);
        }
    }
    fn graph(kind: &str, config: Value) -> Graph {
        Graph::from_json(&json!({"version":1,"nodes":[{"id":"source","kind":"fixture.fixed","version":1},{"id":"worker","kind":kind,"version":1,"config":config}],"edges":[{"from":{"node":"source","port":"out"},"to":{"node":"worker","port":"in"}}]}).to_string()).unwrap()
    }
    fn approve(node: &WasmNode, gate: &ExecutionGate, h: &Harness) {
        gate.approve(node.grant_target(h.c().session_epoch(), id(1), "worker"));
    }
    pub fn run() -> Result<Value, String> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.len() != 3 {
            return Err("usage: wasm_v3_probe worker.exe runtime.dll fixture.wasm".into());
        }
        let mut policy = WorkerPolicy::bounded_probe();
        policy.fuel_per_call = None; // Infinite guest fixture must be ended by the actual OS owner.
        policy.termination_grace = Duration::ZERO;
        let artifact = WasmArtifact::capture(
            &std::env::temp_dir(),
            Path::new(&args[0]),
            Path::new(&args[1]),
            std::fs::read(&args[2]).map_err(|e| e.to_string())?,
            policy,
        )
        .map_err(|e| e.to_string())?;
        let gate = ExecutionGate::default();
        let configs = [
            "echo",
            "hold",
            "malformed",
            "forge",
            "noOutput",
            "duplicatePort",
            "duplicateEnvelope",
            "duplicateMeta",
        ]
        .map(|mode| json!({"mode":mode}))
        .to_vec();
        for (kind, configs) in [
            ("fixture.wasm.duplicate", vec![]),
            ("fixture.wasm", vec![json!({"mode":"duplicateValidate"})]),
        ] {
            match artifact.prepare(kind, 1, configs, gate.clone(), CancellationToken::default()) {
                Err(error) if error.code == "worker-reply" => (),
                _ => return Err("duplicate describe/config reply accepted".into()),
            }
        }
        let node = artifact
            .prepare(
                "fixture.wasm",
                1,
                configs,
                gate.clone(),
                CancellationToken::default(),
            )
            .map_err(|e| e.to_string())?;
        let bounded = artifact
            .with_module(
                std::fs::read(&args[2]).map_err(|e| e.to_string())?,
                WorkerPolicy::bounded_probe(),
            )
            .map_err(|e| e.to_string())?;
        let cpu_gate = ExecutionGate::default();
        let cpu = bounded
            .prepare(
                "fixture.wasm",
                1,
                vec![json!({"mode":"cpu"})],
                cpu_gate.clone(),
                CancellationToken::default(),
            )
            .map_err(|e| e.to_string())?;
        let cpu_phases;
        {
            let h = Harness::new(&cpu, json!({"mode":"cpu"}))?;
            approve(&cpu, &cpu_gate, &h);
            h.c().start_detailed_trace().map_err(|e| e.to_string())?;
            let key = h.start(1)?;
            let snapshot = h.terminal(&key)?;
            // sum(i*i, i=0..99_999) = n(n-1)(2n-1)/6, independently of the loop.
            let expected = json!({"status":"data","packet":{"version":1,"root":{"type":"text","text":"333328333350000"}}});
            if snapshot["current"] != true
                || snapshot["result"]["reason"] != "completed"
                || snapshot["result"]["delivery"] != "complete"
                || snapshot["result"]["causes"] != json!([])
                || snapshot["result"]["ports"].as_array().is_none_or(|ports| {
                    !ports
                        .iter()
                        .any(|p| p["endpoint"]["node"] == "worker" && p["value"] == expected)
                })
            {
                return Err("bounded CPU fixture changed its exact output".into());
            }
            h.drained()?;
            h.c().stop_trace();
            let trace = h.c().read_trace(0, 256).map_err(|e| e.to_string())?;
            use diy_ime2_core::execution_control::WorkerTracePhase;
            let phase = |phase| {
                trace
                    .events
                    .iter()
                    .filter_map(|event| event.worker.as_ref())
                    .find(|interval| interval.phase == phase)
            };
            let compile = phase(WorkerTracePhase::Compile).ok_or("missing compilation trace")?;
            let input = phase(WorkerTracePhase::InputRead).ok_or("missing input read trace")?;
            let instantiate =
                phase(WorkerTracePhase::Instantiate).ok_or("missing instantiation trace")?;
            let micros = |value: &str| value.parse::<u64>().map_err(|error| error.to_string());
            if compile.instance != input.instance
                || input.instance != instantiate.instance
                || micros(
                    compile
                        .finished_us
                        .as_deref()
                        .ok_or("incomplete compilation")?,
                )? > micros(&input.started_us)?
                || micros(
                    input
                        .finished_us
                        .as_deref()
                        .ok_or("incomplete input read")?,
                )? > micros(&instantiate.started_us)?
            {
                return Err(
                    "JIT preparation, input read, and instantiation occurred out of order".into(),
                );
            }
            cpu_phases = json!({"dropped":trace.dropped,"worker":trace.events.iter().filter_map(|e| e.worker.as_ref()).filter(|w| matches!(w.phase, diy_ime2_core::execution_control::WorkerTracePhase::Compile | diy_ime2_core::execution_control::WorkerTracePhase::GuestCall)).collect::<Vec<_>>()});
        }
        if bounded.active_processes() != 0 {
            return Err("bounded CPU fixture retained a physical Worker".into());
        }
        for fuel_limit in [true, false] {
            let mut limited = WorkerPolicy::bounded_probe();
            if fuel_limit {
                limited.fuel_per_call = Some(1);
            } else {
                limited.wasm_memory_bytes = Some(1);
            }
            let limited = artifact
                .with_module(std::fs::read(&args[2]).map_err(|e| e.to_string())?, limited)
                .map_err(|e| e.to_string())?;
            let result = limited.prepare(
                "fixture.wasm",
                1,
                vec![json!({"mode":"cpu"})],
                ExecutionGate::default(),
                CancellationToken::default(),
            );
            if !matches!(result, Err(ref e) if e.code == "worker-failed")
                || limited.active_processes() != 0
            {
                return Err("tiny Wasm fuel/memory policy was bypassed or not reclaimed".into());
            }
        }
        let calls = artifact.request_count();
        let mut registry = Registry::new();
        registry.register(Fixed { no_output: false }).unwrap();
        registry.register_deferred(node.clone()).unwrap();
        ExecutionPlan::compile(&graph("fixture.wasm", json!({"mode":"echo"})), &registry).unwrap();
        if artifact.request_count() != calls {
            return Err("Core prepare executed guest code".into());
        }
        let rejected = ExecutionPlan::compile(
            &graph("fixture.wasm", json!({"mode":"unprepared"})),
            &registry,
        );
        if rejected.as_ref().err().is_none_or(|e| {
            e.code != "invalid-config" || !e.message.contains("wasm-config-not-prepared")
        }) {
            return Err("unprepared config was accepted".into());
        }
        let invalid = artifact.prepare(
            "fixture.wasm",
            1,
            vec![json!({"mode":"invalid"})],
            gate.clone(),
            CancellationToken::default(),
        );
        if invalid
            .as_ref()
            .err()
            .is_none_or(|e| e.code != "invalid-config")
        {
            return Err("isolated config failure was hidden".into());
        }
        {
            let h = Harness::new(&node, json!({"mode":"echo"}))?;
            let before = artifact.request_count();
            let key = h.start(1)?;
            let snapshot = h.terminal(&key)?;
            if snapshot["result"]["delivery"] != "failed" || artifact.request_count() != before {
                return Err("default-deny execution started a Worker".into());
            }
        }
        {
            let h = Harness::new(&node, json!({"mode":"echo"}))?;
            approve(&node, &gate, &h);
            let key = h.start(1)?;
            let snapshot = h.terminal(&key)?;
            let text = snapshot["result"]["ports"]
                .as_array()
                .and_then(|v| v.iter().find(|p| p["endpoint"]["node"] == "worker"))
                .and_then(|v| v["value"]["packet"]["root"]["text"].as_str())
                .ok_or("guest projection missing")?;
            if text.contains("private-full-input-canary")
                || text.contains("sessionEpoch")
                || text.contains("deliveryGeneration")
                || text.contains("runId")
            {
                return Err("Host-only context leaked to guest".into());
            }
            let request: Value =
                serde_json::from_str(text).map_err(|_| "invalid reflected guest request")?;
            if request["operation"]["inputs"]["in"]["packet"]["root"]["text"] != "kya"
                || request["operation"]["mode"] != "finalize"
                || request.as_object().is_none_or(|m| m.len() != 3)
                || request["operation"]
                    .as_object()
                    .is_none_or(|m| m.len() != 4)
            {
                return Err("connected-input/config/mode projection mismatch".into());
            }
            h.drained()?;
        }
        for mode in [
            "malformed",
            "forge",
            "duplicatePort",
            "duplicateEnvelope",
            "duplicateMeta",
        ] {
            let h = Harness::new(&node, json!({"mode":mode}))?;
            approve(&node, &gate, &h);
            let key = h.start(1)?;
            let snapshot = h.terminal(&key)?;
            if snapshot["result"]["delivery"] != "failed" {
                return Err("forged or malformed reply published".into());
            }
            h.drained()?;
        }
        {
            let h = Harness::new(&node, json!({"mode":"noOutput"}))?;
            approve(&node, &gate, &h);
            let key = h.start(1)?;
            let snapshot = h.terminal(&key)?;
            if snapshot["result"]["delivery"] != "no-output" {
                return Err("NoOutput contract changed".into());
            }
            h.drained()?;
        }
        for action in ["cancel", "revoke", "protect", "close"] {
            let mut h = Harness::new(&node, json!({"mode":"hold"}))?;
            approve(&node, &gate, &h);
            let before = artifact.request_count();
            let key = h.start(1)?;
            wait(|| artifact.request_count() > before && artifact.active_processes() > 0)?;
            std::thread::sleep(Duration::from_millis(100));
            if h.host.usage().physical_jobs != 1 {
                return Err("physical slot released before guest ending".into());
            }
            // A second request has no physical capacity while the first producer is held.
            let blocked = h.start(2);
            if blocked.is_ok() {
                return Err("capacity was reused while a producer was alive".into());
            }
            match action {
                "cancel" => {
                    h.c().cancel_run(&key).map_err(|e| e.to_string())?;
                }
                "revoke" => gate.revoke(&node.grant_target(h.c().session_epoch(), id(1), "worker")),
                "protect" => gate.set_protection(true, false),
                "close" => {
                    h.controller.take();
                }
                _ => unreachable!(),
            }
            h.drained()?;
            if action != "close" {
                let snapshot = h.terminal(&key)?;
                if action == "cancel" {
                    if snapshot["result"]["delivery"] != "cancelled" {
                        return Err("cancel was not terminal cancellation".into());
                    }
                } else if snapshot["result"]["delivery"] != "failed" {
                    return Err("revoked/protected result was published".into());
                }
            }
            gate.set_protection(false, false);
        }
        {
            let h = Harness::with_input(&node, json!({"mode":"echo"}), true)?;
            approve(&node, &gate, &h);
            let before = artifact.request_count();
            let key = h.start(1)?;
            let snapshot = h.terminal(&key)?;
            if snapshot["result"]["delivery"] != "no-output" || artifact.request_count() != before {
                return Err("required NoOutput invoked the guest".into());
            }
            h.drained()?;
        }
        for action in ["revoke", "protect"] {
            let mut h = Harness::new(&node, json!({"mode":"echo"}))?;
            let target = node.grant_target(h.c().session_epoch(), id(1), "worker");
            gate.approve(target.clone());
            let guarded = gate
                .seal(h.controller.take().unwrap(), vec![target.clone()])
                .map_err(|e| e.to_string())?;
            let request:StartRequest=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"worker","port":"out"}]})).unwrap();
            let key = guarded.start_run(request).map_err(|e| e.to_string())?.key;
            wait(|| {
                guarded
                    .get_run(&key)
                    .is_ok_and(|r| r.state == RunPhase::Terminal)
            })?;
            if guarded.get_run(&key).unwrap().result.unwrap().delivery
                != diy_ime2_core::execution_control::RunDelivery::Complete
            {
                return Err("guarded result did not complete".into());
            }
            match action {
                "revoke" => gate.revoke(&target),
                "protect" => gate.set_protection(true, false),
                _ => unreachable!(),
            }
            if guarded
                .get_run(&key)
                .as_ref()
                .err()
                .is_none_or(|e| e.code != "worker-authority")
            {
                return Err("retained result escaped revoked controller".into());
            }
            gate.set_protection(false, false);
            gate.approve(target);
            if guarded.get_run(&key).is_ok() {
                return Err("old Session resurrected after protection/reapproval".into());
            }
            drop(guarded);
            h.drained()?;
        }
        {
            let mut h = Harness::new(&node, json!({"mode":"hold"}))?;
            let target = node.grant_target(h.c().session_epoch(), id(1), "worker");
            gate.approve(target.clone());
            let guarded = gate
                .seal(h.controller.take().unwrap(), vec![target])
                .map_err(|e| e.to_string())?;
            let request:StartRequest=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"worker","port":"out"}]})).unwrap();
            let key = guarded.start_run(request).map_err(|e| e.to_string())?.key;
            wait(|| artifact.active_processes() > 0)?;
            gate.set_protection(false, true);
            if guarded.get_run(&key).is_ok() {
                return Err("automatic protection left the controller open".into());
            }
            h.drained()?;
            gate.set_protection(false, false);
            if guarded.get_run(&key).is_ok() {
                return Err("protected Session reopened automatically".into());
            }
            drop(guarded);
        }
        Ok(
            json!({"kind":"wasm-v3-integration","realWasm":true,"defaultDeny":true,"noFullInput":true,"pureCorePrepare":true,"unpreparedRejected":true,"invalidConfigRejected":true,"malformedRejected":true,"duplicateRepliesRejected":true,"forgedExitRejected":true,"noOutput":true,"cancellation":true,"revocation":true,"protection":true,"sessionClose":true,"physicalSlot":true,"retainedResultsRevoked":true,"requiredNoOutputSkipped":true,"protectedControllerClosed":true,"boundedCpuOutput":true,"tinyFuelRejected":true,"tinyMemoryRejected":true,"cpuRelativePhases":cpu_phases,"requests":artifact.request_count()}),
        )
    }
}
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    match probe::run() {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("wasm-v3: {error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("isolated Wasm v3 requires x64 Windows");
        std::process::exit(1);
    }
}
