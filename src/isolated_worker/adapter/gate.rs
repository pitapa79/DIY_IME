//! Host-only execute authorization. No Node-supplied or serialized permits.
use crate::execution_control::{CancellationToken, DecimalId, RunKey, SessionEpoch};
use crate::node::CoreError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GrantTarget {
    pub(super) artifact: String,
    pub(super) session: String,
    pub(super) plan: u64,
    pub(super) node: String,
}
impl GrantTarget {
    pub(super) fn new(
        artifact: String,
        session: &SessionEpoch,
        plan: DecimalId,
        node: String,
    ) -> Self {
        Self {
            artifact,
            session: session.as_str().to_owned(),
            plan: plan.get(),
            node,
        }
    }
}
struct State {
    manual: bool,
    automatic: bool,
    epoch: Arc<()>,
    grants: BTreeMap<GrantTarget, Arc<()>>,
    active: Vec<Weak<Lease>>,
    controllers: Vec<Weak<SessionOwner>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            manual: false,
            automatic: false,
            epoch: Arc::new(()),
            grants: BTreeMap::new(),
            active: Vec::new(),
            controllers: Vec::new(),
        }
    }
}
#[derive(Clone, Default)]
pub struct ExecutionGate(Arc<Mutex<State>>);
pub(super) struct Lease {
    target: GrantTarget,
    key: RunKey,
    epoch: Arc<()>,
    grant: Arc<()>,
    cancel: CancellationToken,
}
impl Lease {
    pub(super) fn same_grant(&self, other: &Self) -> bool {
        self.target == other.target
            && Arc::ptr_eq(&self.grant, &other.grant)
            && Arc::ptr_eq(&self.epoch, &other.epoch)
    }
}
fn denied() -> CoreError {
    CoreError::new(
        "worker-authority",
        "Host execution permission is absent, revoked, or protected",
    )
}
impl ExecutionGate {
    pub(in crate::isolated_worker) fn same_authority(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Trusted Host policy only. This is not the persisted user-permission format.
    pub fn approve(&self, target: GrantTarget) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for lease in state.active.iter().filter_map(Weak::upgrade) {
            if lease.target == target {
                lease.cancel.cancel();
            }
        }
        for controller in state.controllers.iter().filter_map(Weak::upgrade) {
            if controller.targets.contains(&target) {
                controller.close();
            }
        }
        state.grants.insert(target, Arc::new(()));
    }
    pub fn revoke(&self, target: &GrantTarget) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.grants.remove(target);
        for controller in state.controllers.iter().filter_map(Weak::upgrade) {
            if controller.targets.contains(target) {
                controller.close();
            }
        }
        for lease in state.active.iter().filter_map(Weak::upgrade) {
            if lease.target == *target {
                lease.cancel.cancel();
            }
        }
    }
    /// Manual OR automatic protection; every transition invalidates old leases.
    /// The Host must also invalidate/filter already-retained Core results.
    pub fn set_protection(&self, manual: bool, automatic: bool) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if (state.manual, state.automatic) != (manual, automatic) {
            state.epoch = Arc::new(());
            for controller in state.controllers.iter().filter_map(Weak::upgrade) {
                controller.close();
            }
            for lease in state.active.iter().filter_map(Weak::upgrade) {
                lease.cancel.cancel();
            }
        }
        state.manual = manual;
        state.automatic = automatic;
    }
    pub(super) fn begin(
        &self,
        target: GrantTarget,
        key: &RunKey,
        cancel: CancellationToken,
    ) -> Result<Arc<Lease>, CoreError> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.manual
            || state.automatic
            || key.session_epoch.as_str() != target.session
            || key.plan_version.get() != target.plan
        {
            return Err(denied());
        }
        let grant = state.grants.get(&target).cloned().ok_or_else(denied)?;
        let lease = Arc::new(Lease {
            target,
            key: key.clone(),
            epoch: state.epoch.clone(),
            grant,
            cancel,
        });
        state.active.retain(|x| x.strong_count() != 0);
        state.active.push(Arc::downgrade(&lease));
        Ok(lease)
    }
    pub(super) fn publish<T>(
        &self,
        lease: &Lease,
        key: &RunKey,
        publish: impl FnOnce() -> T,
    ) -> Result<T, CoreError> {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.manual
            || state.automatic
            || lease.key != *key
            || !Arc::ptr_eq(&state.epoch, &lease.epoch)
            || !state
                .grants
                .get(&lease.target)
                .is_some_and(|g| Arc::ptr_eq(g, &lease.grant))
        {
            return Err(denied());
        }
        // Linearize policy validation with enqueueing the completion event.
        Ok(publish())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protection_clears_recording_even_with_a_temporary_raw_operation_clone() {
        use crate::{
            execution_control::{ExecutionClock, ExecutionHost, ExecutionService, ResourcePolicy},
            graph::{ExecutionPlan, Graph},
            node::Registry,
        };
        let policy = ResourcePolicy::from_value(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        let host = ExecutionHost::new("trace-gate-immediate-clear", policy).unwrap();
        let service = ExecutionService::with_clock(
            host,
            ExecutionClock::controlled(std::time::Duration::ZERO),
        )
        .unwrap();
        let mut registry = Registry::new();
        crate::builtins::register_system_nodes(&mut registry).unwrap();
        let graph = Graph::from_json(
            r#"{"version":1,"nodes":[{"id":"source","kind":"input.text","version":1}],"edges":[]}"#,
        )
        .unwrap();
        let raw = service
            .open_ready(
                ExecutionPlan::compile(&graph, &registry).unwrap(),
                DecimalId::new(1).unwrap(),
            )
            .unwrap();
        let gate = ExecutionGate::default();
        let guarded = gate.seal_system_only(raw.clone()).unwrap();
        guarded.start_trace().unwrap();
        assert!(raw.read_trace(0, 256).unwrap().recording_id.is_some());
        gate.set_protection(true, false);
        assert!(raw.read_trace(0, 256).unwrap().recording_id.is_none());
        assert!(guarded.read_trace(0, 256).is_err());
        assert!(guarded.start_trace().is_err());
        gate.set_protection(false, false);
        assert!(!raw.read_trace(0, 256).unwrap().recording);
        assert!(!guarded.trace_is_recording());
    }
    fn values() -> (GrantTarget, RunKey) {
        let key: RunKey = serde_json::from_value(serde_json::json!({"sessionEpoch":"11111111111111111111111111111111","planVersion":"1","runId":"1","deliveryGeneration":"1"})).unwrap();
        (
            GrantTarget::new(
                "module-and-worker".into(),
                &key.session_epoch,
                key.plan_version,
                "node".into(),
            ),
            key,
        )
    }
    #[test]
    #[ignore = "requires real Windows AppContainer, pinned DLL and test Wasm"]
    fn revoke_ready_raw_worker_before_delivery() {
        use crate::isolated_worker::pipe::{PROBE_READY, WorkerScope, execute_observed};
        use crate::isolated_worker::{Cache, EndReason, Job, Outcome, Profile, RunControl};
        use std::{
            ffi::OsString,
            io::{Read, Write},
            path::Path,
            sync::{
                Arc,
                atomic::{AtomicU64, Ordering},
            },
        };
        let bundle =
            std::fs::canonicalize(std::env::var_os("DIY_COMPILED_PROBE_BUNDLE").unwrap()).unwrap();
        let fixture =
            std::fs::canonicalize(std::env::var_os("DIY_GENERIC_PROBE_WASM").unwrap()).unwrap();
        let gate = ExecutionGate::default();
        let (target, key) = values();
        gate.approve(target.clone());
        let cancel = CancellationToken::default();
        let lease = gate.begin(target.clone(), &key, cancel.clone()).unwrap();
        let mut profile = Profile::new().unwrap();
        let cache = Cache::new(Path::new(env!("CARGO_MANIFEST_DIR")), &profile).unwrap();
        let exe = cache
            .copy(&std::env::current_exe().unwrap(), "probe.exe")
            .unwrap();
        let dll = cache
            .copy(&bundle.join("runtime/wasmtime.dll"), "wasmtime.dll")
            .unwrap();
        let module = cache.copy(&fixture, "generic.wasm").unwrap();
        let digest = super::super::sha256(&std::fs::read(&fixture).unwrap()).unwrap();
        let hash = cache.write("generic.sha256", digest.as_bytes()).unwrap();
        let mode = cache.write("mode.txt", b"ready-generic-jit").unwrap();
        cache.allow_read_execute(&profile).unwrap();
        let job = Job::with_memory_limit(Some(512 * 1024 * 1024)).unwrap();
        let control = RunControl {
            cancel: cancel.clone(),
            walltime: Some(std::time::Duration::from_secs(30)),
            ..RunControl::default()
        };
        let activity = Arc::new(AtomicU64::new(0));
        let wrote = Arc::new(AtomicU64::new(0));
        let delivered = wrote.clone();
        let worker = WorkerScope::observed(&profile, &job, &cache, activity.clone())
            .with_test_exchange(Box::new(move |output, mut input| {
                let mut output = std::io::BufReader::new(output);
                let mut prefix = Vec::new();
                while prefix.len() < 4096 && !prefix.ends_with(PROBE_READY) {
                    let mut byte = [0]; output.read_exact(&mut byte).map_err(|e| e.to_string())?; prefix.push(byte[0]);
                }
                if !prefix.ends_with(PROBE_READY) { return Err("ready marker missing".into()); }
                gate.revoke(&target);
                if !cancel.is_cancelled() { return Err("revocation did not cancel lease".into()); }
                if gate.publish(&lease, &key, || { input.write_all(b"private-input").unwrap(); delivered.fetch_add(1, Ordering::SeqCst); }).is_ok() { return Err("revoked delivery accepted".into()); }
                gate.approve(target);
                if gate.publish(&lease, &key, || { input.write_all(b"private-input").unwrap(); delivered.fetch_add(1, Ordering::SeqCst); }).is_ok() { return Err("old lease revived".into()); }
                println!("revoke-ready real_gate_denied=true reapproval_old_lease_denied=true input_bytes=0");
                let mut tail = Vec::new(); let _ = output.read_to_end(&mut tail); Ok(tail)
            }));
        let args = [
            "--exact",
            "isolated_worker::runtime::compiled_module_probe::child",
            "--ignored",
            "--nocapture",
        ]
        .map(OsString::from);
        let Outcome::Exited(receipt) =
            execute_observed(worker, &exe, &args, Vec::new(), &control).unwrap()
        else {
            panic!("exit not confirmed")
        };
        assert_eq!(receipt.reason(), EndReason::Cancelled);
        assert_eq!(wrote.load(Ordering::SeqCst), 0);
        assert_eq!(activity.load(Ordering::Acquire), 0);
        drop(job);
        cache.revoke_read_execute(&profile).unwrap();
        profile.delete().unwrap();
        for path in [mode, hash, module, dll, exe] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(cache.path()).unwrap();
        println!("revoke-ready physical_exit=true activity=0 cleanup=true");
    }

    #[test]
    fn grants_are_bound_to_artifact_session_plan_node_and_actual_run() {
        let gate = ExecutionGate::default();
        let (target, key) = values();
        assert!(
            gate.begin(target.clone(), &key, CancellationToken::default())
                .is_err()
        );
        gate.approve(target.clone());
        for other in [
            GrantTarget {
                artifact: "other".into(),
                ..target.clone()
            },
            GrantTarget {
                session: "22222222222222222222222222222222".into(),
                ..target.clone()
            },
            GrantTarget {
                plan: 2,
                ..target.clone()
            },
            GrantTarget {
                node: "other".into(),
                ..target.clone()
            },
        ] {
            assert!(
                gate.begin(other, &key, CancellationToken::default())
                    .is_err()
            );
        }
        let lease = gate
            .begin(target, &key, CancellationToken::default())
            .unwrap();
        let mut other = key.clone();
        other.run_id = DecimalId::new(2).unwrap();
        assert!(
            gate.publish(&lease, &other, || panic!("wrong run published"))
                .is_err()
        );
        assert_eq!(gate.publish(&lease, &key, || 7).unwrap(), 7);
    }
    #[test]
    fn revoke_reapprove_and_manual_or_automatic_protection_never_resurrect_old_leases() {
        let gate = ExecutionGate::default();
        let (target, key) = values();
        gate.approve(target.clone());
        let cancel = CancellationToken::default();
        let lease = gate.begin(target.clone(), &key, cancel.clone()).unwrap();
        gate.revoke(&target);
        gate.approve(target.clone());
        assert!(cancel.is_cancelled());
        assert!(
            gate.publish(&lease, &key, || panic!("revoked published"))
                .is_err()
        );
        let lease = gate
            .begin(target.clone(), &key, CancellationToken::default())
            .unwrap();
        gate.set_protection(true, true);
        gate.set_protection(false, true);
        assert!(
            gate.begin(target.clone(), &key, CancellationToken::default())
                .is_err()
        );
        gate.set_protection(false, false);
        assert!(
            gate.publish(&lease, &key, || panic!("protected result published"))
                .is_err()
        );
        assert!(
            gate.begin(target, &key, CancellationToken::default())
                .is_ok()
        );
    }
}

use crate::execution_control::{
    ExecutionController, RunSnapshot, SessionControlState, StartReceipt, StartRequest,
};

struct SessionOwner {
    controller: Mutex<Option<ExecutionController>>,
    epoch: SessionEpoch,
    targets: Vec<GrantTarget>,
}
impl SessionOwner {
    fn close(&self) {
        if let Some(controller) = self
            .controller
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            controller.clear_trace();
        }
    }
}
struct Seal {
    epoch: Arc<()>,
    grants: Vec<(GrantTarget, Arc<()>)>,
}
/// Sole controller facade for a ready Wasm plan. No raw-controller getter.
/// Policy withdrawal closes this facade and rejects already-retained results.
#[derive(Clone)]
pub struct GuardedController {
    gate: ExecutionGate,
    owner: Arc<SessionOwner>,
    seal: Arc<Seal>,
}
impl ExecutionGate {
    /// Consume the sole Host controller owner; callers must not retain raw clones.
    pub fn seal(
        &self,
        controller: ExecutionController,
        targets: Vec<GrantTarget>,
    ) -> Result<GuardedController, CoreError> {
        self.seal_inner(controller, targets, false)
    }
    /// Internal composition only: the permission layer has compiled the graph
    /// with its canonical system-only Registry. No external job is authorized.
    pub(in crate::isolated_worker) fn seal_system_only(
        &self,
        controller: ExecutionController,
    ) -> Result<GuardedController, CoreError> {
        self.seal_inner(controller, vec![], true)
    }
    fn seal_inner(
        &self,
        controller: ExecutionController,
        targets: Vec<GrantTarget>,
        system_only: bool,
    ) -> Result<GuardedController, CoreError> {
        let plan = controller.plan_version()?;
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.manual || state.automatic || (targets.is_empty() && !system_only) {
            return Err(denied());
        }
        let mut grants = Vec::new();
        for target in &targets {
            if target.session != controller.session_epoch().as_str() || target.plan != plan.get() {
                return Err(denied());
            }
            grants.push((
                target.clone(),
                state.grants.get(target).cloned().ok_or_else(denied)?,
            ));
        }
        let owner = Arc::new(SessionOwner {
            epoch: controller.session_epoch().clone(),
            controller: Mutex::new(Some(controller)),
            targets,
        });
        let seal = Arc::new(Seal {
            epoch: state.epoch.clone(),
            grants,
        });
        state.controllers.retain(|x| x.strong_count() != 0);
        state.controllers.push(Arc::downgrade(&owner));
        Ok(GuardedController {
            gate: self.clone(),
            owner,
            seal,
        })
    }
}
impl GuardedController {
    pub(in crate::isolated_worker) fn trace_is_recording(&self) -> bool {
        self.call(|c| Ok(c.trace_is_recording())).unwrap_or(false)
    }
    pub(in crate::isolated_worker) fn trace_response_published(&self, key: &RunKey) {
        let _ = self.call(|c| {
            c.trace_response_published(key);
            Ok(())
        });
    }
    pub(in crate::isolated_worker) fn start_trace(&self) -> Result<(), CoreError> {
        self.call(ExecutionController::start_trace)
    }
    pub(in crate::isolated_worker) fn start_detailed_trace(&self) -> Result<(), CoreError> {
        self.call(ExecutionController::start_detailed_trace)
    }
    pub(in crate::isolated_worker) fn stop_trace(&self) -> Result<(), CoreError> {
        self.call(|c| {
            c.stop_trace();
            Ok(())
        })
    }
    pub(in crate::isolated_worker) fn clear_trace(&self) -> Result<(), CoreError> {
        self.call(|c| {
            c.clear_trace();
            Ok(())
        })
    }
    pub(in crate::isolated_worker) fn read_trace(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<crate::execution_control::TraceSnapshot, CoreError> {
        self.call(|c| c.read_trace(after, limit))
    }

    pub(in crate::isolated_worker) fn close(&self) {
        self.owner.close();
    }
    pub fn session_epoch(&self) -> &SessionEpoch {
        &self.owner.epoch
    }
    fn valid(&self, state: &State) -> bool {
        !state.manual
            && !state.automatic
            && Arc::ptr_eq(&state.epoch, &self.seal.epoch)
            && self.seal.grants.iter().all(|(target, grant)| {
                state
                    .grants
                    .get(target)
                    .is_some_and(|g| Arc::ptr_eq(g, grant))
            })
            && self
                .owner
                .controller
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
    }
    fn call<T>(
        &self,
        operation: impl FnOnce(&ExecutionController) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let controller = {
            let state = self.gate.0.lock().unwrap_or_else(|p| p.into_inner());
            if !self.valid(&state) {
                return Err(denied());
            }
            self.owner
                .controller
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .cloned()
                .ok_or_else(denied)?
        };
        let result = operation(&controller);
        let state = self.gate.0.lock().unwrap_or_else(|p| p.into_inner());
        if !self.valid(&state) {
            return Err(denied());
        }
        result
    }
    pub fn start_run(&self, request: StartRequest) -> Result<StartReceipt, CoreError> {
        self.call(|c| c.start_run(request))
    }
    pub fn get_run(&self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.call(|c| c.get_run(key))
    }
    pub(in crate::isolated_worker) fn wait_run(
        &self,
        key: &RunKey,
    ) -> Result<RunSnapshot, CoreError> {
        self.call(|c| c.wait_run(key, std::time::Duration::from_millis(40)))
    }
    pub fn cancel_run(&self, key: &RunKey) -> Result<RunSnapshot, CoreError> {
        self.call(|c| c.cancel_run(key))
    }
    pub fn get_start(&self, key: DecimalId) -> Result<RunKey, CoreError> {
        self.call(|c| c.get_start(key))
    }
    pub fn control_state(&self) -> Result<SessionControlState, CoreError> {
        self.call(ExecutionController::control_state)
    }
    pub fn invalidate_input(&self, plan: DecimalId, revision: u64) -> Result<(), CoreError> {
        self.call(|c| c.invalidate_input(plan, revision))
    }
}
