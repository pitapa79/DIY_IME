//! Actual production CLI composition, distinct from the synthetic protocol test Host.
use std::{
    io::Write,
    process::{Command, Stdio},
};
fn invoke(command: &str, input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_diy-ime2"))
        .args(if command == "list-nodes" {
            vec![command]
        } else {
            vec![command, "-"]
        })
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
fn production_cli_has_only_system_adapters_and_never_falls_back_to_processing_copies() {
    let output = invoke("list-nodes", "");
    assert!(output.status.success());
    let defs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let actual: std::collections::BTreeSet<_> =
        defs.iter().map(|d| d["kind"].as_str().unwrap()).collect();
    assert_eq!(
        actual,
        ["demo.packet", "e2e.input", "e2e.output", "input.text"].into()
    );
    assert_eq!(defs.len(), 4);
    let old_graph = include_str!("../examples/graphs/hello.json");
    let refused = invoke("run", old_graph);
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    let graph = r#"{"version":1,"nodes":[{"id":"source","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":{"type":"text","text":"unchanged 𠮷"}}}}],"edges":[]}"#;
    let success = invoke("run", graph);
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(
        report["nodes"]["source"]["outputs"]["out"]["packet"]["root"]["text"],
        "unchanged 𠮷"
    );
}
