//! Host-owned consent. Stored fingerprints never contain input/config/secret text.
use super::super::{
    catalog::{read_bounded, regular, replace, write_temp},
    format::is_hash,
};
use super::{ApprovalRequest, ApprovalSubject, PermissionPlan};
use crate::{
    execution_control::{
        CancellationToken, DecimalId, ExecutionController, ExecutionService, RunKey, SessionEpoch,
    },
    isolated_worker::adapter::{ExecutionGate, GrantTarget, GuardedController},
    node::CoreError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
fn err(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
fn denied() -> CoreError {
    err(
        "permission-required",
        "current implementation/input-flow permission required",
    )
}
#[derive(Clone, Copy)]
pub enum PermissionLifetime {
    ThisSession,
    Saved,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    format_version: u32,
    saved: Vec<String>,
}
struct Binding {
    controller: GuardedController,
    cancel: CancellationToken,
    epoch: String,
    gate: ExecutionGate,
    targets: Vec<GrantTarget>,
    required: BTreeSet<ApprovalSubject>,
    live: AtomicBool,
}
impl Binding {
    fn withdraw(&self) {
        self.cancel.cancel();
        self.invalidate();
    }
    fn invalidate(&self) {
        if self.live.swap(false, Ordering::AcqRel) {
            self.controller.close();
            for target in &self.targets {
                self.gate.revoke(target);
            }
        }
    }
}
impl Drop for Binding {
    fn drop(&mut self) {
        self.invalidate();
    }
}
struct State {
    root: PathBuf,
    limit: usize,
    _lock: File,
    saved: BTreeSet<ApprovalSubject>,
    once: BTreeMap<String, BTreeSet<ApprovalSubject>>,
    bindings: Vec<Weak<Binding>>,
    closed: BTreeSet<String>,
    generation: Arc<()>,
    manual: bool,
    automatic: bool,
}
impl Drop for State {
    fn drop(&mut self) {
        for binding in self.bindings.iter().filter_map(Weak::upgrade) {
            binding.withdraw();
        }
    }
}
#[derive(Clone)]
pub struct PermissionBook(Arc<Mutex<State>>);
fn persist(state: &State, saved: &BTreeSet<ApprovalSubject>) -> Result<(), CoreError> {
    let bytes = serde_json::to_vec(&Stored {
        format_version: 1,
        saved: saved.iter().map(|s| s.0.clone()).collect(),
    })
    .map_err(|_| err("permission-store", "permission encoding failed"))?;
    if bytes.len() > state.limit {
        return Err(err(
            "permission-limit",
            "selected permission storage limit exceeded",
        ));
    }
    regular(&state.root, true)?;
    let temporary = write_temp(&state.root, &bytes)?;
    if let Err(error) = replace(&temporary, &state.root.join("permissions.json")) {
        let _ = fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}
fn has(state: &State, epoch: &str, subject: &ApprovalSubject) -> bool {
    !state.closed.contains(epoch)
        && (state.saved.contains(subject)
            || state
                .once
                .get(epoch)
                .is_some_and(|set| set.contains(subject)))
}
impl PermissionBook {
    /// One live Host owns this book. Other processes cannot silently cache stale grants.
    /// Host creates/selects this directory; guests never receive access to it.
    pub fn open(root: &Path, file_limit: usize) -> Result<Self, CoreError> {
        let root = root
            .canonicalize()
            .map_err(|_| err("permission-store", "Host data root unavailable"))?;
        regular(&root, true)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(0x00200000)
            .open(root.join("permissions.lock"))
            .map_err(|_| {
                err(
                    "permission-busy",
                    "permission book already owned/unavailable",
                )
            })?;
        let metadata = lock
            .metadata()
            .map_err(|_| err("permission-store", "permission lock metadata unavailable"))?;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(err("permission-store", "invalid permission lock object"));
        }
        let path = root.join("permissions.json");
        let saved = match fs::symlink_metadata(&path) {
            Ok(_) => {
                let bytes = read_bounded(&path, file_limit)?;
                let stored: Stored = serde_json::from_slice(&bytes)
                    .map_err(|_| err("permission-store", "invalid stored permission fields"))?;
                let mut saved = BTreeSet::new();
                if stored.format_version != 1 {
                    return Err(err("permission-store", "unsupported permission format"));
                }
                for id in stored.saved {
                    if !is_hash(&id) || !saved.insert(ApprovalSubject(id)) {
                        return Err(err(
                            "permission-store",
                            "invalid/duplicate saved permission",
                        ));
                    }
                }
                saved
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(_) => return Err(err("permission-store", "stored permission unavailable")),
        };
        Ok(Self(Arc::new(Mutex::new(State {
            root,
            limit: file_limit,
            _lock: lock,
            saved,
            once: BTreeMap::new(),
            bindings: vec![],
            closed: BTreeSet::new(),
            generation: Arc::new(()),
            manual: false,
            automatic: false,
        }))))
    }
    pub fn saved_ids(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .saved
            .iter()
            .map(|s| s.0.clone())
            .collect()
    }
    /// Create the exact ready Core snapshot here; never expose its raw controller.
    pub fn open_session(
        &self,
        service: &ExecutionService,
        mut plan: PermissionPlan,
        version: DecimalId,
    ) -> Result<PendingPermissionSession, CoreError> {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.manual || state.automatic {
            return Err(denied());
        }
        let generation = state.generation.clone();
        drop(state);
        let controller = service.open_ready(
            plan.plan
                .take()
                .ok_or_else(|| err("permission-plan", "compiled plan already consumed"))?,
            version,
        )?;
        Ok(PendingPermissionSession {
            book: self.clone(),
            epoch: controller.session_epoch().clone(),
            controller: Some(controller),
            plan: Some(plan),
            generation,
            owns_session: true,
        })
    }
    /// Revocation removes both saved and Session-only approvals immediately.
    /// If durable writing fails, live authority stays revoked and the error must
    /// be shown; do not claim persistence across restart in that case.
    pub fn revoke_id(&self, id: &str) -> Result<(), CoreError> {
        if !is_hash(id) {
            return Err(err("permission-id", "invalid permission ID"));
        }
        let subject = ApprovalSubject(id.into());
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let saved = state.saved.remove(&subject);
        for set in state.once.values_mut() {
            set.remove(&subject);
        }
        state.generation = Arc::new(());
        for binding in state.bindings.iter().filter_map(Weak::upgrade) {
            if binding.required.contains(&subject) {
                binding.withdraw();
            }
        }
        if saved {
            persist(&state, &state.saved)?;
        }
        Ok(())
    }
    pub fn set_protection(&self, manual: bool, automatic: bool) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if (state.manual, state.automatic) != (manual, automatic) {
            state.generation = Arc::new(());
            for binding in state.bindings.iter().filter_map(Weak::upgrade) {
                binding.withdraw();
                binding.gate.set_protection(manual, automatic);
            }
        }
        state.manual = manual;
        state.automatic = automatic;
    }
    fn close_session(&self, epoch: &SessionEpoch) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.once.remove(epoch.as_str());
        state.closed.insert(epoch.as_str().into());
        for binding in state.bindings.iter().filter_map(Weak::upgrade) {
            if binding.epoch == epoch.as_str() {
                binding.invalidate();
            }
        }
    }
    fn current(state: &State, epoch: &SessionEpoch, generation: &Arc<()>) -> bool {
        !state.manual
            && !state.automatic
            && !state.closed.contains(epoch.as_str())
            && Arc::ptr_eq(&state.generation, generation)
    }
    fn approve(
        &self,
        epoch: &SessionEpoch,
        generation: &Arc<()>,
        subjects: Vec<ApprovalSubject>,
        lifetime: PermissionLifetime,
    ) -> Result<(), CoreError> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if !Self::current(&state, epoch, generation) {
            return Err(denied());
        }
        match lifetime {
            PermissionLifetime::ThisSession => {
                state
                    .once
                    .entry(epoch.as_str().into())
                    .or_default()
                    .extend(subjects);
            }
            PermissionLifetime::Saved => {
                let mut saved = state.saved.clone();
                saved.extend(subjects);
                if saved != state.saved {
                    persist(&state, &saved)?;
                    state.saved = saved;
                }
            }
        }
        Ok(())
    }
}
pub struct PendingPermissionSession {
    book: PermissionBook,
    epoch: SessionEpoch,
    controller: Option<ExecutionController>,
    plan: Option<PermissionPlan>,
    generation: Arc<()>,
    owns_session: bool,
}
impl PendingPermissionSession {
    pub(in crate::isolated_worker) fn retained_stages(
        &self,
        proof: &super::super::StaticOfficialPipeline,
    ) -> Result<
        Vec<(
            String,
            crate::isolated_worker::adapter::WasmNode,
            serde_json::Value,
        )>,
        CoreError,
    > {
        let plan = self.plan.as_ref().expect("pending plan");
        if !proof.matches_plan(plan) {
            return Err(denied());
        }
        proof
            .stages()
            .iter()
            .map(|stage| {
                let node = plan
                    .nodes
                    .iter()
                    .find(|node| node.id == stage.id() && &node.config == stage.config())
                    .ok_or_else(denied)?;
                Ok((node.id.clone(), node.node.clone(), node.config.clone()))
            })
            .collect()
    }
    pub fn is_current(&self) -> bool {
        PermissionBook::current(
            &self.book.0.lock().unwrap_or_else(|p| p.into_inner()),
            &self.epoch,
            &self.generation,
        )
    }
    pub fn session_epoch(&self) -> &SessionEpoch {
        &self.epoch
    }
    pub fn requests(&self) -> &[ApprovalRequest] {
        &self.plan.as_ref().expect("pending plan").requests
    }
    pub fn missing(&self) -> Vec<ApprovalRequest> {
        let state = self.book.0.lock().unwrap_or_else(|p| p.into_inner());
        self.requests()
            .iter()
            .filter(|r| {
                !PermissionBook::current(&state, &self.epoch, &self.generation)
                    || !has(&state, self.epoch.as_str(), &r.subject)
            })
            .cloned()
            .collect()
    }
    /// Only IDs in this immutable Host-held review can be approved.
    pub fn approve(&self, ids: &[String], lifetime: PermissionLifetime) -> Result<(), CoreError> {
        let available: BTreeMap<_, _> = self
            .requests()
            .iter()
            .map(|r| (r.subject.id(), r.subject.clone()))
            .collect();
        let mut seen = BTreeSet::new();
        let mut subjects = Vec::new();
        for id in ids {
            if !seen.insert(id) {
                return Err(err("permission-id", "duplicate approval ID"));
            }
            subjects.push(
                available
                    .get(id.as_str())
                    .ok_or_else(|| err("permission-id", "ID is not in this Host-held review"))?
                    .clone(),
            );
        }
        if subjects.is_empty() {
            return Err(err("permission-id", "empty approval selection"));
        }
        self.book
            .approve(&self.epoch, &self.generation, subjects, lifetime)
    }
    pub fn activate(self) -> Result<PermissionSession, CoreError> {
        self.activate_with_cancel(CancellationToken::default())
    }
    pub fn activate_with_cancel(
        self,
        cancel: CancellationToken,
    ) -> Result<PermissionSession, CoreError> {
        self.activate_official_with_cancel(cancel, None)
    }
    pub(in crate::isolated_worker) fn activate_official_with_cancel(
        mut self,
        cancel: CancellationToken,
        proof: Option<&super::super::StaticOfficialPipeline>,
    ) -> Result<PermissionSession, CoreError> {
        if cancel.is_cancelled() {
            return Err(denied());
        }
        let mut state = self.book.0.lock().unwrap_or_else(|p| p.into_inner());
        let plan = self.plan.as_ref().expect("pending plan");
        if proof.is_some_and(|proof| !proof.matches_plan(plan)) {
            return Err(denied());
        }
        if !PermissionBook::current(&state, &self.epoch, &self.generation)
            || plan
                .requests
                .iter()
                .any(|r| !has(&state, self.epoch.as_str(), &r.subject))
        {
            return Err(denied());
        }
        let controller = self.controller.take().expect("pending controller");
        let version = controller.plan_version()?;
        let targets: Vec<_> = plan
            .nodes
            .iter()
            .map(|n| n.node.grant_target(&self.epoch, version, n.id.clone()))
            .collect();
        plan.gate.set_protection(false, false);
        for target in &targets {
            plan.gate.approve(target.clone());
        }
        let guarded = if plan.nodes.is_empty() {
            // PermissionPlan owns the exact canonical compiled graph; callers
            // cannot supply a Registry or substitute Native processing code.
            plan.gate.seal_system_only(controller)
        } else {
            plan.gate.seal(controller, targets.clone())
        };
        let guarded = match guarded {
            Ok(guarded) => guarded,
            Err(error) => {
                for target in &targets {
                    plan.gate.revoke(target);
                }
                return Err(error);
            }
        };
        let binding = Arc::new(Binding {
            controller: guarded.clone(),
            cancel: cancel.clone(),
            epoch: self.epoch.as_str().into(),
            gate: plan.gate.clone(),
            targets,
            required: plan.requests.iter().map(|r| r.subject.clone()).collect(),
            live: AtomicBool::new(true),
        });
        state.bindings.retain(|b| b.strong_count() > 0);
        state.bindings.push(Arc::downgrade(&binding));
        self.owns_session = false;
        let session = PermissionSession {
            book: self.book.clone(),
            epoch: self.epoch.clone(),
            controller: guarded,
            _binding: binding,
        };
        drop(state);
        let companions = proof.and_then(|proof| proof.dictionary_companions());
        let preparation_key = if companions.is_some() {
            Some(RunKey {
                session_epoch: self.epoch.clone(),
                plan_version: version,
                run_id: DecimalId::new(1)?,
                input_revision: None,
                delivery_generation: session.controller.control_state()?.delivery_generation,
            })
        } else {
            None
        };
        // This sealed controller remains private until all resource snapshots
        // have passed guest preparation. Binding withdrawal cancels its Worker.
        plan.initialize_resources(
            cancel.clone(),
            &mut || {
                if cancel.is_cancelled() || !session._binding.live.load(Ordering::Acquire) {
                    return Err(denied());
                }
                session.controller.control_state().map(|_| ())
            },
            companions
                .zip(preparation_key.as_ref())
                .map(|((id, modules), key)| (id, modules, key)),
        )?;
        if cancel.is_cancelled() || !session._binding.live.load(Ordering::Acquire) {
            return Err(denied());
        }
        Ok(session)
    }
}
impl Drop for PendingPermissionSession {
    fn drop(&mut self) {
        if self.owns_session {
            self.book.close_session(&self.epoch);
        }
    }
}
/// Ready immutable plan. Retained controller clones are invalidated on revocation,
/// protection or this owner's drop; no prepare/activate/raw-controller escape.
pub struct PermissionSession {
    book: PermissionBook,
    epoch: SessionEpoch,
    controller: GuardedController,
    _binding: Arc<Binding>,
}
impl PermissionSession {
    pub fn controller(&self) -> &GuardedController {
        &self.controller
    }
    pub fn session_epoch(&self) -> &SessionEpoch {
        &self.epoch
    }
}
impl Drop for PermissionSession {
    fn drop(&mut self) {
        self.book.close_session(&self.epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "diyime-consent-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn book(&self, limit: usize) -> PermissionBook {
            PermissionBook::open(&self.0, limit).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for entry in fs::read_dir(&self.0).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                assert!(
                    matches!(name.as_str(), "permissions.json" | "permissions.lock")
                        || name.starts_with(".catalog-")
                );
                assert!(entry.file_type().unwrap().is_file());
                fs::remove_file(entry.path()).unwrap();
            }
            fs::remove_dir(&self.0).unwrap();
        }
    }
    fn epoch(c: char) -> SessionEpoch {
        SessionEpoch::new(c.to_string().repeat(32)).unwrap()
    }
    fn subject(c: char) -> ApprovalSubject {
        ApprovalSubject(c.to_string().repeat(64))
    }
    fn generation(book: &PermissionBook) -> Arc<()> {
        book.0.lock().unwrap().generation.clone()
    }
    #[test]
    fn session_only_is_not_serialized_saved_reopens_and_exclusive_ownership_is_enforced() {
        let fixture = Fixture::new();
        let book = fixture.book(1024 * 1024);
        let a = epoch('a');
        let once = subject('0');
        let saved = subject('1');
        assert!(PermissionBook::open(&fixture.0, 1024 * 1024).is_err());
        book.approve(
            &a,
            &generation(&book),
            vec![once.clone()],
            PermissionLifetime::ThisSession,
        )
        .unwrap();
        book.approve(
            &a,
            &generation(&book),
            vec![saved.clone()],
            PermissionLifetime::Saved,
        )
        .unwrap();
        let bytes = fs::read(fixture.0.join("permissions.json")).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"formatVersion":1,"saved":[saved.id()]})
        );
        drop(book);
        let book = fixture.book(1024 * 1024);
        let state = book.0.lock().unwrap();
        assert!(has(&state, a.as_str(), &saved));
        assert!(!has(&state, a.as_str(), &once));
        drop(state);
        book.revoke_id(saved.id()).unwrap();
        drop(book);
        assert!(fixture.book(1024 * 1024).saved_ids().is_empty());
    }
    #[test]
    fn real_storage_failure_never_creates_saved_authority_and_revocation_stays_denied() {
        let fixture = Fixture::new();
        let book = fixture.book(1024 * 1024);
        let a = epoch('a');
        let item = subject('2');
        let path = fixture.0.join("permissions.json");
        fs::write(&path, br#"{"formatVersion":1,"saved":[]}"#).unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(
            book.approve(
                &a,
                &generation(&book),
                vec![item.clone()],
                PermissionLifetime::Saved
            )
            .is_err()
        );
        assert!(book.saved_ids().is_empty());
        drop(locked);
        book.approve(
            &a,
            &generation(&book),
            vec![item.clone()],
            PermissionLifetime::Saved,
        )
        .unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(book.revoke_id(item.id()).is_err());
        assert!(!has(&book.0.lock().unwrap(), a.as_str(), &item));
        drop(locked);
        // The old file survives the failed durable revocation. Error callers must
        // never claim restart persistence; immediate in-memory authority is gone.
        assert_eq!(
            serde_json::from_slice::<Stored>(&fs::read(path).unwrap())
                .unwrap()
                .saved
                .len(),
            1
        );
    }
    #[test]
    fn protection_transitions_and_close_cannot_resurrect_old_session_approvals() {
        let fixture = Fixture::new();
        let book = fixture.book(1024 * 1024);
        let a = epoch('a');
        let b = epoch('b');
        let item = subject('3');
        let old = generation(&book);
        book.approve(
            &a,
            &old,
            vec![item.clone()],
            PermissionLifetime::ThisSession,
        )
        .unwrap();
        book.set_protection(true, true);
        book.set_protection(false, true);
        assert!(!PermissionBook::current(&book.0.lock().unwrap(), &a, &old));
        book.set_protection(false, false);
        assert!(
            book.approve(&a, &old, vec![item.clone()], PermissionLifetime::Saved)
                .is_err()
        );
        book.close_session(&a);
        let state = book.0.lock().unwrap();
        assert!(!has(&state, a.as_str(), &item));
        assert!(!has(&state, b.as_str(), &item));
    }
    #[test]
    fn malformed_unknown_duplicate_and_oversized_permission_files_fail_closed() {
        let fixture = Fixture::new();
        let hash = subject('4').0;
        for value in [
            serde_json::json!({"formatVersion":2,"saved":[]}),
            serde_json::json!({"formatVersion":1,"saved":[],"trusted":true}),
            serde_json::json!({"formatVersion":1,"saved":[hash,hash]}),
            serde_json::json!({"formatVersion":1,"saved":["../outside"]}),
        ] {
            fs::write(
                fixture.0.join("permissions.json"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
            assert!(PermissionBook::open(&fixture.0, 1024 * 1024).is_err());
        }
        fs::write(
            fixture.0.join("permissions.json"),
            br#"{"formatVersion":1,"saved":[]}"#,
        )
        .unwrap();
        assert!(PermissionBook::open(&fixture.0, 1).is_err());
        fs::remove_file(fixture.0.join("permissions.json")).unwrap();
        let book = fixture.book(1);
        let a = epoch('a');
        assert!(
            book.approve(
                &a,
                &generation(&book),
                vec![subject('5')],
                PermissionLifetime::Saved
            )
            .is_err()
        );
        assert!(!fixture.0.join("permissions.json").exists());
    }
    #[test]
    fn canonical_system_only_plans_need_no_external_grants_but_retain_protection_and_owner_fences()
    {
        use crate::execution_control::{ExecutionHost, ExecutionService, ResourcePolicy};
        use std::time::{Duration, Instant};
        let fixture = Fixture::new();
        let book = fixture.book(1024 * 1024);
        let policy = ResourcePolicy::from_value(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../../tests/contracts/execution_control.json"
            ))
            .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
        let service =
            ExecutionService::new(ExecutionHost::new("canonical-system-only", policy).unwrap())
                .unwrap();
        let graph = crate::graph::Graph::from_json(
            r#"{"version":1,"nodes":[{"id":"source","kind":"input.text","version":1}],"edges":[]}"#,
        )
        .unwrap();
        let unknown = crate::graph::Graph::from_json(r#"{"version":1,"nodes":[{"id":"processing","kind":"text.romaji-to-hiragana","version":1}],"edges":[]}"#).unwrap();
        assert!(PermissionPlan::capture(&unknown, &BTreeMap::new()).is_err());
        let capture = || PermissionPlan::capture(&graph, &BTreeMap::new()).unwrap();
        // The public arbitrary-controller entry still rejects an empty grant set.
        let mut raw = capture();
        let controller = service
            .open_ready(raw.plan.take().unwrap(), DecimalId::new(1).unwrap())
            .unwrap();
        assert!(raw.gate.seal(controller, vec![]).is_err());
        let pending = book
            .open_session(&service, capture(), DecimalId::new(1).unwrap())
            .unwrap();
        assert!(pending.requests().is_empty());
        assert!(pending.missing().is_empty());
        let session = pending.activate().unwrap();
        let retained = session.controller().clone();
        let receipt = retained.start_run(serde_json::from_value(serde_json::json!({
            "startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}],
            "input":{"revision":1,"text":"system-only-input","mode":"compose"}
        })).unwrap()).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = retained.get_run(&receipt.key).unwrap();
            if let Some(result) = snapshot.result {
                assert_eq!(
                    result.ports[0].value["packet"]["root"]["text"],
                    "system-only-input"
                );
                break;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        book.set_protection(true, false);
        assert!(retained.get_run(&receipt.key).is_err());
        book.set_protection(false, false);
        assert!(retained.control_state().is_err());
        drop(session);
        let replacement = book
            .open_session(&service, capture(), DecimalId::new(1).unwrap())
            .unwrap()
            .activate()
            .unwrap();
        let closed = replacement.controller().clone();
        drop(replacement);
        assert!(closed.control_state().is_err());
        assert!(book.saved_ids().is_empty());
    }
}
