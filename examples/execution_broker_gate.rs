//! Integration-test host. This example registers only a gated test producer;
//! it is not a product node or a replacement registry for the editor.
use diy_ime2_core::{
    execution_control::{
        CancellationToken, CompletionPermit, DeferredNode, OwnedInvocation, ResourcePolicy, broker,
    },
    node::{CoreError, NodeDefinition, Port, Registry},
};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};
struct Held(PathBuf);
impl DeferredNode for Held {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.http-held", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        _: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let mark = |name: &str| {
            fs::write(self.0.join(name), b"1").map_err(|e| CoreError::new("test-io", e.to_string()))
        };
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.0.join("starts.log"))
            .and_then(|mut file| file.write_all(b"start\n"))
            .map_err(|e| CoreError::new("test-io", e.to_string()))?;
        mark("started")?;
        while !self.0.join("producer-end").exists() {
            if cancel.is_cancelled() {
                mark("cancelled")?;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        permit.finish_cancelled();
        mark("producer-ended")?;
        while !self.0.join("callback-return").exists() {
            if cancel.is_cancelled() {
                mark("cancelled")?;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}
struct GateWriter<W> {
    inner: W,
    dir: PathBuf,
    bytes: Vec<u8>,
}
impl<W: Write> Write for GateWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.dir.join("hold-replies").exists() {
            fs::write(self.dir.join("reply-held"), b"1")?;
            while self.dir.join("hold-replies").exists() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.inner.write_all(&self.bytes)?;
        self.bytes.clear();
        self.inner.flush()
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [command, policy, ownership] = args.as_slice() else {
        return Err("expected broker-v3 <policy> <owner-lock>".into());
    };
    if command != "broker-v3" {
        return Err("expected broker-v3".into());
    }
    let dir = PathBuf::from(std::env::var("DIY_IME_BROKER_TEST_GATE")?);
    let policy = ResourcePolicy::from_value(serde_json::from_str(&fs::read_to_string(policy)?)?)?;
    let mut registry = Registry::new();
    registry.register_deferred(Held(dir.clone()))?;
    broker::serve_host_locked(
        policy,
        registry,
        io::stdin().lock(),
        GateWriter {
            inner: io::stdout().lock(),
            dir,
            bytes: Vec::new(),
        },
        std::path::Path::new(ownership),
    )?;
    Ok(())
}
