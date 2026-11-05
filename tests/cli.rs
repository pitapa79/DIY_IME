use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

fn invoke(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn cli_runs_a_real_graph_from_stdin() {
    let output = invoke(&["run", "-"], include_str!("../examples/graphs/hello.json"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["succeeded"], true);
    assert_eq!(
        report["nodes"]["result"]["outputs"]["out"]["packet"]["root"]["text"],
        "HELLO 𠮷"
    );
}

#[test]
fn cli_exposes_inactive_branches_without_treating_them_as_errors() {
    let output = invoke(
        &["run", "-"],
        include_str!("../examples/graphs/branch.json"),
    );
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["nodes"]["inactive"]["status"], "skipped");
    assert_eq!(
        report["nodes"]["inactive"]["outputs"]["out"]["status"],
        "no-output"
    );
    assert_eq!(
        report["nodes"]["selected"]["outputs"]["out"]["packet"]["root"]["text"],
        "HELLO"
    );
}

#[test]
fn cli_validates_graph_without_running_it_and_lists_node_settings() {
    let output = invoke(
        &["validate-graph", "-"],
        include_str!("../examples/graphs/hello.json"),
    );
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["order"],
        serde_json::json!(["input", "uppercase", "result"])
    );
    let output = invoke(&["list-nodes"], "");
    assert!(output.status.success());
    let definitions: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(definitions.as_array().unwrap().len(), 9);
    assert!(definitions[0].get("configSchema").is_some());
}

#[test]
fn cli_rejects_invalid_packets_graphs_and_arguments() {
    for (args, input) in [
        (
            vec!["validate-packet", "-"],
            r#"{"version":2,"root":{"type":"text","text":"a"}}"#,
        ),
        (
            vec!["run", "-"],
            r#"{"version":1,"nodes":[{"id":"a","kind":"missing","version":1}],"edges":[]}"#,
        ),
        (vec!["run", "-"], "{"),
        (vec!["unknown"], ""),
        (vec!["run"], ""),
    ] {
        let output = invoke(&args, input);
        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn cli_round_trips_packet_and_reads_graph_from_file() {
    let output = invoke(
        &["validate-packet", "-"],
        r#"{"version":1,"root":{"type":"text","text":"日本語"}}"#,
    );
    assert!(output.status.success());
    let packet: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["root"]["text"], "日本語");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/graphs/hello.json");
    assert!(
        invoke(&["run", path.to_str().unwrap()], "")
            .status
            .success()
    );
    assert!(
        !invoke(&["run", "this-file-does-not-exist.json"], "")
            .status
            .success()
    );
}
