//! Core-only four-stage synthetic transport benchmark, not Reference IME latency.
#[allow(dead_code)]
#[path = "../tests/support/demo.rs"]
mod test_nodes;
use std::{
    env,
    error::Error,
    hint::black_box,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use test_nodes::register_builtin_nodes;

use diy_ime2_core::{
    Candidate,
    graph::{Edge, Endpoint, ExecutionPlan, Graph, NodeInstance},
    node::{InputMode, Registry, RuntimeInput},
    runtime::{Engine, NodeStatus, RunReport},
};
use serde::Serialize;
use serde_json::json;

const LENGTHS: [usize; 7] = [1, 8, 32, 128, 512, 2048, 4096];
const NODES: [(&str, &str); 4] = [
    ("input", "input.text"),
    ("first", "demo.identity"),
    ("second", "demo.identity"),
    ("result", "demo.identity"),
];

struct Options {
    iterations: usize,
    warmup: usize,
    build_label: String,
    environment: String,
}

fn options() -> Result<Option<Options>, Box<dyn Error>> {
    let mut options = Options {
        iterations: 100,
        warmup: 10,
        build_label: "unspecified (see debugAssertions)".into(),
        environment: "not supplied".into(),
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--help" {
            println!(
                "runtime_benchmark [--iterations 1..10000] [--warmup 0..1000] \
                 [--build-label TEXT] [--environment TEXT]\n\
                 Defaults: 100 measurements, 10 warmup runs per phase/case.\n\
                 Prints JSON only after every measured result passes validation."
            );
            return Ok(None);
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--iterations" => options.iterations = value.parse()?,
            "--warmup" => options.warmup = value.parse()?,
            "--build-label" => options.build_label = value,
            "--environment" => options.environment = value,
            _ => return Err(format!("unknown argument: {flag}").into()),
        }
    }
    if !(1..=10_000).contains(&options.iterations) || options.warmup > 1000 {
        return Err("iterations must be 1..10000 and warmup must be 0..1000".into());
    }
    Ok(Some(options))
}

fn graph() -> Graph {
    Graph {
        version: 1,
        nodes: NODES
            .iter()
            .map(|(id, kind)| NodeInstance {
                id: (*id).into(),
                kind: (*kind).into(),
                version: 1,
                config: None,
            })
            .collect(),
        edges: NODES
            .windows(2)
            .map(|pair| Edge {
                from: Endpoint::new(pair[0].0, "out"),
                to: Endpoint::new(pair[1].0, "in"),
            })
            .collect(),
    }
}

/// Synthetic transport returns the exact input; processing latency is measured with the real application.
fn input_case(family: &str, length: usize) -> (String, String) {
    let pattern = if family == "ascii" {
        "ka"
    } else {
        "ka𠮷🦀"
    };
    let input: String = pattern.chars().cycle().take(length).collect();
    (input.clone(), input)
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Statistics {
    samples: usize,
    median_micros: f64,
    p95_micros: f64,
    min_micros: f64,
    max_micros: f64,
}

fn statistics(mut samples: Vec<Duration>) -> Statistics {
    samples.sort_unstable();
    let count = samples.len();
    assert!(count > 0);
    let micros = |index: usize| samples[index].as_secs_f64() * 1_000_000.0;
    let median = if count.is_multiple_of(2) {
        (micros(count / 2 - 1) + micros(count / 2)) / 2.0
    } else {
        micros(count / 2)
    };
    Statistics {
        samples: count,
        median_micros: median,
        p95_micros: micros((count * 95).div_ceil(100) - 1),
        min_micros: micros(0),
        max_micros: micros(count - 1),
    }
}

fn compile_measurements(
    graph: &Graph,
    registry: &Registry,
    options: &Options,
) -> Result<Statistics, Box<dyn Error>> {
    let mut samples = Vec::with_capacity(options.iterations);
    for index in 0..options.warmup + options.iterations {
        let start = Instant::now();
        let plan = ExecutionPlan::compile(black_box(graph), black_box(registry))?;
        let elapsed = start.elapsed();
        // 順序検証・計画の破棄は測定の外。グラフに毎回の入力は含まない。
        if plan.node_order().collect::<Vec<_>>() != NODES.map(|(id, _)| id) {
            return Err("unexpected execution order".into());
        }
        black_box(&plan);
        if index >= options.warmup {
            samples.push(elapsed);
        }
    }
    Ok(statistics(samples))
}

fn verify(report: &RunReport, input: &RuntimeInput, expected: &str) -> Result<(), Box<dyn Error>> {
    if !report.succeeded()
        || report.plan_version != 1
        || report.input_revision != Some(input.revision)
        || report.nodes.len() != NODES.len()
        || report
            .nodes
            .values()
            .any(|node| node.status != NodeStatus::Executed)
    {
        return Err("run failed or returned unexpected identifiers/statuses".into());
    }
    for (node, port, expected_text) in [
        ("input", "out", input.text.as_str()),
        ("first", "out", expected),
        ("second", "out", expected),
        ("result", "out", expected),
    ] {
        match report.packet(node, port).map(|packet| &packet.root) {
            Some(Candidate::Text { text, meta: None }) if text == expected_text => {}
            _ => return Err(format!("incorrect Text/meta at {node}.{port}").into()),
        }
    }
    Ok(())
}

fn run_measurements(
    engine: &mut Engine,
    text: String,
    expected: &str,
    options: &Options,
    revision: &mut u64,
) -> Result<Statistics, Box<dyn Error>> {
    let mut input = RuntimeInput {
        revision: *revision,
        text,
        mode: InputMode::Finalize,
    };
    let mut samples = Vec::with_capacity(options.iterations);
    for index in 0..options.warmup + options.iterations {
        input.revision = *revision;
        *revision += 1;
        let start = Instant::now();
        let report = engine.run_with_input(black_box(&input))?;
        let elapsed = start.elapsed();
        // 正しさの検証・RunReportの破棄は計測の外。失敗した値は採用しない。
        verify(&report, &input, expected)?;
        black_box(&report);
        if index >= options.warmup {
            samples.push(elapsed);
        }
    }
    Ok(statistics(samples))
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() -> Result<(), Box<dyn Error>> {
    let Some(options) = options()? else {
        return Ok(());
    };
    let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    // メタデータ取得やRegistry生成、表示I/Oもすべて計測の外に置く。
    let git_revision = command_output("git", &["rev-parse", "HEAD"]);
    let git_dirty =
        command_output("git", &["status", "--porcelain"]).map(|state| !state.is_empty());
    let compiler = command_output("rustc", &["-vV"]);
    let mut registry = Registry::new();
    register_builtin_nodes(&mut registry)?;
    let graph = graph();
    let compilation = compile_measurements(&graph, &registry, &options)?;
    let mut engine = Engine::new(ExecutionPlan::compile(&graph, &registry)?);
    let mut revision = 1;
    let mut cases = Vec::new();
    for family in ["ascii", "unicode"] {
        for length in LENGTHS {
            let (input, expected) = input_case(family, length);
            assert_eq!(input.chars().count(), length);
            let bytes = input.len();
            let timing = run_measurements(&mut engine, input, &expected, &options, &mut revision)?;
            cases.push(json!({
                "family": family, "inputCodePoints": length, "inputUtf8Bytes": bytes,
                "outputCodePoints": expected.chars().count(), "run": timing,
            }));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "benchmarkVersion": 2, "purpose": "synthetic-core-transport-only", "startedAtUnixSeconds": started,
            "environment": {
                "os": env::consts::OS, "architecture": env::consts::ARCH,
                "logicalProcessors": std::thread::available_parallelism().ok().map(|count| count.get()),
                "processorIdentifier": env::var("PROCESSOR_IDENTIFIER").ok(),
                "description": options.environment, "compiler": compiler,
                "buildLabel": options.build_label, "debugAssertions": cfg!(debug_assertions),
                "gitRevision": git_revision, "gitDirty": git_dirty,
            },
            "nodeKinds": NODES.map(|(_, kind)| kind), "nodeCount": NODES.len(),
            "inputMode": "finalize", "planVersion": engine.plan_version(),
            "iterations": options.iterations, "warmupPerPhaseOrCase": options.warmup,
            "lengthMetric": "Unicode scalar values (Rust chars)", "timingUnit": "microseconds",
            "validation": "all warmup and measured runs passed; all four output ports checked",
            "runCountIncludingWarmup": revision - 1,
            "planCompilation": compilation, "cases": cases,
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_p95_use_the_documented_definitions() {
        let stats = statistics((1..=20).rev().map(Duration::from_micros).collect());
        assert_eq!(stats.samples, 20);
        assert!((stats.median_micros - 10.5).abs() < 0.000_001);
        assert!((stats.p95_micros - 19.0).abs() < 0.000_001);
        let single = statistics(vec![Duration::from_micros(42)]);
        assert!((single.median_micros - 42.0).abs() < 0.000_001);
        assert!((single.p95_micros - 42.0).abs() < 0.000_001);
    }

    #[test]
    fn every_generated_case_has_the_exact_length_and_correct_four_node_result() {
        let options = Options {
            iterations: 1,
            warmup: 0,
            build_label: "test".into(),
            environment: "test".into(),
        };
        let mut registry = Registry::new();
        register_builtin_nodes(&mut registry).unwrap();
        let mut engine = Engine::new(ExecutionPlan::compile(&graph(), &registry).unwrap());
        let mut revision = 1;
        for family in ["ascii", "unicode"] {
            for length in LENGTHS {
                let (input, expected) = input_case(family, length);
                assert_eq!(input.chars().count(), length);
                run_measurements(&mut engine, input, &expected, &options, &mut revision).unwrap();
            }
        }
        assert_eq!(revision, 15);
    }
}
