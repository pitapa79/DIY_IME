use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
struct PolicyFile(PathBuf);
impl PolicyFile {
    fn new(value: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "diy-ime2-v3-policy-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
        Self(path)
    }
}
impl Drop for PolicyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn policy() -> Value {
    serde_json::from_str::<Value>(include_str!("contracts/execution_control.json")).unwrap()["resourceProfiles"]["bounded"].clone()
}
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    id: u64,
    _policy: PolicyFile,
}
impl Client {
    fn new(policy: Value) -> Self {
        let policy = PolicyFile::new(&policy);
        let mut child = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
            .arg("session-v3")
            .arg(&policy.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
            id: 0,
            _policy: policy,
        }
    }
    fn request(&mut self, request: Value) -> Value {
        self.id += 1;
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            json!({"protocol":3,"id":self.id.to_string(),"request":request})
        )
        .unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], self.id.to_string());
        assert_eq!(reply["protocol"], 3);
        reply
    }
    fn ok(&mut self, request: Value) -> Value {
        let reply = self.request(request);
        assert_eq!(reply["ok"], true, "{reply}");
        reply
    }
    fn finish(&mut self) {
        self.input.take();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < end, "CLI did not finish after EOF");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
#[test]
fn actual_v3_cli_runs_existing_input_nodes_and_queries_an_ignored_acceptance() {
    let policy = policy();
    let mut client = Client::new(policy.clone());
    let hello = client.ok(json!({"method":"hello"}));
    assert_eq!(hello["data"]["resourcePolicy"], policy);
    assert!(!hello["data"]["resourceDomain"].as_str().unwrap().is_empty());
    assert_eq!(hello["state"]["revision"], "0");
    let graph =
        json!({"version":1,"nodes":[{"id":"source","kind":"input.text","version":1}],"edges":[]});
    let prepared = client.ok(json!({"method":"prepare","graph":graph}));
    let active = client.ok(json!({"method":"activate","token":prepared["data"]["token"]}));
    let request = json!({"method":"start-run","startKey":"18446744073709551615","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],"input":{"revision":7,"text":"kya😀","mode":"compose"}});
    drop(client.request(request.clone())); // Do not use the delivered acceptance.
    let key =
        client.ok(json!({"method":"get-start","startKey":"18446744073709551615"}))["data"]["key"]
            .clone();
    let end = Instant::now() + Duration::from_secs(5);
    let completed = loop {
        let reply = client.ok(json!({"method":"get-run","key":key}));
        if reply["data"]["state"] == "terminal" {
            break reply;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(
        completed["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "kya😀"
    );
    let replay = client.ok(request);
    assert_eq!(replay["data"]["replayed"], true);
    assert_eq!(replay["data"]["key"], key);
    assert_eq!(
        client.ok(json!({"method":"status"}))["state"]["revision"],
        active["state"]["revision"]
    );
    assert_eq!(
        client.ok(json!({"method":"cancel-run","key":key}))["data"]["result"],
        completed["data"]["result"]
    );
    let invalid = client.request(json!({"method":"hello","extra":true}));
    assert_eq!(invalid["error"]["code"], "protocol-request");
    client.finish();
}
#[test]
fn v3_cli_requires_an_explicit_complete_policy_before_emitting_any_protocol_reply() {
    for args in [vec!["session-v3"], vec!["session-v3", "-"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let file = PolicyFile::new(&json!({"maxPhysicalJobs":1}));
    let output = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
        .arg("session-v3")
        .arg(&file.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid-resource-policy"));
}
#[test]
fn old_cli_session_mode_remains_v2_and_rejects_v3_before_interpreting_methods() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
        .arg("session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        json!({"protocol":3,"id":1,"request":{"method":"start-run"}})
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        json!({"protocol":2,"id":2,"request":{"method":"status"}})
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["protocol"], 2);
    assert_eq!(lines[0]["error"]["code"], "protocol-version");
    assert_eq!(lines[1]["ok"], true);
    assert_eq!(lines[0]["state"], lines[1]["state"]);
}
