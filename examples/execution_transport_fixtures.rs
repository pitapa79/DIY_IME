//! Test-only transport fixture Host. Clock controls are trusted files outside
//! the product wire; all replies come from the real v2/v3 implementations.
#[allow(dead_code)]
#[path = "../tests/support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    error_input::{ErrorAwareExecution, ErrorAwareInputs, ErrorAwareNode},
    execution_control::{
        CancellationToken, CompletionPermit, DeferredNode, ExecutionClock, ExecutionHost,
        ExecutionService, OwnedInvocation, ResourcePolicy, broker,
    },
    node::{CoreError, NodeDefinition, Port, Registry, RunContext},
    session::{self, Session},
};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use test_nodes::register_builtin_nodes;
struct Held {
    directory: PathBuf,
    clock: ExecutionClock,
}
impl DeferredNode for Held {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.fixture-held", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        invocation: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let record = json!({"key":invocation.key(),"nodeId":invocation.context().node_id,"input":invocation.context().input,"clockMs":self.clock.now()?.as_millis()});
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.directory.join("starts.jsonl"))
            .map_err(test_error)?;
        serde_json::to_writer(&mut file, &record).map_err(test_error)?;
        writeln!(file).map_err(test_error)?;
        file.flush().map_err(test_error)?;
        while !self.directory.join("release-producer").exists() {
            if cancel.is_cancelled() {
                fs::write(self.directory.join("cancelled"), b"1").map_err(test_error)?;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // Cleanup may release the producer before the close command arrives.
        // A non-cancelled producer must publish its actual completion too.
        if !permit.finish_cancelled() {
            permit.complete(Err(CoreError::new(
                "fixture-ended",
                "test producer released",
            )));
        }
        Ok(())
    }
}
fn test_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::new("fixture-io", error.to_string())
}
struct Unsupported(PathBuf);
impl ErrorAwareNode for Unsupported {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "test.fixture-error-aware",
            1,
            vec![],
            vec![Port::packet("out")],
        )
    }
    fn error_inputs(&self) -> Vec<String> {
        vec![]
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        _: &ErrorAwareInputs,
        _: &Value,
    ) -> Result<ErrorAwareExecution, CoreError> {
        fs::write(self.0.join("error-aware-executed"), b"1").map_err(test_error)?;
        Err(CoreError::new(
            "fixture-unexpected-execution",
            "v2 must refuse before execution",
        ))
    }
}
struct Ownership {
    file: Option<File>,
    path: PathBuf,
}
impl Ownership {
    fn claim(path: PathBuf, domain: &str) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let file = options.open(&path)?;
        let mut owner = Self {
            file: Some(file),
            path,
        };
        serde_json::to_writer(
            owner.file.as_mut().unwrap(),
            &json!({"pid":std::process::id(),"resourceDomain":domain}),
        )?;
        owner.file.as_mut().unwrap().flush()?;
        Ok(owner)
    }
}
impl Drop for Ownership {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
struct ClockOwner {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl ClockOwner {
    fn start(clock: ExecutionClock, directory: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut previous = None;
            while !stopping.load(Ordering::Acquire) {
                if let Ok(text) = fs::read_to_string(directory.join("clock-request"))
                    && let Ok(ms) = text.trim().parse::<u64>()
                    && previous != Some(ms)
                {
                    match clock.advance_to(Duration::from_millis(ms)) {
                        Ok(()) => {
                            let observed = clock.now().unwrap().as_millis();
                            fs::write(
                                directory.join("clock-ack"),
                                json!({"requested":ms,"observed":observed}).to_string(),
                            )
                            .unwrap();
                            previous = Some(ms);
                        }
                        Err(error) => {
                            fs::write(directory.join("clock-error"), error.to_string()).unwrap();
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for ClockOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(std::env::var("DIY_IME_TRANSPORT_FIXTURE_DIR")?);
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["session"] {
        let mut registry = Registry::new();
        register_builtin_nodes(&mut registry)?;
        registry.register_error_aware(Unsupported(directory))?;
        session::serve_session(
            Session::new(registry),
            io::stdin().lock(),
            io::stdout().lock(),
        )?;
        return Ok(());
    }
    let [command, policy, ownership] = args.as_slice() else {
        return Err("expected broker-v3 <policy> <ownership> or session".into());
    };
    if command != "broker-v3" {
        return Err("expected broker-v3".into());
    }
    let policy = ResourcePolicy::from_value(serde_json::from_str(&fs::read_to_string(policy)?)?)?;
    let domain = format!("fixture-host-{}", std::process::id());
    let _ownership = Ownership::claim(PathBuf::from(ownership), &domain)?;
    let host = ExecutionHost::new(domain, policy)?;
    let clock = ExecutionClock::controlled(Duration::ZERO);
    let _clock = ClockOwner::start(clock.clone(), directory.clone());
    let service = ExecutionService::with_clock(host.clone(), clock.clone())?;
    let mut registry = Registry::new();
    registry.register_deferred(Held {
        directory: directory.clone(),
        clock,
    })?;
    let result = broker::serve(&service, registry, io::stdin().lock(), io::stdout().lock());
    drop(service);
    loop {
        let usage = host.usage();
        if usage.physical_jobs == 0
            && usage.active_runs == 0
            && usage.queued_jobs == 0
            && usage.observers == 0
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    result?;
    Ok(())
}
