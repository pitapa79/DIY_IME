//! Scoped canary and real-Wasm launch through an experimental AppContainer Worker.
#[cfg(windows)]
#[path = "security_support/windows_worker.rs"]
mod worker;
#[cfg(windows)]
mod fixture {
    use super::worker;
    use serde_json::{Value, json};
    use std::{
        ffi::OsString,
        io::{self, Read, Write},
        net::{SocketAddr, TcpListener, TcpStream},
        path::Path,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    fn child(args: &[OsString]) -> Result<Value, String> {
        if args.len() != 3 {
            return Err("invalid child canary args".into());
        }
        let private = Path::new(&args[0]);
        let allowed = Path::new(&args[1]);
        let port = args[2]
            .to_string_lossy()
            .parse::<u16>()
            .map_err(|_| "invalid canary port")?;
        let private_read = std::fs::read(private).is_ok();
        let allowed_read = std::fs::read(allowed).is_ok_and(|b| b == b"allowed-node-canary");
        let write = std::fs::OpenOptions::new()
            .write(true)
            .open(allowed)
            .is_ok();
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let network = TcpStream::connect_timeout(&address, Duration::from_millis(500))
            .and_then(|mut s| s.write_all(b"canary"))
            .is_ok();
        let status = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--descendant")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(
            json!({"privateRead":private_read,"allowedRead":allowed_read,"allowedWrite":write,"networkConnected":network,"secretPresent":std::env::var_os("DIYIME2_SECRET_CANARY").is_some(),"descendantSucceeded":status.is_ok_and(|s|s.success())}),
        )
    }
    fn received(listener: &TcpListener) -> Result<usize, String> {
        let mut count = 0;
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(500)))
                        .map_err(|e| e.to_string())?;
                    let mut bytes = [0u8; 16];
                    let _ = stream.read(&mut bytes);
                    count += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(count),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    fn canary() -> Result<Value, String> {
        let mut profile = worker::Profile::new()?;
        let cache = worker::Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
        let source = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = cache.copy(&source, "canary-worker.exe")?;
        let allowed = cache.write("allowed.txt", b"allowed-node-canary")?;
        let private = cache.private_canary(b"private-node-canary")?;
        if std::fs::read(&private).map_err(|e| e.to_string())? != b"private-node-canary" {
            return Err("Parent canary unreadable".into());
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let args = vec![
            private.as_os_str().to_owned(),
            allowed.as_os_str().to_owned(),
            OsString::from(port.to_string()),
        ];
        // Trusted paired control, before the isolated launch; never a fallback path.
        let baseline = Command::new(&source)
            .arg("--child-canary")
            .args(&args)
            .env("DIYIME2_SECRET_CANARY", "fake-secret-canary")
            .output()
            .map_err(|e| e.to_string())?;
        if !baseline.status.success() {
            return Err("normal fixture did not run".into());
        }
        let baseline: Value =
            serde_json::from_slice(&baseline.stdout).map_err(|_| "normal fixture JSON invalid")?;
        for key in [
            "privateRead",
            "allowedRead",
            "allowedWrite",
            "networkConnected",
            "secretPresent",
            "descendantSucceeded",
        ] {
            if baseline[key] != true {
                return Err(format!("normal control failed: {key}: {baseline}"));
            }
        }
        if received(&listener)? != 1 {
            return Err("normal network control not received".into());
        }
        cache.allow_read_execute(&profile)?;
        let job = worker::Job::new()?;
        let reply = worker::run(
            &profile,
            &job,
            &cache,
            &exe,
            &[vec![OsString::from("--child-canary")], args].concat(),
            30_000,
        )?;
        if reply.exit_code != 0 {
            return Err(format!(
                "isolated fixture exit {}: {}",
                reply.exit_code,
                String::from_utf8_lossy(&reply.stdout)
            ));
        }
        let isolated: Value = serde_json::from_slice(&reply.stdout).map_err(|_| {
            format!(
                "isolated fixture JSON invalid: {}",
                String::from_utf8_lossy(&reply.stdout)
            )
        })?;
        if isolated["allowedRead"] != true {
            return Err(format!("authorized read failed: {isolated}"));
        }
        for key in [
            "privateRead",
            "allowedWrite",
            "networkConnected",
            "secretPresent",
            "descendantSucceeded",
        ] {
            if isolated[key] != false {
                return Err(format!("isolation boundary failed: {key}: {isolated}"));
            }
        }
        let until = Instant::now() + Duration::from_millis(200);
        while Instant::now() < until {
            if received(&listener)? != 0 {
                return Err("isolated network canary reached control receiver".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(job);
        profile.delete()?;
        Ok(
            json!({"kind":"appcontainer-canary","baseline":baseline,"isolated":isolated,"processId":reply.process_id,"physicalExit":true,"profileDeleted":true}),
        )
    }
    fn timeout_recovery() -> Result<Value, String> {
        let mut profile = worker::Profile::new()?;
        let cache = worker::Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
        let source = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = cache.copy(&source, "canary-worker.exe")?;
        cache.allow_read_execute(&profile)?;
        let job = worker::Job::new()?;
        let outside = worker::run(&profile, &job, &cache, &source, &[], 100);
        if !outside.is_err_and(|e| e == "Worker program is not a cache asset") {
            return Err("outside-cache executable was not refused".into());
        }
        let failed = worker::run(
            &profile,
            &job,
            &cache,
            &exe,
            &[OsString::from("--hang")],
            100,
        );
        if !failed
            .is_err_and(|e| e == "Worker timed out or wait failed; physical process terminated")
        {
            return Err("hung Worker physical exit was not confirmed".into());
        }
        drop(job);
        // A fresh Job can launch only after the previous process handle signalled.
        let job = worker::Job::new()?;
        let reply = worker::run(
            &profile,
            &job,
            &cache,
            &exe,
            &[OsString::from("--descendant")],
            30_000,
        )?;
        if reply.exit_code != 0
            || serde_json::from_slice::<Value>(&reply.stdout)
                .map_err(|_| "recovery JSON invalid")?["descendant"]
                != true
        {
            return Err("new isolated Worker failed after timeout".into());
        }
        drop(job);
        profile.delete()?;
        Ok(
            json!({"kind":"appcontainer-timeout-recovery","outsideCacheRefused":true,"timedOutPhysicalExit":true,"recoveryPhysicalExit":true,"processId":reply.process_id,"profileDeleted":true}),
        )
    }
    fn wasm(args: &[OsString]) -> Result<Value, String> {
        if args.len() != 4 {
            return Err("wasm requires probe.exe runtime.dll module.wasm request.json".into());
        }
        let mut profile = worker::Profile::new()?;
        let cache = worker::Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile)?;
        let exe = cache.copy(Path::new(&args[0]), "wasm-worker.exe")?;
        let dll = cache.copy(Path::new(&args[1]), "wasmtime.dll")?;
        let module_name = if Path::new(&args[2]).extension().is_some_and(|e| e == "wat") {
            "node.wat"
        } else {
            "node.wasm"
        };
        let module = cache.copy(Path::new(&args[2]), module_name)?;
        let request = cache.copy(Path::new(&args[3]), "request.json")?;
        cache.allow_read_execute(&profile)?;
        let job = worker::Job::new()?;
        let reply = worker::run(
            &profile,
            &job,
            &cache,
            &exe,
            &[
                dll.into_os_string(),
                module.into_os_string(),
                request.into_os_string(),
            ],
            30_000,
        )?;
        if reply.exit_code != 0 {
            return Err(format!(
                "isolated Wasm failed {}: {}",
                reply.exit_code,
                String::from_utf8_lossy(&reply.stdout)
            ));
        }
        let result: Value = serde_json::from_slice(&reply.stdout)
            .map_err(|_| "isolated Wasm reply JSON invalid")?;
        drop(job);
        profile.delete()?;
        Ok(
            json!({"kind":"appcontainer-wasm","processId":reply.process_id,"physicalExit":true,"profileDeleted":true,"result":result}),
        )
    }
    pub fn run() -> Result<Value, String> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        match args.first().and_then(|s| s.to_str()) {
            Some("--descendant") => Ok(json!({"descendant":true})),
            Some("--hang") => loop { std::thread::sleep(Duration::from_secs(1)); },
            Some("timeout") if args.len() == 1 => timeout_recovery(),
            Some("--child-canary") => child(&args[1..]),
            Some("canary") if args.len() == 1 => canary(),
            Some("wasm") => wasm(&args[1..]),
            _ => Err(
                "usage: appcontainer_worker_probe canary | timeout | wasm <probe> <dll> <wasm> <request>"
                    .into(),
            ),
        }
    }
}
fn main() {
    #[cfg(windows)]
    match fixture::run() {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("worker-probe: {error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Worker OS isolation unavailable; refusing launch");
        std::process::exit(1);
    }
}
