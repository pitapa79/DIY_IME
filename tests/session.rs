use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    id: u64,
}
impl Client {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
            .arg("session")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
            id: 0,
        }
    }
    fn line(&mut self, line: &str) -> Value {
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
        let mut reply = String::new();
        self.output.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }
    fn send(&mut self, request: Value) -> Value {
        self.id += 1;
        let reply = self.line(&json!({"protocol":2,"id":self.id,"request":request}).to_string());
        assert_eq!(reply["id"], self.id);
        assert_eq!(reply["protocol"], 2);
        reply
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn hello() -> Value {
    serde_json::from_str(include_str!("../examples/graphs/hello.json")).unwrap()
}
fn edited() -> Value {
    let mut graph = hello();
    graph["nodes"][0]["config"]["packet"]["root"] = json!({"type":"text","text":"updated"});
    graph
}
fn run(client: &mut Client, version: u64) -> Value {
    client.send(json!({"method":"run","expectedPlanVersion":version}))
}
fn output(reply: &Value) -> &str {
    reply["data"]["nodes"]["result"]["outputs"]["out"]["packet"]["root"]["text"]
        .as_str()
        .unwrap()
}

#[test]
fn persistent_session_prepares_activates_preserves_old_plan_and_rolls_back() {
    let mut client = Client::new();
    assert_eq!(run(&mut client, 1)["error"]["code"], "no-active-plan");
    let prepared = client.send(json!({"method":"prepare","graph":hello()}));
    assert_eq!(prepared["ok"], true);
    assert!(prepared["state"]["active"].is_null());
    let token = prepared["state"]["prepared"]["token"].clone();
    let activated = client.send(json!({"method":"activate","token":token}));
    assert_eq!(activated["state"]["active"]["version"], 1);
    let first = run(&mut client, 1);
    assert_eq!(output(&first), "HELLO 𠮷");
    let prepared = client.send(json!({"method":"prepare","graph":edited()}));
    let token = prepared["state"]["prepared"]["token"].clone();
    let old = run(&mut client, 1);
    assert_eq!(output(&old), "HELLO 𠮷");
    assert!(old["data"]["runId"].as_u64().unwrap() > first["data"]["runId"].as_u64().unwrap());
    let mut broken = edited();
    broken["edges"] = json!([]);
    let invalid = client.send(json!({"method":"prepare","graph":broken}));
    assert_eq!(invalid["error"]["code"], "missing-input");
    assert_eq!(invalid["state"]["active"]["version"], 1);
    assert_eq!(invalid["state"]["prepared"]["token"], token);
    assert_eq!(output(&run(&mut client, 1)), "HELLO 𠮷");
    assert_eq!(
        client.send(json!({"method":"activate","token":token}))["state"]["active"]["version"],
        2
    );
    assert_eq!(output(&run(&mut client, 2)), "UPDATED");
    let restored = client.send(json!({"method":"rollback","expectedPlanVersion":2}));
    assert_eq!(restored["state"]["active"]["version"], 3);
    assert_eq!(restored["state"]["active"]["graph"], hello());
    assert_eq!(restored["state"]["canRollback"], false);
    assert_eq!(output(&run(&mut client, 3)), "HELLO 𠮷");
}

#[test]
fn stale_tokens_and_plan_versions_do_not_mutate_the_active_plan() {
    let mut client = Client::new();
    let first = client.send(json!({"method":"prepare","graph":hello()}));
    let second = client.send(json!({"method":"prepare","graph":edited()}));
    assert_eq!(
        client.send(json!({"method":"activate","token":first["state"]["prepared"]["token"]}))["error"]
            ["code"],
        "stale-preparation"
    );
    client.send(json!({"method":"activate","token":second["state"]["prepared"]["token"]}));
    assert_eq!(run(&mut client, 0)["error"]["code"], "plan-conflict");
    assert_eq!(
        client.send(json!({"method":"rollback","expectedPlanVersion":0}))["error"]["code"],
        "plan-conflict"
    );
    assert_eq!(
        client.send(json!({"method":"rollback","expectedPlanVersion":1}))["error"]["code"],
        "no-rollback"
    );
    assert_eq!(output(&run(&mut client, 1)), "UPDATED");
}

#[test]
fn sessions_are_independent_and_validation_does_not_prepare_a_plan() {
    let mut first = Client::new();
    let mut second = Client::new();
    first.send(json!({"method":"prepare","graph":hello()}));
    let checked = second.send(json!({"method":"validate","graph":hello()}));
    assert_eq!(
        checked["data"]["order"],
        json!(["input", "uppercase", "result"])
    );
    assert!(checked["state"]["prepared"].is_null());
    assert!(second.send(json!({"method":"status"}))["state"]["active"].is_null());
    assert!(!first.send(json!({"method":"status"}))["state"]["prepared"].is_null());
}

#[test]
fn malformed_or_incompatible_messages_are_framed_errors_and_session_recovers() {
    let mut client = Client::new();
    assert_eq!(client.line("{")["error"]["code"], "protocol-json");
    let wrong = client.line(r#"{"protocol":9,"id":99,"request":{"method":"status"}}"#);
    assert_eq!(wrong["id"], 99);
    assert_eq!(wrong["error"]["code"], "protocol-version");
    assert_eq!(
        client.send(json!({"method":"status","extra":true}))["error"]["code"],
        "protocol-request"
    );
    assert_eq!(
        client.send(json!({"method":"definitions"}))["data"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
    assert_eq!(client.send(json!({"method":"status"}))["ok"], true);
}

#[test]
fn oversized_frames_end_the_session_without_executing_trailing_commands() {
    let mut input = vec![b' '; diy_ime2_core::session::MAX_REQUEST_BYTES + 1];
    input.extend_from_slice(b"\n{\"protocol\":2,\"id\":2,\"request\":{\"method\":\"status\"}}\n");
    let mut output = Vec::new();
    diy_ime2_core::session::serve(std::io::Cursor::new(input), &mut output).unwrap();
    let lines: Vec<_> = std::str::from_utf8(&output).unwrap().lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(lines[0]).unwrap()["error"]["code"],
        "request-limit"
    );
}

#[test]
fn protocol_two_round_trips_runtime_inputs_and_recovers_after_invalid_snapshots() {
    let mut client = Client::new();
    assert_eq!(
        client.line(r#"{"protocol":1,"id":90,"request":{"method":"status"}}"#)["error"]["code"],
        "protocol-version"
    );
    let graph: Value =
        serde_json::from_str(include_str!("../examples/graphs/runtime-input.json")).unwrap();
    let prepared = client.send(json!({"method":"prepare","graph":graph}));
    client.send(json!({"method":"activate","token":prepared["state"]["prepared"]["token"]}));
    let first=client.send(json!({"method":"run","expectedPlanVersion":1,"input":{"revision":1,"text":"k","mode":"compose"}}));
    assert_eq!(first["data"]["inputRevision"], 1);
    assert_eq!(output(&first), "k");
    for input in [
        Value::Null,
        json!({"revision":0,"text":"a","mode":"compose"}),
        json!({"revision":2,"text":"a","mode":"bad"}),
    ] {
        let invalid = client.send(json!({"method":"run","expectedPlanVersion":1,"input":input}));
        assert_eq!(invalid["error"]["code"], "invalid-runtime-input");
        assert_eq!(invalid["state"], first["state"]);
    }
    let next=client.send(json!({"method":"run","expectedPlanVersion":1,"input":{"revision":2,"text":"kya","mode":"finalize"}}));
    assert_eq!(output(&next), "kya");
    assert_eq!(next["data"]["inputRevision"], 2);
    assert_eq!(next["state"], first["state"]);
    let missing = client.send(json!({"method":"run","expectedPlanVersion":1}));
    assert_eq!(missing["data"]["succeeded"], false);
}
