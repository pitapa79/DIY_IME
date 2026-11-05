use super::super::{manager::nonce, secrets::SecretValue};
use super::*;
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone)]
struct Request {
    head: String,
    body: Vec<u8>,
}
fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    let split = loop {
        if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = stream.read(&mut buffer)?;
        if n == 0 || bytes.len() + n > 65536 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        bytes.extend_from_slice(&buffer[..n]);
    };
    let head =
        String::from_utf8(bytes[..split].to_vec()).map_err(|_| std::io::ErrorKind::InvalidData)?;
    let length = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() - split < length {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    Ok(Request {
        head,
        body: bytes[split..split + length].to_vec(),
    })
}
struct Server {
    port: u16,
    records: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(responses: Vec<Vec<u8>>, hold: Option<mpsc::Receiver<()>>) -> Self {
        Self::at("127.0.0.1", responses, hold)
    }
    fn at(host: &str, responses: Vec<Vec<u8>>, hold: Option<mpsc::Receiver<()>>) -> Self {
        let listener = TcpListener::bind((host, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let saved = records.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            let mut hold = hold;
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Ok(request) = read_request(&mut stream) {
                            let index = {
                                let mut rows = saved.lock().unwrap();
                                let index = rows.len();
                                rows.push(request);
                                index
                            };
                            let response = responses
                                .get(index)
                                .cloned()
                                .unwrap_or_else(|| response(500, b"unexpected", ""));
                            if let Some(wait) = hold.take() {
                                let split =
                                    response.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                                let _ = stream.write_all(&response[..split]);
                                let _ = wait.recv_timeout(Duration::from_secs(5));
                                let _ = stream.write_all(&response[split..]);
                            } else {
                                let _ = stream.write_all(&response);
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            records,
            stop,
            worker: Some(worker),
        }
    }
    fn requests(&self) -> Vec<Request> {
        self.records.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn response(status: u16, body: &[u8], extra: &str) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
fn config(port: u16) -> HttpEndpointConfig {
    HttpEndpointConfig {
        host: "127.0.0.1".into(),
        port,
        secure: false,
        allow_insecure_loopback: true,
        method: HttpMethod::Post,
        path: "/chosen?fixed=1".into(),
        content_type: "application/json".into(),
        request_bytes: 512,
        response_bytes: 512,
        header_bytes: 4096,
        timeouts: HttpTimeouts {
            resolve_ms: 2000,
            connect_ms: 2000,
            send_ms: 2000,
            receive_ms: 2000,
            budget_ms: 8000,
        },
        authentication: None,
    }
}
fn plan(config: HttpEndpointConfig, vault: Option<&SecretVault>) -> NetworkPlan {
    NetworkBindings::open(BTreeMap::from([("api".into(), config)]))
        .unwrap()
        .capture("api", vault)
        .unwrap()
}
fn allow() -> Result<(), CoreError> {
    Ok(())
}
struct Vault(PathBuf);
impl Vault {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("diyime-network-{}", nonce().unwrap()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn open(&self) -> SecretVault {
        SecretVault::open(&self.0).unwrap()
    }
}
impl Drop for Vault {
    fn drop(&mut self) {
        for name in ["secrets.json", "secrets.lock"] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}
fn put(vault: &mut SecretVault, value: &str) {
    vault
        .put(
            "service",
            SecretValue::new(value.as_bytes().to_vec()).unwrap(),
        )
        .unwrap();
}
#[test]
fn fixed_policy_rejects_unsafe_endpoints_headers_limits_and_unknown_schema() {
    for mutate in 0..14 {
        let mut cfg = config(12345);
        match mutate {
            0 => cfg.host = "evil.example".into(),
            1 => cfg.host = "localhost".into(),
            2 => cfg.host = "127.1".into(),
            3 => cfg.port = 0,
            4 => cfg.path = "//other.invalid/path".into(),
            5 => cfg.path = "/header\r\nInjected: true".into(),
            6 => cfg.path = "/fragment#x".into(),
            7 => cfg.path = "/back\\slash".into(),
            8 => cfg.content_type = "a\r\nsecret: x".into(),
            9 => cfg.request_bytes = 0,
            10 => cfg.response_bytes = 0,
            11 => cfg.header_bytes = 0,
            12 => cfg.timeouts.receive_ms = 0,
            _ => cfg.allow_insecure_loopback = false,
        };
        assert!(NetworkBindings::open(BTreeMap::from([("api".into(), cfg)])).is_err());
    }
    for header in [
        "Host",
        "Cookie",
        "Content-Length",
        "Proxy-Authorization",
        "bad\r\nname",
    ] {
        let mut cfg = config(12345);
        cfg.authentication = Some(HttpAuthentication {
            secret: "service".into(),
            header: header.into(),
            prefix: "Bearer ".into(),
        });
        assert!(NetworkBindings::open(BTreeMap::from([("api".into(), cfg)])).is_err());
    }
    let mut json = serde_json::to_value(config(12345)).unwrap();
    json["root"] = serde_json::json!("untrusted");
    assert!(serde_json::from_value::<HttpEndpointConfig>(json).is_err());
    let bindings = NetworkBindings::open(BTreeMap::from([("api".into(), config(12345))])).unwrap();
    assert!(bindings.capture("unknown", None).is_err());
}
#[test]
fn real_http_preserves_fixed_method_body_and_injects_only_host_owned_authentication() {
    let server = Server::new(
        vec![response(200, b"reply", ""), response(200, b"reply", "")],
        None,
    );
    let fixture = Vault::new();
    let mut vault = fixture.open();
    put(&mut vault, "synthetic-private-api-value");
    let mut cfg = config(server.port);
    cfg.authentication = Some(HttpAuthentication {
        secret: "service".into(),
        header: "Authorization".into(),
        prefix: "Bearer ".into(),
    });
    let selected = plan(cfg.clone(), Some(&vault));
    let metadata = serde_json::to_string(&selected.description()).unwrap();
    for private in [
        "synthetic-private-api-value",
        &fixture.0.to_string_lossy(),
        "ciphertext",
    ] {
        assert!(!metadata.contains(private));
    }
    let reply = selected.exchange(b"request-body", &mut allow).unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, b"reply");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .head
            .starts_with("POST /chosen?fixed=1 HTTP/1.1\r\n")
    );
    assert!(
        requests[0]
            .head
            .contains("Authorization: Bearer synthetic-private-api-value\r\n")
    );
    assert_eq!(requests[0].body, b"request-body");
    // Maximum supported credential still produces exactly one fixed header.
    let maximum = "s".repeat(4096);
    put(&mut vault, &maximum);
    assert_eq!(
        plan(cfg, Some(&vault))
            .exchange(b"", &mut allow)
            .unwrap()
            .body,
        b"reply"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .head
            .contains(&format!("Authorization: Bearer {maximum}\r\n"))
    );
}
#[test]
fn redirects_are_not_followed_and_cookies_and_automatic_auth_are_not_sent() {
    let sink = Server::new(vec![response(200, b"wrong", "")], None);
    let redirect = Server::new(
        vec![response(
            302,
            b"redirect",
            &format!("Location: http://127.0.0.1:{}/other\r\n", sink.port),
        )],
        None,
    );
    assert_eq!(
        plan(config(redirect.port), None)
            .exchange(b"", &mut allow)
            .unwrap()
            .status,
        302
    );
    assert!(sink.requests().is_empty());
    let server = Server::new(
        vec![
            response(200, b"first", "Set-Cookie: private-cookie=synthetic\r\n"),
            response(
                401,
                b"denied",
                "WWW-Authenticate: Basic realm=synthetic\r\n",
            ),
        ],
        None,
    );
    let selected = plan(config(server.port), None);
    selected.exchange(b"one", &mut allow).unwrap();
    assert_eq!(selected.exchange(b"two", &mut allow).unwrap().status, 401);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let lower = request.head.to_ascii_lowercase();
        assert!(!lower.contains("cookie:"));
        assert!(!lower.contains("authorization:"));
    }
}
#[test]
fn bounded_request_and_response_and_truncation_never_return_partial_data() {
    let silent = Server::new(vec![], None);
    let mut cfg = config(silent.port);
    cfg.request_bytes = 1;
    assert!(plan(cfg, None).exchange(b"too-large", &mut allow).is_err());
    assert!(silent.requests().is_empty());
    for wire in [
        response(200, &[b'x'; 129], ""),
        b"HTTP/1.1 200 Test\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n81\r\n"
            .iter()
            .copied()
            .chain([b'x'; 129])
            .chain(b"\r\n0\r\n\r\n".iter().copied())
            .collect(),
        b"HTTP/1.1 200 Test\r\nContent-Length: 20\r\nConnection: close\r\n\r\nshort".to_vec(),
    ] {
        let server = Server::new(vec![wire], None);
        let mut cfg = config(server.port);
        cfg.response_bytes = 128;
        assert!(plan(cfg, None).exchange(b"", &mut allow).is_err());
        assert_eq!(server.requests().len(), 1);
    }
    let server = Server::new(
        vec![response(
            200,
            b"body",
            &format!("X-Large: {}\r\n", "x".repeat(5000)),
        )],
        None,
    );
    let mut cfg = config(server.port);
    cfg.header_bytes = 1024;
    assert!(plan(cfg, None).exchange(b"", &mut allow).is_err());
    assert_eq!(server.requests().len(), 1);
}
#[test]
fn secret_echo_is_refused_and_rotation_changes_only_new_captured_policy() {
    let fixture = Vault::new();
    let mut vault = fixture.open();
    put(&mut vault, "synthetic-private-echo-value");
    let server = Server::new(
        vec![response(200, b"synthetic-private-echo-value", "")],
        None,
    );
    let mut cfg = config(server.port);
    cfg.authentication = Some(HttpAuthentication {
        secret: "service".into(),
        header: "X-Api-Key".into(),
        prefix: "".into(),
    });
    let bindings = NetworkBindings::open(BTreeMap::from([("api".into(), cfg)])).unwrap();
    let old = bindings.capture("api", Some(&vault)).unwrap();
    let old_metadata = old.description();
    assert_eq!(
        old.exchange(b"", &mut allow).err().unwrap().code,
        "network-secret"
    );
    put(&mut vault, "rotated-private-value");
    let next = bindings.capture("api", Some(&vault)).unwrap();
    assert_ne!(old_metadata.policy_sha256, next.description().policy_sha256);
    assert_eq!(
        old.description().secret.unwrap().version,
        old_metadata.secret.unwrap().version
    );
    vault.remove("service").unwrap();
    assert!(bindings.capture("api", Some(&vault)).is_err());
}
#[test]
fn denied_authority_sends_nothing_and_cancellation_retains_stalled_synchronous_io_until_it_returns()
{
    let silent = Server::new(vec![], None);
    let denied =
        plan(config(silent.port), None).exchange(b"", &mut || Err(fail("cancelled", "withdrawn")));
    assert_eq!(denied.err().unwrap().code, "cancelled");
    assert!(silent.requests().is_empty());
    let (release, held) = mpsc::channel();
    let server = Server::new(vec![response(200, b"late-private", "")], Some(held));
    let selected = plan(config(server.port), None);
    let cancelled = Arc::new(AtomicBool::new(false));
    let token = cancelled.clone();
    let (receiving, started) = mpsc::channel();
    let (done, ended) = mpsc::channel();
    let worker = thread::spawn(move || {
        windows::observe_read(receiving);
        let result = selected.exchange(b"request", &mut || {
            if token.load(Ordering::Acquire) {
                Err(fail("cancelled", "withdrawn"))
            } else {
                Ok(())
            }
        });
        done.send(result.map(|_| ())).unwrap();
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    cancelled.store(true, Ordering::Release);
    thread::sleep(Duration::from_millis(50));
    assert!(matches!(ended.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(
        ended
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .err()
            .unwrap()
            .code,
        "cancelled"
    );
    worker.join().unwrap();
    assert_eq!(server.requests().len(), 1);
}
#[test]
fn secure_binding_sends_tls_instead_of_falling_back_to_plain_http() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (seen, captured) = mpsc::channel();
    let worker = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut bytes = [0; 8];
                    let n = stream.read(&mut bytes).unwrap_or(0);
                    if n == 0 {
                        continue;
                    }
                    seen.send(bytes[..n].to_vec()).unwrap();
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => break,
            }
        }
        let _ = seen.send(vec![]);
    });
    let mut cfg = config(port);
    cfg.secure = true;
    cfg.allow_insecure_loopback = false;
    let error = plan(cfg, None).exchange(b"", &mut allow).err().unwrap();
    let prefix = captured.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert_eq!(
        prefix.first(),
        Some(&22),
        "a secure binding must start a TLS handshake: {}",
        error.message
    );
}

#[test]
fn untrusted_self_signed_certificate_is_rejected_without_changing_user_trust_stores() {
    use std::{
        io::{BufRead, BufReader},
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/network-untrusted-tls.ps1");
    let mut child = Command::new("pwsh.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
        .arg(script)
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let errors = child.stderr.take().unwrap();
    let diagnostics = thread::spawn(move || {
        let mut bytes = Vec::new();
        errors.take(16384).read_to_end(&mut bytes).unwrap();
        bytes
    });
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let port = line.trim().parse::<u16>();
    if port.is_err() {
        let status = child.wait().unwrap();
        let errors = diagnostics.join().unwrap();
        panic!(
            "synthetic certificate fixture failed: {status}; {}",
            String::from_utf8_lossy(&errors)
        );
    }
    let mut cfg = config(port.unwrap());
    cfg.secure = true;
    cfg.allow_insecure_loopback = false;
    let error = plan(cfg, None).exchange(b"", &mut allow).err().unwrap();
    drop(child.stdin.take());
    let mut result = String::new();
    output.read_to_string(&mut result).unwrap();
    let status = child.wait().unwrap();
    let errors = diagnostics.join().unwrap();
    assert!(
        status.success(),
        "synthetic certificate fixture: {}",
        String::from_utf8_lossy(&errors)
    );
    assert!(result.contains("self-check-ok"));
    assert!(
        !result.contains("http-received"),
        "untrusted endpoint received application bytes"
    );
    assert_eq!(error.code, "network-io");
    assert!(
        error.message.contains("12175"),
        "certificate validation must fail closed: {}",
        error.message
    );
}

#[test]
fn ipv6_loopback_get_uses_fixed_literal_and_unambiguous_destination() {
    let server = Server::at("::1", vec![response(204, b"", "")], None);
    let mut cfg = config(server.port);
    cfg.host = "::1".into();
    cfg.method = HttpMethod::Get;
    let selected = plan(cfg, None);
    assert_eq!(
        selected.description().destination,
        format!("http://[::1]:{}/chosen?fixed=1", server.port)
    );
    assert_eq!(selected.exchange(b"", &mut allow).unwrap().status, 204);
    let rows = server.requests();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].head.starts_with("GET /chosen?fixed=1 HTTP/1.1"));
    assert!(rows[0].body.is_empty());
}

#[test]
fn cooperative_budget_withdraws_a_late_body_only_after_owned_os_read_returns() {
    let (release, held) = mpsc::channel();
    let server = Server::new(vec![response(200, b"late-private", "")], Some(held));
    let mut cfg = config(server.port);
    cfg.timeouts.budget_ms = 1000;
    cfg.timeouts.receive_ms = 5000;
    let selected = plan(cfg, None);
    let (reading, started) = mpsc::channel();
    let (done, ended) = mpsc::channel();
    let worker = thread::spawn(move || {
        windows::observe_read(reading);
        done.send(selected.exchange(b"", &mut allow).map(|_| ()))
            .unwrap();
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    thread::sleep(Duration::from_millis(1100));
    assert!(matches!(ended.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(
        ended
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .err()
            .unwrap()
            .code,
        "network-deadline"
    );
    worker.join().unwrap();
}
#[test]
fn unsafe_authentication_value_never_opens_a_connection_or_echoes_the_value() {
    let server = Server::new(vec![], None);
    let fixture = Vault::new();
    let mut vault = fixture.open();
    put(&mut vault, "private-key\r\nInjected: value");
    let mut cfg = config(server.port);
    cfg.authentication = Some(HttpAuthentication {
        secret: "service".into(),
        header: "Authorization".into(),
        prefix: "Bearer ".into(),
    });
    let error = plan(cfg, Some(&vault))
        .exchange(b"", &mut allow)
        .err()
        .unwrap();
    assert_eq!(error.code, "network-secret");
    assert!(!error.message.contains("private-key"));
    assert!(server.requests().is_empty());
}
#[test]
fn endpoint_limits_and_vault_location_are_part_of_current_captured_policy() {
    let cfg = config(12345);
    let original = plan(cfg.clone(), None).description().policy_sha256;
    for change in 0..3 {
        let mut selected = cfg.clone();
        match change {
            0 => selected.path = "/another".into(),
            1 => selected.response_bytes += 1,
            _ => selected.timeouts.budget_ms += 1,
        };
        assert_ne!(original, plan(selected, None).description().policy_sha256);
    }
    let one = Vault::new();
    let two = Vault::new();
    let mut source = one.open();
    put(&mut source, "synthetic-location-private");
    fs::copy(one.0.join("secrets.json"), two.0.join("secrets.json")).unwrap();
    let copied = two.open();
    let mut cfg = cfg;
    cfg.authentication = Some(HttpAuthentication {
        secret: "service".into(),
        header: "Authorization".into(),
        prefix: "Bearer ".into(),
    });
    let a = plan(cfg.clone(), Some(&source)).description();
    let b = plan(cfg, Some(&copied)).description();
    assert_eq!(a.secret.unwrap().version, b.secret.unwrap().version);
    assert_ne!(a.policy_sha256, b.policy_sha256);
}
