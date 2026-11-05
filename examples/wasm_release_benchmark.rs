//! Host-only phase timing of actual fresh isolated calls; synthetic public configs only.
#[cfg(all(windows, target_arch = "x86_64"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use diy_ime2_core::{
        execution_control::CancellationToken,
        isolated_worker::adapter::{ExecutionGate, WasmArtifact, WorkerPolicy},
    };
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 5 && !(args.len() == 6 && args[5] == "--parallel") {
        return Err("usage: WORKER DLL WORKSPACE MODULE KIND [--parallel]".into());
    }
    let workspace = Path::new(&args[2]);
    std::fs::create_dir_all(workspace)?;
    let module = std::fs::read(&args[3])?;
    let module_bytes = module.len();
    let policy = WorkerPolicy {
        process_memory_bytes: Some(512 * 1024 * 1024),
        wasm_memory_bytes: Some(128 * 1024 * 1024),
        fuel_per_call: Some(50_000_000),
        walltime: Some(Duration::from_secs(30)),
        termination_grace: Duration::from_millis(5),
    };
    let start = Instant::now();
    let artifact = WasmArtifact::capture(
        workspace,
        Path::new(&args[0]),
        Path::new(&args[1]),
        module,
        policy,
    )?;
    let capture = start.elapsed().as_micros();
    let kind = args[4].to_str().ok_or("kind UTF8")?;
    let start = Instant::now();
    let parallel = args.len() == 6;
    let prepare = || -> Result<u128, diy_ime2_core::node::CoreError> {
        let started = Instant::now();
        let node = artifact.prepare(
            kind,
            1,
            vec![],
            ExecutionGate::default(),
            CancellationToken::default(),
        )?;
        drop(node);
        Ok(started.elapsed().as_micros())
    };
    let calls = if parallel {
        std::thread::scope(|scope| {
            let first = scope.spawn(|| (0..2).map(|_| prepare()).collect::<Result<Vec<_>, _>>());
            let second = scope.spawn(|| (0..2).map(|_| prepare()).collect::<Result<Vec<_>, _>>());
            let mut values = first.join().unwrap()?;
            values.extend(second.join().unwrap()?);
            Ok::<_, diy_ime2_core::node::CoreError>(values)
        })?
    } else {
        (0..4).map(|_| prepare()).collect::<Result<Vec<_>, _>>()?
    };
    assert_eq!(
        artifact.active_processes(),
        0,
        "all actual process owners ended"
    );
    println!(
        "{}",
        serde_json::json!({"kind":"isolated-phase-benchmark","moduleBytes":module_bytes,"captureMicros":capture,"totalMicros":start.elapsed().as_micros(),"parallel":parallel,"prepareMicros":calls,"requests":artifact.request_count(),"timings":artifact.timings()})
    );
    Ok(())
}
#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("Windows x64 required");
    std::process::exit(1);
}
