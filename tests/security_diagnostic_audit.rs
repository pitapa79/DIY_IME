//! Canary exposure audit. These tests document current diagnostic behavior;
//! successful assertions do not imply confidentiality or secret masking.
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
};
fn cli(command: &str, graph: Value) -> Output {
    cli_with(env!("CARGO_BIN_EXE_diy-ime2"), command, graph)
}
fn cli_with(executable: &str, command: &str, graph: Value) -> Output {
    let mut child = Command::new(executable)
        .args([command, "-"])
        .env_remove("DIY_IME_RESOURCE_ROOT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(graph.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
fn graph(text: &str, output: bool) -> Value {
    let mut nodes = vec![
        json!({"id":"input","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":text}}}}),
    ];
    let mut edges = Vec::<Value>::new();
    if output {
        nodes.push(json!({"id":"log","kind":"e2e.output","version":1}));
        edges.push(json!({"from":{"node":"input","port":"out"},"to":{"node":"log","port":"in"}}));
    }
    json!({"version":1,"nodes":nodes,"edges":edges})
}
#[test]
fn raw_cli_compile_errors_can_echo_graph_identifiers_but_not_a_later_invocation() {
    let canary = "canary-graph-identifier";
    let bad = json!({"version":1,"nodes":[{"id":"source","kind":canary,"version":1}],"edges":[]});
    let rejected = cli("validate-graph", bad);
    assert!(!rejected.status.success());
    assert!(String::from_utf8(rejected.stderr).unwrap().contains(canary));
    let next = cli("validate-graph", graph("通常の偽入力", false));
    assert!(next.status.success());
    assert!(!String::from_utf8(next.stdout).unwrap().contains(canary));
    assert!(!String::from_utf8(next.stderr).unwrap().contains(canary));
}
#[test]
fn explicitly_selected_legacy_output_node_writes_body_to_both_report_and_stderr() {
    let canary = "監査用偽機密本文";
    let mut selected = graph(canary, true);
    selected["nodes"][0] =
        json!({"id":"input","kind":"test.output-source","version":1,"config":{"text":canary}});
    let output = cli_with(env!("CARGO_BIN_EXE_diy-ime2-test-host"), "run", selected);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["succeeded"], true);
    assert_eq!(
        report["nodes"]["log"]["outputs"]["out"]["packet"]["root"]["text"],
        canary
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[Output:log"));
    assert!(stderr.contains(canary));
}
#[test]
fn removed_processing_kind_is_refused_before_invocation_or_input_body_diagnostics() {
    let canary = "q-private-canary";
    let mut candidate = graph(canary, false);
    candidate["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"romaji","kind":"e2e.romaji-to-hiragana","version":1}));
    candidate["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from":{"node":"input","port":"out"},"to":{"node":"romaji","port":"in"}}));
    let output = cli("run", candidate);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8(output.stderr).unwrap().contains(canary));
}
