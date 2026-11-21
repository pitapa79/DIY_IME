//! Windows Core ExecutionService with native clock and a synthetic identity chain.
use diy_ime2_runtime::{
    Candidate, Packet,
    error_input::ErrorAwareExecution,
    execution_control::{
        CancellationToken, CompletionPermit, DecimalId, DeferredNode, ExecutionHost,
        ExecutionService, OwnedInvocation, PhysicalCounts, ResourcePolicy, RunDelivery, RunPhase,
        StartRequest, TerminalReason, TraceEvent, TraceKind,
    },
    graph::{Edge, Endpoint, ExecutionPlan, Graph, NodeInstance},
    node::{
        self, CoreError, InputMode, InputPort, Inputs, Node, NodeDefinition, Outputs, Port,
        Registry, RunContext, RuntimeInput,
    },
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[path = "../src/nodes/runtime_input/mod.rs"]
mod runtime_input;

struct Echo;
impl Node for Echo {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "probe.echo",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("out")],
        )
    }

    fn execute(&self, _: RunContext<'_>, inputs: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        Ok([("out".into(), inputs["in"].clone())].into())
    }
}

struct DeferredEcho {
    later: bool,
}
impl DeferredNode for DeferredEcho {
    fn definition(&self) -> NodeDefinition {
        Node::definition(&Echo)
    }

    fn start(
        &self,
        work: OwnedInvocation,
        permit: CompletionPermit,
        _: CancellationToken,
    ) -> Result<(), CoreError> {
        let result = ErrorAwareExecution::new([("out".into(), work.inputs()["in"].clone())].into());
        drop(work);
        if self.later {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2));
                assert!(permit.complete(Ok(result)));
            });
        } else {
            assert!(permit.complete(Ok(result)));
        }
        Ok(())
    }
}

fn summarize(samples: &mut [u64]) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    samples.sort_unstable();
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100) - 1];
    json!({"n":samples.len(),"p50Ns":percentile(50),"p95Ns":percentile(95),
        "p99Ns":percentile(99),"maxNs":percentile(100)})
}

fn dispatch_windows(events: &[TraceEvent], expected_nodes: usize) -> Option<[u64; 3]> {
    let run = events.first()?.run;
    let mut nodes = BTreeMap::<_, [Option<&TraceEvent>; 4]>::new();
    for event in events {
        if event.run != run {
            return None;
        }
        let index = match event.event {
            TraceKind::NodeReady => 0,
            TraceKind::CoreDispatchSpawn => 1,
            TraceKind::CoreDispatchEntered => 2,
            TraceKind::NodeStarted => 3,
            _ => continue,
        };
        let slots = nodes.entry(event.node.as_deref()?).or_default();
        if slots[index].replace(event).is_some() {
            return None;
        }
    }
    if nodes.len() != expected_nodes {
        return None;
    }
    let mut sums = [0; 3];
    for rows in nodes.into_values() {
        let [Some(ready), Some(spawn), Some(entered), Some(started)] = rows else {
            return None;
        };
        if spawn.call.is_none() || spawn.call != entered.call || entered.call != started.call {
            return None;
        }
        let [ready, spawn, entered, started] =
            [ready, spawn, entered, started].map(|row| row.at_us.parse::<u64>().ok());
        let [Some(ready), Some(spawn), Some(entered), Some(started)] =
            [ready, spawn, entered, started]
        else {
            return None;
        };
        for (sum, delta) in sums.iter_mut().zip([
            spawn.checked_sub(ready)?,
            entered.checked_sub(spawn)?,
            started.checked_sub(entered)?,
        ]) {
            *sum += delta * 1000;
        }
    }
    Some(sums)
}

fn completion_windows(
    events: &[TraceEvent],
    expected_nodes: usize,
    expected_deferred: usize,
) -> Option<([u64; 2], usize)> {
    let mut calls = BTreeMap::<_, [Option<&TraceEvent>; 5]>::new();
    for event in events {
        let index = match event.event {
            TraceKind::CoreProducerEndQueued => 0,
            TraceKind::CoreProducerEndReceived => 1,
            TraceKind::CoreStartReturnQueued => 2,
            TraceKind::CoreStartReturnReceived => 3,
            TraceKind::CallEnded => 4,
            _ => continue,
        };
        let slots = calls
            .entry((event.node.as_deref()?, event.call?))
            .or_default();
        if slots[index].replace(event).is_some() {
            return None;
        }
    }
    if calls.len() != expected_nodes {
        return None;
    }
    let mut sums = [0; 2];
    let mut deferred = 0;
    let mut start_return_received_last = 0;
    for rows in calls.into_values() {
        let [
            Some(queued),
            Some(received),
            return_queued,
            return_received,
            Some(ended),
        ] = rows
        else {
            return None;
        };
        let as_time = |row: &TraceEvent| row.at_us.parse::<u64>().ok();
        let as_order = |row: &TraceEvent| row.sequence.parse::<u64>().ok();
        if as_order(queued)? >= as_order(received)? || as_time(queued)? > as_time(received)? {
            return None;
        }
        let mut message = (queued, received);
        match (return_queued, return_received) {
            (Some(queued), Some(start_received)) => {
                if as_order(queued)? >= as_order(start_received)?
                    || as_time(queued)? > as_time(start_received)?
                {
                    return None;
                }
                deferred += 1;
                if as_order(start_received)? > as_order(received)? {
                    start_return_received_last += 1;
                    message = (queued, start_received);
                }
            }
            (None, None) => {}
            _ => return None,
        }
        if as_order(message.1)? >= as_order(ended)? {
            return None;
        }
        sums[0] += as_time(message.1)?.checked_sub(as_time(message.0)?)? * 1000;
        sums[1] += as_time(ended)?.checked_sub(as_time(message.1)?)? * 1000;
    }
    (deferred == expected_deferred).then_some((sums, start_return_received_last))
}

fn run_case(count: usize, text: &str, samples: u64, warmup: u64, deferred: Option<bool>) -> Value {
    let mut registry = Registry::new();
    registry.register(runtime_input::RuntimeTextInput).unwrap();
    if let Some(later) = deferred {
        registry.register_deferred(DeferredEcho { later }).unwrap();
    } else {
        registry.register(Echo).unwrap();
    }
    let names: Vec<_> = std::iter::once("input".to_owned())
        .chain((0..count).map(|i| format!("echo{i}")))
        .collect();
    let graph = Graph {
        version: 1,
        nodes: names
            .iter()
            .enumerate()
            .map(|(i, id)| NodeInstance {
                id: id.clone(),
                kind: if i == 0 { "input.text" } else { "probe.echo" }.into(),
                version: 1,
                config: None,
            })
            .collect(),
        edges: names
            .windows(2)
            .map(|p| Edge {
                from: Endpoint::new(&p[0], "out"),
                to: Endpoint::new(&p[1], "in"),
            })
            .collect(),
    };
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/contracts/execution_control.json")).unwrap();
    let policy =
        ResourcePolicy::from_value(fixture["resourceProfiles"]["bounded"].clone()).unwrap();
    let host = ExecutionHost::new(
        format!("probe-core-async-{count}-{}", text.chars().count()),
        policy,
    )
    .unwrap();
    let service = ExecutionService::new(host.clone()).unwrap();
    let version = DecimalId::new(1).unwrap();
    let plan = ExecutionPlan::compile(&graph, &registry).unwrap();
    let controller = service.open_ready(plan, version).unwrap();
    let target = Endpoint::new(names.last().unwrap(), "out");
    let mut invalidate = Vec::new();
    let mut admit = Vec::new();
    let mut observe = Vec::new();
    let mut total = Vec::new();
    let mut phases: [Vec<u64>; 3] = Default::default();
    let mut completion_phases: [Vec<u64>; 2] = Default::default();
    let mut start_return_received_last = 0;
    let mut incomplete = Vec::new();
    for revision in 1..=samples + warmup {
        let request = StartRequest {
            start_key: controller.next_start_key().unwrap(),
            expected_plan_version: version,
            targets: vec![target.clone()],
            input: Some(RuntimeInput {
                revision,
                text: text.into(),
                mode: InputMode::Compose,
            }),
            run_budget_ms: None,
            job_budgets: vec![],
        };
        let measuring = revision > warmup;
        if measuring {
            controller.start_detailed_trace().unwrap();
        }
        let start = Instant::now();
        if revision > 1 {
            controller.invalidate_input(version, revision).unwrap();
        }
        let invalidated = Instant::now();
        let receipt = controller.start_run(request).unwrap();
        let admitted = Instant::now();
        let mut snapshot = controller
            .wait_run(&receipt.key, Duration::from_millis(500))
            .unwrap();
        while snapshot.physical != PhysicalCounts::default()
            && start.elapsed() < Duration::from_millis(500)
        {
            std::thread::yield_now();
            snapshot = controller.get_run(&receipt.key).unwrap();
        }
        let observed = Instant::now();
        assert!(!receipt.replayed && snapshot.current && snapshot.key == receipt.key);
        assert_eq!(snapshot.state, RunPhase::Terminal);
        assert_eq!(snapshot.physical, PhysicalCounts::default());
        let result = snapshot.result.unwrap();
        assert_eq!(result.reason, TerminalReason::Completed);
        assert_eq!(result.delivery, RunDelivery::Complete);
        let mut returned = result.ports.iter().filter(|port| port.endpoint == target);
        let result = returned.next().expect("requested target missing");
        assert!(returned.next().is_none());
        assert_eq!(result.value["packet"]["root"]["text"], text);
        if measuring {
            controller.stop_trace();
            let mut events = Vec::new();
            let mut after = 0;
            let mut dropped = 0;
            loop {
                let page = controller.read_trace(after, 256).unwrap();
                dropped = dropped.max(page.dropped);
                let Some(last) = page.events.last() else {
                    break;
                };
                after = last.sequence.parse().unwrap();
                events.extend(page.events);
            }
            let dispatch = dispatch_windows(&events, names.len());
            let finish = completion_windows(
                &events,
                names.len(),
                if deferred.is_some() { count } else { 0 },
            );
            match (dispatch, finish) {
                (Some(windows), Some((completion, return_last))) if dropped == 0 => {
                    for (samples, duration) in phases.iter_mut().zip(windows) {
                        samples.push(duration);
                    }
                    for (samples, duration) in completion_phases.iter_mut().zip(completion) {
                        samples.push(duration);
                    }
                    start_return_received_last += return_last;
                }
                _ => incomplete.push(json!({"sample":revision-warmup,"dropped":dropped,
                    "eventCount":events.len()})),
            }
            controller.clear_trace();
            invalidate.push((invalidated - start).as_nanos() as u64);
            admit.push((admitted - invalidated).as_nanos() as u64);
            observe.push((observed - admitted).as_nanos() as u64);
            total.push((observed - start).as_nanos() as u64);
        }
    }
    assert_eq!(host.usage().physical_jobs, 0);
    let [mut before_spawn, mut spawn_to_entry, mut entry_to_start] = phases;
    let [mut queue_to_receive, mut receive_to_ended] = completion_phases;
    let echo = match deferred {
        None => "sync",
        Some(false) => "deferred-inline-completion",
        Some(true) => "deferred-later-completion",
    };
    json!({"echoStages":count,"codePoints":text.chars().count(),"warmup":warmup,
         "echo":echo,"deferredCallsWithStartReturnReceivedLast":start_return_received_last,
         "planned":samples,"incompleteTraceSamples":incomplete,
         "readyToSpawn":summarize(&mut before_spawn),
         "spawnToThreadEntry":summarize(&mut spawn_to_entry),
         "entryToCallbackStart":summarize(&mut entry_to_start),
         "completionQueuedToReceived":summarize(&mut queue_to_receive),
         "completionReceivedToCallEnded":summarize(&mut receive_to_ended),
         "invalidate":summarize(&mut invalidate),"startToReceipt":summarize(&mut admit),
         "receiptToTerminalObservation":summarize(&mut observe),
         "invalidateToTerminalObservation":summarize(&mut total)})
}

fn main() {
    let ascii = "kakikukeko";
    let unicode = "ka𠮷漢🙂".repeat(8);
    let mut cases: Vec<_> = [1, 4, 20]
        .into_iter()
        .flat_map(|count| {
            [ascii, unicode.as_str()]
                .into_iter()
                .map(move |text| run_case(count, text, 500, 30, None))
        })
        .collect();
    cases.push(run_case(20, ascii, 150, 30, Some(false)));
    cases.push(run_case(1, ascii, 150, 30, Some(true)));
    println!(
        "{}",
        json!({"boundary":"native-clock Core controller, synthetic in-process echo",
                       "trace":"detailed","cases":cases})
    );
}
