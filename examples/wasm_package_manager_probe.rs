//! Real package-manager IPC, cancellation and EOF with actual AppContainer guests.
#[cfg(all(windows, target_arch = "x86_64"))]
mod probe {
    use diy_ime2_core::{
        execution_control::{CancellationToken, DeferredNode},
        isolated_worker::{
            adapter::{ExecutionGate, WasmArtifact, WorkerPolicy},
            package::{Manifest, Package, PackageLimits, RuntimeKind, module_sha256},
        },
    };
    use serde_json::{Value, json};
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };
    struct Client {
        child: Child,
        lines: mpsc::Receiver<String>,
        reader: Option<std::thread::JoinHandle<()>>,
    }
    impl Client {
        fn new(exe: &str, config: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
            let mut child = Command::new(exe)
                .arg(config)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?;
            let out = child.stdout.take().unwrap();
            let (sender, lines) = mpsc::channel();
            let reader = Some(std::thread::spawn(move || {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            }));
            Ok(Self {
                child,
                lines,
                reader,
            })
        }
        fn send(
            &mut self,
            id: &str,
            operation: Value,
            payload: &[u8],
        ) -> Result<(), Box<dyn std::error::Error>> {
            let json = serde_json::to_vec(&json!({"id":id,"operation":operation}))?;
            let input = self.child.stdin.as_mut().unwrap();
            input.write_all(&(json.len() as u32).to_le_bytes())?;
            input.write_all(&(payload.len() as u64).to_le_bytes())?;
            input.write_all(&json)?;
            input.write_all(payload)?;
            input.flush()?;
            Ok(())
        }
        fn reply(&self) -> Result<Value, Box<dyn std::error::Error>> {
            Ok(serde_json::from_str(
                &self.lines.recv_timeout(Duration::from_secs(15))?,
            )?)
        }
        fn call(
            &mut self,
            id: &str,
            op: Value,
            payload: &[u8],
        ) -> Result<Value, Box<dyn std::error::Error>> {
            self.send(id, op, payload)?;
            let result = self.reply()?;
            assert_eq!(result["id"], id);
            Ok(result)
        }
        fn end(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            self.child.stdin.take();
            assert!(self.child.wait()?.success());
            if let Some(reader) = self.reader.take() {
                reader.join().unwrap();
            }
            Ok(())
        }
    }
    impl Drop for Client {
        fn drop(&mut self) {
            self.child.stdin.take();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }
    fn children(parent: u32) -> usize {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Diagnostics::ToolHelp::*,
        };
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            assert_ne!(snapshot, INVALID_HANDLE_VALUE);
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry);
            let mut count = 0;
            while more != 0 {
                if entry.th32ParentProcessID == parent {
                    count += 1;
                }
                more = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            count
        }
    }
    fn wait(done: impl Fn() -> bool) -> Result<(), Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(10);
        while !done() {
            if Instant::now() > end {
                return Err("manager physical process observation timed out".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn batch(package: &Package) -> Vec<u8> {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&(package.bytes().len() as u64).to_le_bytes());
        bytes.extend_from_slice(package.bytes());
        bytes
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args().collect();
        if args.len() != 5 {
            return Err("usage: manager_probe MANAGER WORKER DLL FIXTURE_WASM".into());
        }
        let root =
            std::env::temp_dir().join(format!("diyime-manager-probe-{}", std::process::id()));
        fs::create_dir(&root)?;
        let workspace = root.join("workers");
        fs::create_dir(&workspace)?;
        let module = fs::read(&args[4])?;
        let artifact = WasmArtifact::capture(
            &workspace,
            &PathBuf::from(&args[2]),
            &PathBuf::from(&args[3]),
            module.clone(),
            WorkerPolicy::bounded_probe(),
        )?;
        let definition = artifact
            .prepare(
                "fixture.wasm",
                1,
                vec![],
                ExecutionGate::default(),
                CancellationToken::default(),
            )?
            .definition();
        let manifest = Manifest {
            package_id: "test.fixture".into(),
            version: "1.0.0".parse()?,
            display_name: "Fixture".into(),
            runtime: RuntimeKind::Wasm32,
            guest_api_version: 1,
            node: definition,
            module_sha256: module_sha256(&module)?,
            capabilities: vec![],
        };
        let limits = PackageLimits::default();
        let package = Package::encode(manifest.clone(), &module, &limits)?;
        let mut hang = manifest;
        hang.node.kind = "fixture.wasm.hang".into();
        let hang = Package::encode(hang, &module, &limits)?;
        let config = root.join("config.json");
        fs::write(
            &config,
            serde_json::to_vec(
                &json!({"dataRoot":root.join("data"),"workspace":workspace,"worker":args[2],"runtimeDll":args[3],"policy":{"processMemoryBytes":536870912u64,"wasmMemoryBytes":134217728u64,"fuelPerCall":null,"walltimeMs":null,"terminationGraceMs":5}}),
            )?,
        )?;
        let mut client = Client::new(&args[1], &config)?;
        assert_eq!(
            client.call("list", json!({"method":"list"}), &[])?["data"]["catalog"]["entries"],
            json!([])
        );
        let review = client.call("review", json!({"method":"review"}), &batch(&package))?;
        assert_eq!(review["ok"], true);
        assert_eq!(review["data"]["executionPermissionGranted"], false);
        let review_id = review["data"]["reviewId"].as_str().unwrap();
        assert_eq!(client.call("forged",json!({"method":"commit","reviewId":"forged","allowDowngrade":false,"allowSameVersionRebuild":false}),&[])?["error"]["code"],"manager-review");
        assert_eq!(client.call("commit",json!({"method":"commit","reviewId":review_id,"allowDowngrade":false,"allowSameVersionRebuild":false}),&[])?["data"]["executionPermissionGranted"],false);
        assert_eq!(client.call("replay",json!({"method":"commit","reviewId":review_id,"allowDowngrade":true,"allowSameVersionRebuild":true}),&[])?["error"]["code"],"manager-review");
        client.send("hang", json!({"method":"review"}), &batch(&hang))?;
        wait(|| children(client.child.id()) > 0)?;
        assert_eq!(
            client.call("busy", json!({"method":"list"}), &[])?["error"]["code"],
            "manager-busy"
        );
        client.send("cancel", json!({"method":"cancel","requestId":"hang"}), &[])?;
        let first = client.reply()?;
        let second = client.reply()?;
        let cancelled = if first["id"] == "hang" { first } else { second };
        assert_eq!(cancelled["id"], "hang");
        assert_eq!(cancelled["ok"], false);
        wait(|| children(client.child.id()) == 0)?;
        let recovery = client.call("recovery", json!({"method":"review"}), &batch(&package))?;
        assert_eq!(recovery["ok"], true);
        assert_eq!(
            client.call(
                "discard",
                json!({"method":"discard","reviewId":recovery["data"]["reviewId"]}),
                &[]
            )?["data"]["discarded"],
            true
        );
        client.call("close", json!({"method":"close"}), &[])?;
        client.end()?;
        let mut eof = Client::new(&args[1], &config)?;
        let pid = eof.child.id();
        eof.send("eof", json!({"method":"review"}), &batch(&hang))?;
        wait(|| children(pid) > 0)?;
        eof.end()?;
        wait(|| children(pid) == 0)?;
        println!(
            "Package manager: real isolated import, opaque review/replay refusal, busy, cancel after actual process start, recovery, close and EOF physical exit passed; diagnostics {}",
            root.display()
        );
        Ok(())
    }
}
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = probe::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("Windows x64 required");
        std::process::exit(1);
    }
}
