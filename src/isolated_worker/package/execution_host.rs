//! Production pure-Wasm execution. Assets, roots and authority come from the Host.
use super::{
    Catalog, Package, PackageVerifier, VerifiedPackage,
    manager::{ManagerConfig, nonce},
    permissions::{
        PendingPermissionSession, PermissionBook, PermissionLifetime, PermissionPlan,
        PermissionSession,
    },
    secrets::{SecretValue, SecretVault, SensitiveFrame},
};
use crate::{
    execution_control::{
        CancellationToken, DecimalId, ExecutionHost, ExecutionService, ResourcePolicy, RunKey,
        StartRequest,
    },
    graph::Graph,
    isolated_worker::adapter::ExecutionGate,
    node::{CoreError, Registry},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
fn error(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
fn default_permission_bytes() -> usize {
    16 * 1024 * 1024
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    package_manager: ManagerConfig,
    permission_root: PathBuf,
    #[serde(default)]
    secret_root: Option<PathBuf>,
    #[serde(default)]
    network: Option<crate::isolated_worker::adapter::NetworkConfiguration>,
    resource_policy: Value,
    request_bytes: usize,
    response_bytes: usize,
    #[serde(default = "default_permission_bytes")]
    permission_bytes: usize,
}
impl Configuration {
    pub fn read(path: &Path) -> Result<Self, CoreError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| error("external-config", "Host configuration unavailable"))?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| error("external-config", "Host configuration read failed"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(error("external-config", "Host configuration too large"));
        }
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| error("external-config", "invalid Host configuration"))?;
        if config.request_bytes == 0
            || config.response_bytes < 1024
            || config.request_bytes > u32::MAX as usize
        {
            return Err(error(
                "external-config",
                "invalid selected transport limits",
            ));
        }
        Ok(config)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    TraceStart {},
    TraceStartDetailed {},
    TraceRead {
        after: String,
        limit: usize,
    },
    TraceStop {},
    TraceClear {},
    Definitions {},
    Status {},
    Saved {},
    Secrets {},
    SecretPut {
        id: String,
        value: SecretValue,
    },
    SecretDelete {
        id: String,
    },
    Prepare {
        graph: Graph,
    },
    Approve {
        review_id: String,
        ids: Vec<String>,
        save: bool,
    },
    Activate {
        review_id: String,
    },
    Run {
        request: StartRequest,
    },
    GetRun {
        key: RunKey,
    },
    CancelRun {
        key: RunKey,
    },
    GetStart {
        start_key: DecimalId,
    },
    InvalidateInput {
        expected_plan_version: DecimalId,
        revision: u64,
    },
    CloseSession {},
    Cancel {
        request_id: String,
    },
    Protection {
        enabled: bool,
    },
    Revoke {
        id: String,
    },
    Close {},
}
type Output = Arc<Mutex<io::Stdout>>;
fn reply(output: &Output, id: &str, result: Result<Value, CoreError>, limit: usize) {
    let _ = publish(output, id, result, limit, None);
}
fn publish(
    output: &Output,
    id: &str,
    result: Result<Value, CoreError>,
    limit: usize,
    scope: Option<(&Controls, &Arc<()>, &CancellationToken)>,
) -> bool {
    let mut out = output.lock().unwrap_or_else(|p| p.into_inner());
    let result = if scope.is_some_and(|(controls, generation, cancel)| {
        controls.closing.load(Ordering::Acquire)
            || cancel.is_cancelled()
            || !Arc::ptr_eq(
                generation,
                &controls
                    .generation
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()),
            )
    }) {
        Err(error(
            "external-context-changed",
            "response withdrawn by protection/revocation",
        ))
    } else {
        result
    };
    let value = match result {
        Ok(data) => json!({"id":id,"ok":true,"data":data}),
        Err(e) => json!({"id":id,"ok":false,"error":{"code":e.code,"message":e.message}}),
    };
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    let public = value["ok"] == true && bytes.len() <= limit;
    let bytes = if bytes.len() > limit {
        serde_json::to_vec(&json!({"id":id,"ok":false,"error":{"code":"external-response-limit","message":"response exceeds selected Host limit"}})).unwrap()
    } else {
        bytes
    };
    let written = out.write_all(&bytes).is_ok();
    let newline = out.write_all(b"\n").is_ok();
    let flushed = out.flush().is_ok();
    public && written && newline && flushed
}
#[derive(Default)]
struct Activity {
    active: Option<(String, CancellationToken, bool)>,
}
fn cancel(activity: &Mutex<Activity>) {
    if let Some((_, token, _)) = &activity.lock().unwrap_or_else(|p| p.into_inner()).active {
        token.cancel();
    }
}
struct Work {
    generation: Arc<()>,
    request: Request,
    cancel: CancellationToken,
}
struct Controls {
    generation: Mutex<Arc<()>>,
    book: PermissionBook,
    activity: Mutex<Activity>,
    closing: AtomicBool,
    manual: AtomicBool,
}
fn reader(
    controls: Arc<Controls>,
    output: Output,
    sender: mpsc::SyncSender<Work>,
    request_limit: usize,
    response_limit: usize,
) {
    let mut input = io::stdin().lock();
    loop {
        let mut header = [0u8; 12];
        if input.read_exact(&mut header).is_err() {
            break;
        }
        let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        if length == 0
            || length > request_limit
            || u64::from_le_bytes(header[4..].try_into().unwrap()) != 0
        {
            reply(
                &output,
                "",
                Err(error("external-frame", "invalid request framing")),
                response_limit,
            );
            break;
        }
        let mut bytes = SensitiveFrame(vec![0; length]);
        if input.read_exact(&mut bytes.0).is_err() {
            break;
        }
        // Decode only the correlation header; do not clone plaintext secret
        // fields into a generic JSON Value before strict operation decoding.
        #[derive(Deserialize)]
        struct Header {
            id: String,
        }
        let raw: Header = match serde_json::from_slice(&bytes.0) {
            Ok(value) => value,
            Err(_) => {
                reply(
                    &output,
                    "",
                    Err(error("external-request", "invalid JSON request")),
                    response_limit,
                );
                break;
            }
        };
        let Some(id) = Some(raw.id.as_str()).filter(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }) else {
            reply(
                &output,
                "",
                Err(error("external-request", "invalid request ID")),
                response_limit,
            );
            break;
        };
        let request: Request = match serde_json::from_slice(&bytes.0) {
            Ok(request) => request,
            Err(_) => {
                reply(
                    &output,
                    id,
                    Err(error("external-request", "invalid request fields")),
                    response_limit,
                );
                continue;
            }
        };
        let fast = match &request.operation {
            Operation::Cancel { request_id } => {
                let state = controls.activity.lock().unwrap_or_else(|p| p.into_inner());
                if let Some((id, token, preparing)) = &state.active
                    && id == request_id
                {
                    token.cancel();
                    if *preparing {
                        *controls
                            .generation
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = Arc::new(());
                    }
                }
                Some(Ok(json!({"cancelRequested":true})))
            }
            Operation::Protection { enabled } => {
                *controls
                    .generation
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Arc::new(());
                controls.manual.store(*enabled, Ordering::Release);
                cancel(&controls.activity);
                controls.book.set_protection(*enabled, false);
                Some(Ok(json!({"protected":enabled})))
            }
            Operation::Revoke { id } => {
                *controls
                    .generation
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Arc::new(());
                cancel(&controls.activity);
                Some(controls.book.revoke_id(id).map(|_| json!({"revoked":true})))
            }
            Operation::Close {} => break,
            _ => None,
        };
        if let Some(result) = fast {
            reply(&output, &request.id, result, response_limit);
            continue;
        }
        let mut state = controls.activity.lock().unwrap_or_else(|p| p.into_inner());
        if state.active.is_some() {
            drop(state);
            reply(
                &output,
                &request.id,
                Err(error("external-busy", "another Host operation is settling")),
                response_limit,
            );
            continue;
        }
        let token = CancellationToken::default();
        state.active = Some((
            request.id.clone(),
            token.clone(),
            matches!(
                request.operation,
                Operation::Prepare { .. } | Operation::Activate { .. }
            ),
        ));
        drop(state);
        let generation = controls
            .generation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if sender
            .send(Work {
                generation,
                request,
                cancel: token,
            })
            .is_err()
        {
            return;
        }
    }
    controls.closing.store(true, Ordering::Release);
    cancel(&controls.activity);
    controls.book.set_protection(true, true);
    *controls
        .generation
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Arc::new(());
    let generation = controls
        .generation
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let _ = sender.send(Work {
        generation,
        request: Request {
            id: String::new(),
            operation: Operation::Close {},
        },
        cancel: CancellationToken::default(),
    });
}
struct Runner {
    catalog: Catalog,
    verifier: PackageVerifier,
    vault: Option<SecretVault>,
    book: PermissionBook,
    controls: Arc<Controls>,
    host: ExecutionHost,
    service: ExecutionService,
    gate: ExecutionGate,
    pending: Option<(String, PendingPermissionSession)>,
    session: Option<PermissionSession>,
    used: BTreeMap<String, String>,
    context: Option<Arc<()>>,
    activities: Vec<super::super::adapter::WasmNode>,
}
impl Runner {
    fn reconcile(&mut self) -> Result<(), CoreError> {
        if self.context.as_ref().is_some_and(|g| {
            !Arc::ptr_eq(
                g,
                &self
                    .controls
                    .generation
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()),
            )
        }) {
            self.pending.take();
            self.session.take();
            self.used.clear();
            self.context = None;
        }
        let snapshot = match self.catalog.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.pending.take();
                self.session.take();
                self.used.clear();
                return Err(error);
            }
        };
        if self.used.iter().any(|(id, hash)| {
            !snapshot
                .entries
                .iter()
                .any(|e| &e.manifest.package_id == id && &e.authorization_sha256 == hash)
        }) {
            self.pending.take();
            self.session.take();
            self.used.clear();
        }
        if self.pending.as_ref().is_some_and(|(_, p)| !p.is_current()) {
            self.pending.take();
        }
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.controller().control_state().is_err())
        {
            self.session.take();
        }
        Ok(())
    }
    fn stop(&mut self) {
        self.pending.take();
        self.session.take();
        self.used.clear();
        while {
            let u = self.host.usage();
            u.physical_jobs != 0 || u.queued_jobs != 0 || u.active_runs != 0
        } {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.activities.clear();
        self.context = None;
    }
    fn status(&self) -> Result<Value, CoreError> {
        let protected = self.controls.manual.load(Ordering::Acquire);
        Ok(
            json!({"phase":if protected{"protected"}else if self.session.is_some(){"ready"}else if self.pending.is_some(){"review"}else{"empty"},"protected":protected,
   "sessionEpoch":self.session.as_ref().map(|s|s.session_epoch()).or_else(||self.pending.as_ref().map(|(_,p)|p.session_epoch())),
   "state":self.session.as_ref().map(|s|s.controller().control_state().and_then(|state|crate::execution_control::protocol::WireState::from_control(state,None))).transpose()?,
   "reviewId":self.pending.as_ref().map(|(id,_)|id),"physical":self.host.usage(),"workerProcesses":self.activities.iter().map(|n|n.active_processes()).sum::<u64>()}),
        )
    }
    fn prepare(&mut self, graph: Graph, cancel: CancellationToken) -> Result<Value, CoreError> {
        let generation = self
            .controls
            .generation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        self.stop();
        if cancel.is_cancelled() {
            return Err(error("external-cancelled", "preparation cancelled"));
        }
        if self.context.as_ref().is_some_and(|g| {
            !Arc::ptr_eq(
                g,
                &self
                    .controls
                    .generation
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()),
            )
        }) {
            self.pending.take();
            self.session.take();
            self.used.clear();
            self.context = None;
        }
        let snapshot = match self.catalog.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.pending.take();
                self.session.take();
                self.used.clear();
                return Err(error);
            }
        };
        let mut grouped: BTreeMap<String, (Package, Vec<Value>)> = BTreeMap::new();
        for node in &graph.nodes {
            if matches!(
                node.kind.as_str(),
                "input.text" | "demo.packet" | "e2e.input" | "e2e.output"
            ) {
                continue;
            }
            if !grouped.contains_key(&node.kind) {
                let entry = snapshot
                    .entries
                    .iter()
                    .find(|e| e.manifest.node.kind == node.kind)
                    .ok_or_else(|| {
                        error("external-package-missing", "graph Node is not installed")
                    })?;
                grouped.insert(
                    node.kind.clone(),
                    (
                        self.catalog.read_installed(&entry.manifest.package_id)?,
                        vec![],
                    ),
                );
            }
            let (package, configs) = grouped.get_mut(&node.kind).unwrap();
            if package.manifest().node.kind != node.kind
                || package.manifest().node.version != node.version
            {
                return Err(error(
                    "external-package-version",
                    "installed definition differs from graph",
                ));
            }
            configs.push(
                node.config
                    .clone()
                    .unwrap_or_else(|| package.manifest().node.default_config.clone()),
            );
        }
        let mut verified = BTreeMap::<String, VerifiedPackage>::new();
        for (kind, (package, configs)) in grouped {
            let proof = self.verifier.verify_configs_with_vault(
                package,
                configs,
                self.gate.clone(),
                cancel.clone(),
                self.vault.as_ref(),
            )?;
            verified.insert(kind, proof);
        }
        let external = graph
            .nodes
            .iter()
            .filter_map(|n| verified.get(&n.kind).map(|p| (n.id.clone(), p.clone())))
            .collect();
        let plan = PermissionPlan::capture_with_cancel(&graph, &external, cancel.clone())?;
        if cancel.is_cancelled() {
            return Err(error("external-cancelled", "preparation cancelled"));
        }
        let used = verified
            .values()
            .map(|p| {
                (
                    p.package().manifest().package_id.clone(),
                    p.package().authorization_digest().to_string(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let current = self.catalog.snapshot()?;
        if used.iter().any(|(id, hash)| {
            !current
                .entries
                .iter()
                .any(|e| &e.manifest.package_id == id && &e.authorization_sha256 == hash)
        }) {
            return Err(error(
                "external-catalog-changed",
                "installed implementations changed during preparation",
            ));
        }
        let pending = self
            .book
            .open_session(&self.service, plan, DecimalId::new(1)?)?;
        let review_id = nonce()?;
        let value = json!({"reviewId":review_id,"sessionEpoch":pending.session_epoch(),"requests":pending.requests(),"missing":pending.missing()});
        self.context = Some(generation);
        self.activities = verified.values().map(|p| p.node()).collect();
        self.pending = Some((review_id, pending));
        self.used = used;
        Ok(value)
    }
    fn controller(&self) -> Result<&super::super::adapter::GuardedController, CoreError> {
        self.session
            .as_ref()
            .map(|s| s.controller())
            .ok_or_else(|| error("external-not-ready", "approved Session required"))
    }
    fn handle(
        &mut self,
        operation: Operation,
        cancel: CancellationToken,
    ) -> Result<Value, CoreError> {
        if !matches!(
            operation,
            Operation::Secrets {} | Operation::SecretPut { .. } | Operation::SecretDelete { .. }
        ) {
            self.reconcile()?;
        }
        if cancel.is_cancelled() {
            return Err(error("external-cancelled", "Host operation cancelled"));
        }
        match operation {
            Operation::TraceStart {} => {
                self.controller()?.start_trace()?;
                Ok(json!({"trace":self.controller()?.read_trace(0,256)?}))
            }
            Operation::TraceStartDetailed {} => {
                self.controller()?.start_detailed_trace()?;
                Ok(json!({"trace":self.controller()?.read_trace(0,256)?}))
            }
            Operation::TraceRead { after, limit } => {
                if after.is_empty()
                    || after.len() > 20
                    || after.starts_with('0') && after != "0"
                    || !after.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(error("external-trace", "invalid diagnostic cursor"));
                }
                let after = after
                    .parse::<u64>()
                    .map_err(|_| error("external-trace", "invalid diagnostic cursor"))?;
                Ok(json!({"trace":self.controller()?.read_trace(after,limit)?}))
            }
            Operation::TraceStop {} => {
                self.controller()?.stop_trace()?;
                Ok(json!({"trace":self.controller()?.read_trace(0,256)?}))
            }
            Operation::TraceClear {} => {
                self.controller()?.clear_trace()?;
                Ok(json!({"trace":self.controller()?.read_trace(0,256)?}))
            }
            Operation::Definitions {} => {
                let mut registry = Registry::new();
                crate::builtins::register_system_nodes(&mut registry)?;
                let mut definitions: Vec<_> = registry
                    .execution_definitions()
                    .map(|d| d.definition)
                    .collect();
                definitions.extend(
                    self.catalog
                        .snapshot()?
                        .entries
                        .into_iter()
                        .map(|e| e.manifest.node),
                );
                Ok(json!({"definitions":definitions,"state":self.status()?}))
            }
            Operation::Status {} => self.status(),
            Operation::Saved {} => Ok(json!({"ids":self.book.saved_ids()})),
            Operation::Secrets {} => Ok(
                json!({"available":self.vault.is_some(),"entries":self.vault.as_ref().map(|v|v.list()).unwrap_or_default()}),
            ),
            Operation::SecretPut { id, value } => {
                self.stop();
                if cancel.is_cancelled() {
                    return Err(error("external-cancelled", "Host operation cancelled"));
                }
                let entry = self
                    .vault
                    .as_mut()
                    .ok_or_else(|| {
                        error(
                            "secret-unavailable",
                            "fixed Host secret store is not configured",
                        )
                    })?
                    .put(&id, value)?;
                Ok(json!({"entry":entry,"state":self.status()?}))
            }
            Operation::SecretDelete { id } => {
                self.stop();
                if cancel.is_cancelled() {
                    return Err(error("external-cancelled", "Host operation cancelled"));
                }
                self.vault
                    .as_mut()
                    .ok_or_else(|| {
                        error(
                            "secret-unavailable",
                            "fixed Host secret store is not configured",
                        )
                    })?
                    .remove(&id)?;
                Ok(json!({"deleted":true,"state":self.status()?}))
            }
            Operation::Prepare { graph } => self.prepare(graph, cancel),
            Operation::Approve {
                review_id,
                ids,
                save,
            } => {
                let (_, pending) = self
                    .pending
                    .as_ref()
                    .filter(|(id, _)| id == &review_id)
                    .ok_or_else(|| error("external-review", "current Host review required"))?;
                pending.approve(
                    &ids,
                    if save {
                        PermissionLifetime::Saved
                    } else {
                        PermissionLifetime::ThisSession
                    },
                )?;
                Ok(json!({"reviewId":review_id,"missing":pending.missing()}))
            }
            Operation::Activate { review_id } => {
                if self.pending.as_ref().is_none_or(|(id, _)| id != &review_id) {
                    return Err(error("external-review", "current Host review required"));
                }
                let (_, pending) = self.pending.take().unwrap();
                self.session = Some(pending.activate_with_cancel(cancel)?);
                self.status()
            }
            Operation::Run { request } => {
                let receipt = self.controller()?.start_run(request)?;
                Ok(json!({"receipt":receipt,"state":self.status()?}))
            }
            Operation::GetRun { key } => {
                let mut snapshot = serde_json::to_value(self.controller()?.get_run(&key)?)
                    .map_err(|_| error("external-result", "snapshot encoding failed"))?;
                if snapshot["current"] == false {
                    snapshot["result"] = Value::Null;
                }
                Ok(json!({"snapshot":snapshot,"state":self.status()?}))
            }
            Operation::CancelRun { key } => {
                let mut snapshot = serde_json::to_value(self.controller()?.cancel_run(&key)?)
                    .map_err(|_| error("external-result", "snapshot encoding failed"))?;
                if snapshot["current"] == false {
                    snapshot["result"] = Value::Null;
                }
                Ok(json!({"snapshot":snapshot,"state":self.status()?}))
            }
            Operation::GetStart { start_key } => {
                Ok(json!({"key":self.controller()?.get_start(start_key)?,"state":self.status()?}))
            }
            Operation::InvalidateInput {
                expected_plan_version,
                revision,
            } => {
                self.controller()?
                    .invalidate_input(expected_plan_version, revision)?;
                self.status()
            }
            Operation::CloseSession {} => {
                self.stop();
                self.status()
            }
            _ => Err(error("external-request", "unexpected operation")),
        }
    }
}
pub fn run(config: Configuration, domain: &str) -> Result<(), CoreError> {
    let (catalog, mut verifier) = config.package_manager.prepare_assets()?;
    if let Some(network) = config.network {
        verifier = verifier.with_network_provider(
            crate::isolated_worker::adapter::NetworkProvider::open(network)?,
        );
    }
    fs::create_dir_all(&config.permission_root)
        .map_err(|_| error("external-config", "permission directory unavailable"))?;
    let book = PermissionBook::open(&config.permission_root, config.permission_bytes)?;
    let vault = config
        .secret_root
        .as_ref()
        .map(|root| {
            fs::create_dir_all(root)
                .map_err(|_| error("secret-store", "fixed Host secret directory unavailable"))?;
            SecretVault::open(root)
        })
        .transpose()?;
    let host = ExecutionHost::new(domain, ResourcePolicy::from_value(config.resource_policy)?)?;
    let service = ExecutionService::new(host.clone())?;
    let controls = Arc::new(Controls {
        generation: Mutex::new(Arc::new(())),
        book: book.clone(),
        activity: Mutex::new(Activity::default()),
        closing: AtomicBool::new(false),
        manual: AtomicBool::new(false),
    });
    let output = Arc::new(Mutex::new(io::stdout()));
    let (sender, receiver) = mpsc::sync_channel(1);
    let input = {
        let c = controls.clone();
        let o = output.clone();
        std::thread::spawn(move || {
            reader(c, o, sender, config.request_bytes, config.response_bytes)
        })
    };
    let mut runner = Runner {
        catalog,
        verifier,
        vault,
        book,
        controls: controls.clone(),
        host,
        service,
        gate: ExecutionGate::default(),
        pending: None,
        session: None,
        used: BTreeMap::new(),
        context: None,
        activities: Vec::new(),
    };
    while let Ok(work) = receiver.recv() {
        if matches!(work.request.operation, Operation::Close {}) {
            runner.stop();
            break;
        }
        let publication = match &work.request.operation {
            Operation::GetRun { key } | Operation::CancelRun { key }
                if runner.controller().is_ok_and(|c| c.trace_is_recording()) =>
            {
                Some(key.clone())
            }
            _ => None,
        };
        let preparing = matches!(
            work.request.operation,
            Operation::Prepare { .. } | Operation::Activate { .. }
        );
        let mut result = if controls.closing.load(Ordering::Acquire) {
            Err(error("external-closed", "Host is closing"))
        } else if controls.manual.load(Ordering::Acquire)
            && !matches!(
                work.request.operation,
                Operation::Status {}
                    | Operation::Saved {}
                    | Operation::Definitions {}
                    | Operation::CloseSession {}
                    | Operation::Secrets {}
                    | Operation::SecretPut { .. }
                    | Operation::SecretDelete { .. }
            )
        {
            Err(error("external-protected", "manual protection is enabled"))
        } else {
            runner.handle(work.request.operation, work.cancel.clone())
        };
        if preparing && work.cancel.is_cancelled() {
            runner.stop();
            result = Err(error("external-cancelled", "preparation cancelled"));
        }
        controls
            .activity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active = None;
        let public_result = result.as_ref().is_ok_and(|r| {
            r["snapshot"]["current"] == true
                && matches!(
                    r["snapshot"]["result"]["delivery"].as_str(),
                    Some("complete" | "recovered")
                )
        });
        let sent = publish(
            &output,
            &work.request.id,
            result,
            config.response_bytes,
            Some((&controls, &work.generation, &work.cancel)),
        );
        if sent
            && public_result
            && let Some(key) = publication
            && let Ok(controller) = runner.controller()
        {
            controller.trace_response_published(&key);
        }
    }
    runner.stop();
    drop(receiver);
    let _ = input.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_controls_cannot_claim_roots_authority_or_automatic_protection() {
        for operation in [
            json!({"method":"status","path":"private"}),
            json!({"method":"prepare","graph":{"version":1,"nodes":[],"edges":[]},"trusted":true}),
            json!({"method":"protection","enabled":false,"automatic":false}),
            json!({"method":"approve","reviewId":"fake","ids":[],"save":true,"permissions":["*"]}),
        ] {
            assert!(
                serde_json::from_value::<Request>(json!({"id":"1","operation":operation})).is_err()
            );
        }
        assert!(
            serde_json::from_value::<Request>(
                json!({"id":"1","operation":{"method":"getStart","startKey":1}})
            )
            .is_err()
        );
    }
}
