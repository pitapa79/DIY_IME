//! One stdio owner multiplexes independent Sessions on one existing Host.
//! The broker envelope is separate from its unmodified protocol-v3 payload.
use super::{
    DecimalId, ExecutionHost, ExecutionService, ResourcePolicy, SessionEpoch,
    protocol::{MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES},
    session::SessionV3,
    stream::{HostDrain, read_frame},
};
use crate::node::{CoreError, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const BROKER_VERSION: u32 = 1;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    broker: u32,
    id: Value,
    request: Value,
}
#[derive(Deserialize)]
#[serde(
    tag = "method",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    OpenSession {},
    Request {
        session_epoch: SessionEpoch,
        message: Value,
    },
    CloseSession {
        session_epoch: SessionEpoch,
    },
}
#[derive(Serialize)]
struct Reply<'a> {
    broker: u32,
    id: Option<DecimalId>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a CoreError>,
}
struct FrameWriter(Vec<u8>);
impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_RESPONSE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("broker reply exceeds 8 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(
    id: Option<DecimalId>,
    outcome: Result<&Value, &CoreError>,
) -> Result<Vec<u8>, CoreError> {
    let reply = Reply {
        broker: BROKER_VERSION,
        id,
        ok: outcome.is_ok(),
        data: outcome.as_ref().ok().copied(),
        error: outcome.err(),
    };
    let mut writer = FrameWriter(Vec::new());
    serde_json::to_writer(&mut writer, &reply)
        .map_err(|e| CoreError::new("broker-response", e.to_string()))?;
    Ok(writer.0)
}
/// This owns only its own Session handles. Other brokers/services keep their
/// Sessions, clock, physical work and reservations when this broker is dropped.
pub struct Broker {
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
    service: ExecutionService,
    registry: Arc<Registry>,
    sessions: BTreeMap<SessionEpoch, SessionV3>,
}
impl Broker {
    pub fn new(service: &ExecutionService, registry: Registry) -> Self {
        Self::with_resource_root(service, registry, None)
    }
    pub fn with_resource_root(
        service: &ExecutionService,
        registry: Registry,
        resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
    ) -> Self {
        Self {
            resource_root,
            service: service.clone(),
            registry: Arc::new(registry),
            sessions: BTreeMap::new(),
        }
    }
    /// A bounded complete broker frame in/out, with no line delimiters.
    /// Faults producing a complete reply close the owning transport; no partial
    /// success, implicit Session reopening or automatic replay is performed.
    pub fn reply(&mut self, frame: &[u8]) -> Result<Vec<u8>, CoreError> {
        if frame.len() > MAX_REQUEST_BYTES {
            return encode(
                None,
                Err(&CoreError::new(
                    "request-limit",
                    "broker request exceeds 1 MiB",
                )),
            );
        }
        let raw: Envelope = match serde_json::from_slice(frame) {
            Ok(raw) => raw,
            Err(error) => {
                return encode(
                    None,
                    Err(&CoreError::new("broker-envelope", error.to_string())),
                );
            }
        };
        let id = serde_json::from_value::<DecimalId>(raw.id);
        if raw.broker != BROKER_VERSION {
            return encode(
                id.ok(),
                Err(&CoreError::new(
                    "broker-version",
                    "expected broker envelope 1",
                )),
            );
        }
        let id = match id {
            Ok(id) => id,
            Err(error) => {
                return encode(
                    None,
                    Err(&CoreError::new("broker-envelope", error.to_string())),
                );
            }
        };
        let outcome = serde_json::from_value::<Operation>(raw.request)
            .map_err(|error| CoreError::new("broker-request", error.to_string()))
            .and_then(|request| self.handle(request));
        if let Err(error) = &outcome
            && matches!(
                error.code.as_str(),
                "service-fault"
                    | "service-closed"
                    | "service-unavailable"
                    | "protocol-response"
                    | "response-limit"
            )
        {
            return Err(error.clone());
        }
        encode(Some(id), outcome.as_ref())
    }
    fn handle(&mut self, request: Operation) -> Result<Value, CoreError> {
        match request {
            Operation::OpenSession {} => {
                let session = SessionV3::with_registry_and_resource_root(
                    &self.service,
                    Arc::clone(&self.registry),
                    self.resource_root.clone(),
                )?;
                let epoch = session.session_epoch().clone();
                self.sessions.insert(epoch.clone(), session);
                Ok(
                    json!({"sessionEpoch":epoch,"resourceDomain":self.service.resource_domain(),"resourcePolicy":self.service.resource_policy()}),
                )
            }
            Operation::Request {
                session_epoch,
                message,
            } => {
                let session = self.sessions.get_mut(&session_epoch).ok_or_else(|| {
                    CoreError::new(
                        "broker-session-unknown",
                        "Session is closed or not owned by this broker",
                    )
                })?;
                let frame = serde_json::to_vec(&message)
                    .map_err(|error| CoreError::new("broker-request", error.to_string()))?;
                let reply = session.reply(&frame)?;
                serde_json::from_slice(&reply)
                    .map_err(|error| CoreError::new("protocol-response", error.to_string()))
            }
            Operation::CloseSession { session_epoch } => {
                self.sessions.remove(&session_epoch).ok_or_else(|| {
                    CoreError::new(
                        "broker-session-unknown",
                        "Session is closed or not owned by this broker",
                    )
                })?;
                Ok(json!({"closed":true}))
            }
        }
    }
}
/// Borrow the shared Host. EOF or I/O failure closes only this transport's
/// Sessions, not Sessions on another broker or live work still owned by service.
pub fn serve(
    service: &ExecutionService,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
) -> io::Result<()> {
    serve_with_resource_root(service, registry, reader, writer, None)
}
pub fn serve_with_resource_root(
    service: &ExecutionService,
    registry: Registry,
    mut reader: impl BufRead,
    mut writer: impl Write,
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
) -> io::Result<()> {
    let mut broker = Broker::with_resource_root(service, registry, resource_root);
    while let Some(frame) = read_frame(&mut reader)? {
        let oversized = frame.len() > MAX_REQUEST_BYTES;
        let reply = broker.reply(&frame).map_err(io::Error::other)?;
        writer.write_all(&reply)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        if oversized {
            return Ok(());
        }
    }
    Ok(())
}
/// One explicit process root for every multiplexed Session. On stream loss,
/// close the Sessions first, then retain the Host until genuine physical ending.
pub fn serve_host(
    policy: ResourcePolicy,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
) -> io::Result<()> {
    serve_owned_host(policy, registry, reader, writer, None, None)
}
/// A bridge replacement must use the same canonical ownership path. The Core,
/// not its parent, holds the file through HostDrain. Stale files fail closed;
/// they are never treated as proof that a producer ended or removed on startup.
pub fn serve_host_locked(
    policy: ResourcePolicy,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
    ownership_path: &Path,
) -> io::Result<()> {
    serve_owned_host(policy, registry, reader, writer, Some(ownership_path), None)
}
struct HostOwnership {
    path: PathBuf,
    file: Option<File>,
}
impl HostOwnership {
    fn claim(path: &Path, domain: &str) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Allow inspection, but not replacement/deletion while this Core owns it.
            options.share_mode(1);
        }
        let file = options.open(path)?;
        let mut owner = Self {
            path: path.to_owned(),
            file: Some(file),
        };
        let metadata = json!({"pid":std::process::id(),"resourceDomain":domain});
        serde_json::to_writer(owner.file.as_mut().expect("owned file"), &metadata)?;
        owner.file.as_mut().expect("owned file").flush()?;
        Ok(owner)
    }
}
impl Drop for HostOwnership {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
/// Explicit Host resource root; no client request can change it.
pub fn serve_host_with_resource_root(
    policy: ResourcePolicy,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
    ownership_path: Option<&Path>,
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
) -> io::Result<()> {
    serve_owned_host(
        policy,
        registry,
        reader,
        writer,
        ownership_path,
        resource_root,
    )
}
fn serve_owned_host(
    policy: ResourcePolicy,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
    ownership_path: Option<&Path>,
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
) -> io::Result<()> {
    policy.validate().map_err(io::Error::other)?;
    let domain = format!(
        "host-{}",
        super::epoch::generate().map_err(io::Error::other)?.as_str()
    );
    // Declared before HostDrain so ownership is released only after it ends.
    let _ownership = ownership_path
        .map(|path| HostOwnership::claim(path, &domain))
        .transpose()?;
    let host = ExecutionHost::new(domain, policy).map_err(io::Error::other)?;
    let _drain = HostDrain(host.clone());
    let service = ExecutionService::new(host).map_err(io::Error::other)?;
    serve_with_resource_root(&service, registry, reader, writer, resource_root)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_limit_includes_the_broker_wrapper_and_never_publishes_a_prefix() {
        let id = Some(DecimalId::new(1).unwrap());
        let overhead = encode(id, Ok(&Value::String(String::new()))).unwrap().len();
        let exact = Value::String("x".repeat(MAX_RESPONSE_BYTES - overhead));
        assert_eq!(encode(id, Ok(&exact)).unwrap().len(), MAX_RESPONSE_BYTES);
        let large = Value::String("x".repeat(MAX_RESPONSE_BYTES - overhead + 1));
        assert_eq!(
            encode(Some(DecimalId::new(1).unwrap()), Ok(&large))
                .unwrap_err()
                .code,
            "broker-response"
        );
        let small = json!({"protocol":3,"id":"18446744073709551615","ok":true});
        let bytes = encode(Some(DecimalId::new(u64::MAX).unwrap()), Ok(&small)).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["data"],
            small
        );
    }
}
