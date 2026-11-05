use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
struct PolicyFile(PathBuf);
impl PolicyFile {
    fn new(value: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "diy-ime2-broker-policy-{}-{}.json",
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
    let mut p = serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
        .unwrap()["resourceProfiles"]["bounded"]
        .clone();
    p["retentionMs"] = json!(60000);
    p
}
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
    id: u64,
    _policy: PolicyFile,
}
impl Client {
    fn new(policy: Value) -> Self {
        let policy = PolicyFile::new(&policy);
        let mut child = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
            .arg("broker-v3")
            .arg(&policy.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(reply) = serde_json::from_str(&line) else {
                    break;
                };
                if send.send(reply).is_err() {
                    break;
                }
            }
        });
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
            json!({"broker":1,"id":self.id.to_string(),"request":request})
        )
        .unwrap();
        input.flush().unwrap();
        let reply = self
            .output
            .recv_timeout(Duration::from_secs(5))
            .expect("broker CLI did not reply");
        assert_eq!(reply["broker"], 1);
        assert_eq!(reply["id"], self.id.to_string());
        reply
    }
    fn ok(&mut self, request: Value) -> Value {
        let reply = self.request(request);
        assert_eq!(reply["ok"], true, "{reply}");
        reply["data"].clone()
    }
    fn open(&mut self) -> Value {
        self.ok(json!({"method":"open-session"}))
    }
    fn inner(&mut self, epoch: &Value, request: Value) -> Value {
        self.ok(json!({"method":"request","sessionEpoch":epoch,"message":{"protocol":3,"id":"18446744073709551615","request":request}}))
    }
    fn prepare(&mut self, epoch: &Value) {
        let prepared=self.inner(epoch,json!({"method":"prepare","graph":{"version":1,"nodes":[{"id":"source","kind":"input.text","version":1}],"edges":[]}}));
        assert_eq!(prepared["ok"], true, "{prepared}");
        let active = self.inner(
            epoch,
            json!({"method":"activate","token":prepared["data"]["token"]}),
        );
        assert_eq!(active["ok"], true, "{active}");
    }
    fn completed(&mut self, epoch: &Value, key: &Value) -> Value {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = self.inner(epoch, json!({"method":"get-run","key":key}));
            assert_eq!(reply["ok"], true, "{reply}");
            if reply["data"]["state"] == "terminal" {
                return reply;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn finish(&mut self) {
        self.input.take();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < end, "broker process did not finish");
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
fn actual_broker_cli_keeps_one_host_across_sessions_and_reconnect_without_restoring_old_runs() {
    let p = policy();
    let mut client = Client::new(p.clone());
    let a = client.open();
    let b = client.open();
    assert_eq!(a["resourcePolicy"], p);
    assert_eq!(a["resourceDomain"], b["resourceDomain"]);
    assert_ne!(a["sessionEpoch"], b["sessionEpoch"]);
    let ae = &a["sessionEpoch"];
    let be = &b["sessionEpoch"];
    let hello = client.inner(ae, json!({"method":"hello"}));
    assert_eq!(hello["data"]["sessionEpoch"], *ae);
    assert_eq!(hello["data"]["resourceDomain"], a["resourceDomain"]);
    client.prepare(ae);
    assert!(client.inner(be, json!({"method":"status"}))["state"]["activePlanVersion"].is_null());
    client.prepare(be);
    let request = json!({"method":"start-run","startKey":"18446744073709551615","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],"input":{"revision":1,"text":"A😀","mode":"compose"}});
    drop(client.inner(ae, request.clone())); // Discard the delivered acceptance.
    let key = client.inner(
        ae,
        json!({"method":"get-start","startKey":"18446744073709551615"}),
    )["data"]["key"]
        .clone();
    let completed = client.completed(ae, &key);
    assert_eq!(
        completed["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "A😀"
    );
    assert_eq!(client.inner(ae, request)["data"]["replayed"], true);
    let second=client.inner(be,json!({"method":"start-run","startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],"input":{"revision":1,"text":"B","mode":"finalize"}}));
    let bkey = second["data"]["key"].clone();
    assert_ne!(bkey["sessionEpoch"], key["sessionEpoch"]);
    assert_eq!(
        client.completed(be, &bkey)["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
        "B"
    );
    client.ok(json!({"method":"close-session","sessionEpoch":ae}));
    let fresh = client.open();
    let fe = &fresh["sessionEpoch"];
    assert_ne!(fe, ae);
    assert_eq!(fresh["resourceDomain"], a["resourceDomain"]);
    assert!(client.inner(fe, json!({"method":"status"}))["state"]["activePlanVersion"].is_null());
    assert_eq!(
        client.inner(fe, json!({"method":"get-run","key":key}))["error"]["code"],
        "session-conflict"
    );
    assert_eq!(
        client.inner(
            fe,
            json!({"method":"get-start","startKey":"18446744073709551615"})
        )["error"]["code"],
        "start-not-seen"
    );
    let stale=client.request(json!({"method":"request","sessionEpoch":ae,"message":{"protocol":3,"id":"1","request":{"method":"status"}}}));
    assert_eq!(stale["error"]["code"], "broker-session-unknown");
    assert_eq!(
        client.completed(be, &bkey)["data"]["result"]["reason"],
        "completed"
    );
    let v2=client.ok(json!({"method":"request","sessionEpoch":be,"message":{"protocol":2,"id":1,"request":{"method":"status"}}}));
    assert_eq!(v2["protocol"], 3);
    assert_eq!(v2["error"]["code"], "protocol-version");
    client.open();
    client.open();
    assert_eq!(
        client.request(json!({"method":"open-session"}))["error"]["code"],
        "busy"
    );
    assert_eq!(
        client.request(json!({"method":"open-session","extra":true}))["error"]["code"],
        "broker-request"
    );
    client.finish();
}
#[test]
fn actual_broker_cli_rejects_each_missing_policy_field_before_protocol_output() {
    for args in [vec!["broker-v3"], vec!["broker-v3", "-"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let p = policy();
    assert_eq!(p.as_object().unwrap().len(), 13);
    for field in p.as_object().unwrap().keys() {
        let mut invalid = p.clone();
        invalid.as_object_mut().unwrap().remove(field);
        let file = PolicyFile::new(&invalid);
        let output = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
            .arg("broker-v3")
            .arg(&file.0)
            .output()
            .unwrap();
        assert!(!output.status.success(), "missing {field}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid-resource-policy"));
    }
}
