//! Host-owned implementation reviews and exact, non-serializable qualifications.
//! Review evidence grants no execution permission. The sealed Host controller and
//! each call's resource lease still authorize delivery and publication.
use super::*;
use crate::execution_control::RunKey;

fn denied() -> CoreError {
    fail(
        "retained-review",
        "retention qualification is absent, changed or revoked",
    )
}

/// An implementation review captures actual immutable Host assets and definition.
/// Only this module's trusted review policy can construct one; guest metadata,
/// author labels and caller-supplied digests are never review evidence.
struct ReviewedImplementation {
    artifact: WasmArtifact,
    definition: Arc<NodeDefinition>,
    grouped: bool,
}
impl ReviewedImplementation {
    fn capture(node: &WasmNode, grouped: bool) -> Self {
        Self {
            artifact: node.artifact.clone(),
            definition: node.definition.clone(),
            grouped,
        }
    }

    fn matches(&self, node: &WasmNode, grouped: bool) -> bool {
        Arc::ptr_eq(&self.artifact.0, &node.artifact.0)
            && Arc::ptr_eq(&self.definition, &node.definition)
            && (!grouped || self.grouped)
    }

    fn qualify(
        self,
        node: &WasmNode,
        id: &str,
        config: &Value,
        controller: &GuardedController,
        grouped: bool,
    ) -> Result<Qualification, CoreError> {
        if !self.matches(node, grouped) || !supported(node, config) {
            return Err(denied());
        }
        let state = controller.control_state()?;
        let target = node.grant_target(
            controller.session_epoch(),
            state.active_plan_version.ok_or_else(denied)?,
            id,
        );
        controller.authorize_retention_review(&node.gate, &target)?;
        Ok(Qualification {
            review: self,
            package: node.package_identity.clone().ok_or_else(denied)?,
            config: config.clone(),
            gate: node.gate.clone(),
            target,
            controller: controller.clone(),
            grouped,
        })
    }
}

/// Initial supported risk profile: fresh Store/Instance per call, captured code
/// only, no dictionary/prepared artifact/network/secret broker dependencies.
/// Finite process lifetime/calls and original physical ownership remain enforced
/// by the retained driver; a supported resource declaration alone is NOT review.
fn supported(node: &WasmNode, config: &Value) -> bool {
    node.package_identity.is_some()
        && node.configs.contains(config)
        && node.artifact.0.policy.supports_range_pipeline()
        && node
            .artifact
            .0
            .policy
            .process_memory_bytes
            .is_some_and(|n| n > 0)
        && node.network.is_none()
        && node.dictionary.is_none()
        && node.prepared_dictionary.get().is_none()
}

/// Parse only the ambient-authority surface of an already Wasmtime-validated
/// module. No allocations based on guest counts; section/name/LEB bounds fail
/// closed. This is not a substitute for isolated preparation/ABI validation.
struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], CoreError> {
        let bytes = self.0.get(..count).ok_or_else(denied)?;
        self.0 = &self.0[count..];
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8, CoreError> {
        Ok(self.take(1)?[0])
    }
    fn integer(&mut self) -> Result<u32, CoreError> {
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            if shift == 28 && byte > 15 {
                return Err(denied());
            }
            value |= u32::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err(denied())
    }
    fn name(&mut self) -> Result<&'a [u8], CoreError> {
        let len = self.integer()? as usize;
        self.take(len)
    }
}
fn audit_imports(bytes: &[u8]) -> Result<(), CoreError> {
    let mut module = Cursor(bytes);
    if module.take(8)? != b"\0asm\x01\0\0\0" {
        return Err(denied());
    }
    let mut seen_imports = false;
    while !module.0.is_empty() {
        let section = module.byte()?;
        let len = module.integer()? as usize;
        let mut payload = Cursor(module.take(len)?);
        if section > 13 {
            return Err(denied());
        }
        if section != 2 {
            continue;
        }
        if seen_imports {
            return Err(denied());
        }
        seen_imports = true;
        let count = payload.integer()?;
        // Every import consumes at least two names, a kind and type index.
        if count as usize > payload.0.len() / 4 {
            return Err(denied());
        }
        for _ in 0..count {
            if payload.name()? != b"diyime_resource_v1"
                || payload.name()? != b"read"
                || payload.byte()? != 0
            // Only a function; never imported memory/table/global.
            {
                return Err(denied());
            }
            let _validated_type_index = payload.integer()?;
        }
        if !payload.0.is_empty() {
            return Err(denied());
        }
    }
    Ok(())
}

/// General S1 review policy for a narrow, enforceable retention-risk profile.
/// WasmNode can only be produced by this Host's isolated describe/validate path;
/// its captured Worker/runtime/definition/configs are private immutable assets.
/// Inspect actual code imports, not declared capabilities. With no broker
/// dependency and only the empty per-call read scope, no ambient OS or previous
/// Store state is reachable through imports. Native retained invocation installs
/// a fresh limited Store/Instance and destroys its callback before RESULT.
/// Residual process memory remains an explicit S1 risk, not a wiping guarantee.
/// S2 still requires its independent, previously audited experiment record.
fn implementation_review(
    node: &WasmNode,
    grouped: bool,
) -> Result<ReviewedImplementation, CoreError> {
    audit_imports(&node.artifact.0.module)?;
    let audited_group = node.artifact.0.identity.rsplit(':').next()
        == Some("105c0ceb2854dba27a747eae5faa6e393bb99a88ce5ab3048a468fa9add88134")
        && node.definition.version == 1
        && matches!(
            node.definition.kind.as_str(),
            "experimental.packet-passthrough"
                | "experimental.text-trim"
                | "experimental.text-case"
                | "demo.identity"
                | "text.romaji-to-hiragana"
                | "experimental.text-concat"
        );
    if grouped && !audited_group {
        return Err(denied());
    }
    Ok(ReviewedImplementation::capture(node, audited_group))
}

pub(super) struct Qualification {
    review: ReviewedImplementation,
    package: String,
    config: Value,
    gate: ExecutionGate,
    target: GrantTarget,
    controller: GuardedController,
    grouped: bool,
}
impl Qualification {
    pub(super) fn review(
        node: &WasmNode,
        id: &str,
        config: &Value,
        controller: &GuardedController,
        grouped: bool,
    ) -> Result<Self, CoreError> {
        implementation_review(node, grouped)?.qualify(node, id, config, controller, grouped)
    }

    /// Check before process preparation and again before every input delivery.
    /// Immutable Arc identity binds all captured code/runtime/Worker/policy and
    /// the full validated definition without rehashing module bytes per input.
    pub(super) fn check(
        &self,
        node: &WasmNode,
        id: &str,
        config: &Value,
        key: Option<&RunKey>,
    ) -> Result<(), CoreError> {
        if !self.review.matches(node, self.grouped)
            || node.package_identity.as_ref() != Some(&self.package)
            || !self.gate.same_authority(&node.gate)
            || config != &self.config
            || !supported(node, config)
            || key.is_some_and(|key| {
                node.grant_target(&key.session_epoch, key.plan_version, id) != self.target
            })
            || self.target.node != id
        {
            return Err(denied());
        }
        self.controller
            .authorize_retention_review(&node.gate, &self.target)
    }
}

impl WasmNode {
    pub(in crate::isolated_worker) fn retention_evaluation_eligible(
        &self,
        config: &Value,
        grouped: bool,
    ) -> bool {
        supported(self, config)
            && implementation_review(self, grouped)
                .is_ok_and(|review| review.matches(self, grouped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        builtins::register_system_nodes,
        execution_control::{ExecutionService, ResourcePolicy},
        graph::{ExecutionPlan, Graph},
    };

    // Matching-only fixtures: two valid modules differ in executable i32.const,
    // not custom sections. These tiny assets are NOT production runtime proof.
    fn node(value: u8, gate: ExecutionGate, policy: WorkerPolicy, worker: u8) -> WasmNode {
        let module: Arc<[u8]> = Arc::from([
            0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 127, 3, 2, 1, 0, 7, 5, 1, 1, 102, 0, 0,
            10, 6, 1, 4, 0, 65, value, 11,
        ]);
        let worker: Arc<[u8]> = Arc::from([worker]);
        let dll: Arc<[u8]> = Arc::from(b"matching-only-runtime".as_slice());
        let identity = format!(
            "{}:{}:{}",
            sha256(&worker).unwrap(),
            sha256(&dll).unwrap(),
            sha256(&module).unwrap()
        );
        let image = Image {
            profile_pool: None,
            workspace: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            worker,
            dll,
            module,
            identity,
            policy,
            requests: AtomicU64::new(0),
            activity: Arc::new(AtomicU64::new(0)),
            timings: Arc::new(TimingCounters::default()),
            runtime_files: Arc::new(std::sync::Mutex::new(None)),
            module_files: std::sync::Mutex::new(None),
            code_budget: Arc::new(CodeBudget::default()),
            compiled: None,
            jit_compiled: std::sync::Mutex::new(CompiledState::Pending),
            late_code_cache: None,
        };
        WasmNode {
            artifact: WasmArtifact(Arc::new(image)),
            definition: Arc::new(NodeDefinition::new("fixture.reviewed", 1, vec![], vec![])),
            configs: Arc::new(vec![json!({}), json!({"mode":"other"})]),
            gate,
            package_identity: Some(format!("independent-package-{value}")),
            dictionary: None,
            prepared_dictionary: Arc::default(),
            network: None,
            retained: retained::NodeLink::default(),
            prepared_jit: Arc::default(),
        }
    }
    fn controller(nodes: &[(&WasmNode, &str)]) -> GuardedController {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../tests/contracts/execution_control.json"
        ))
        .unwrap();
        let host = ExecutionHost::new(
            "retention-review-unit",
            ResourcePolicy::from_value(contract["resourceProfiles"]["noTombstones"].clone())
                .unwrap(),
        )
        .unwrap();
        let service = ExecutionService::new(host).unwrap();
        let mut registry = Registry::new();
        register_system_nodes(&mut registry).unwrap();
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
        let targets: Vec<_> = nodes
            .iter()
            .map(|(node, id)| {
                node.grant_target(raw.session_epoch(), DecimalId::new(1).unwrap(), *id)
            })
            .collect();
        let gate = &nodes[0].0.gate;
        for target in &targets {
            gate.approve(target.clone());
        }
        gate.seal(raw, targets).unwrap()
    }
    fn qualify(node: &WasmNode, id: &str, controller: &GuardedController) -> Qualification {
        // The actual internal S1 review policy, followed by sealed qualification.
        implementation_review(node, false)
            .unwrap()
            .qualify(node, id, &json!({}), controller, false)
            .unwrap()
    }
    fn key(controller: &GuardedController) -> RunKey {
        RunKey {
            session_epoch: controller.session_epoch().clone(),
            plan_version: DecimalId::new(1).unwrap(),
            run_id: DecimalId::new(1).unwrap(),
            input_revision: None,
            delivery_generation: DecimalId::new(1).unwrap(),
        }
    }

    fn imports(module_name: &[u8], field: &[u8], kind: u8) -> Vec<u8> {
        let mut payload = vec![1, module_name.len() as u8];
        payload.extend_from_slice(module_name);
        payload.push(field.len() as u8);
        payload.extend_from_slice(field);
        payload.extend_from_slice(&[kind, 0]);
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        module.extend_from_slice(&[2, payload.len() as u8]);
        module.extend(payload);
        module
    }

    #[test]
    fn audit_actual_import_bytes_rejects_ambient_imports_and_malformed_bounds() {
        // Import-only snippets test this bounded surface parser. Full validation
        // and function signature checks still belong to actual isolated prepare.
        let read = imports(b"diyime_resource_v1", b"read", 0);
        audit_imports(&read).unwrap();
        audit_imports(b"\0asm\x01\0\0\0").unwrap();
        for bad in [
            imports(b"wasi_snapshot_preview1", b"fd_read", 0),
            imports(b"env", b"memory", 2),
            imports(b"diyime_resource_v1", b"write", 0),
            imports(b"diyime_resource_v1", b"read", 1),
            imports(b"diyime_resource_v1", b"read", 2),
            imports(b"diyime_resource_v1", b"read", 3),
            imports(b"diyime_resource_v1", b"read", 4),
        ] {
            assert!(audit_imports(&bad).is_err());
        }
        for end in 0..read.len() {
            if end == 8 {
                continue;
            }
            assert!(
                audit_imports(&read[..end]).is_err(),
                "truncated import at {end}"
            );
        }
        let mut duplicate = read.clone();
        duplicate.extend_from_slice(&read[8..]);
        assert!(audit_imports(&duplicate).is_err());
        for tail in [
            vec![2, 255, 255, 255, 255, 31],     // u32 overflow
            vec![2, 128, 128, 128, 128, 128, 0], // overlong u32
            vec![2, 1, 255],                     // incomplete import count
            vec![2, 2, 127, 0],                  // count cannot fit section
            vec![2, 2, 0, 0],                    // trailing import bytes
            vec![14, 0],                         // unknown section surface
        ] {
            let mut bytes = b"\0asm\x01\0\0\0".to_vec();
            bytes.extend(tail);
            assert!(audit_imports(&bytes).is_err());
        }
    }

    #[test]
    fn independently_reviewed_executable_code_qualifies_without_kind_allowlist() {
        let gate = ExecutionGate::default();
        let a = node(1, gate.clone(), WorkerPolicy::bounded_probe(), 1);
        let mut b = node(2, gate, WorkerPolicy::bounded_probe(), 1);
        b.definition = Arc::new(NodeDefinition::new(
            "thirdparty.distinct",
            7,
            vec![],
            vec![],
        ));
        assert_ne!(a.artifact.0.module, b.artifact.0.module);
        assert_ne!(a.artifact.0.identity, b.artifact.0.identity);
        assert!(a.retention_evaluation_eligible(&json!({}), false));
        assert!(!a.retention_evaluation_eligible(&json!({}), true));
        assert!(b.retention_evaluation_eligible(&json!({}), false));
        assert!(!b.retention_evaluation_eligible(&json!({}), true));
        let controller = controller(&[(&a, "a"), (&b, "b")]);
        let qa = qualify(&a, "a", &controller);
        let qb = qualify(&b, "b", &controller);
        qa.check(&a, "a", &json!({}), Some(&key(&controller)))
            .unwrap();
        qb.check(&b, "b", &json!({}), Some(&key(&controller)))
            .unwrap();
        assert!(qa.check(&b, "a", &json!({}), None).is_err());
        assert!(qb.check(&a, "b", &json!({}), None).is_err());
    }

    #[test]
    fn exact_configuration_definition_package_worker_runtime_and_policy_are_bound() {
        let gate = ExecutionGate::default();
        let node = node(1, gate.clone(), WorkerPolicy::bounded_probe(), 1);
        let controller = controller(&[(&node, "node")]);
        let q = qualify(&node, "node", &controller);
        q.check(&node.clone(), "node", &json!({}), None).unwrap();
        assert!(
            q.check(&node, "node", &json!({"mode":"other"}), None)
                .is_err()
        );
        assert!(q.check(&node, "other", &json!({}), None).is_err());
        let mut changed = node.clone();
        changed.definition = Arc::new(NodeDefinition {
            version: 2,
            ..(*node.definition).clone()
        });
        assert!(q.check(&changed, "node", &json!({}), None).is_err());
        changed = node.clone();
        changed.package_identity = None;
        assert!(q.check(&changed, "node", &json!({}), None).is_err());
        changed.package_identity = Some("different-package".into());
        assert!(q.check(&changed, "node", &json!({}), None).is_err());
        changed = node.clone();
        changed.configs = Arc::new(vec![json!({"mode":"other"})]);
        assert!(q.check(&changed, "node", &json!({}), None).is_err());
        changed = node.clone();
        changed.gate = ExecutionGate::default();
        assert!(q.check(&changed, "node", &json!({}), None).is_err());
        for mut replacement in [
            self::node(2, gate.clone(), WorkerPolicy::bounded_probe(), 1),
            self::node(1, gate.clone(), WorkerPolicy::bounded_probe(), 2),
            self::node(
                1,
                gate.clone(),
                WorkerPolicy {
                    fuel_per_call: Some(100),
                    ..WorkerPolicy::bounded_probe()
                },
                1,
            ),
        ] {
            replacement.definition = node.definition.clone();
            replacement.package_identity = node.package_identity.clone();
            assert!(q.check(&replacement, "node", &json!({}), None).is_err());
        }
        // An independently captured runtime owner also requires a fresh review,
        // even if its module bytes and displayed package identity match.
        let mut replacement = self::node(1, gate, WorkerPolicy::bounded_probe(), 1);
        let image = Arc::get_mut(&mut replacement.artifact.0).unwrap();
        image.dll = Arc::from(b"different-runtime".as_slice());
        image.identity = format!(
            "{}:{}:{}",
            sha256(&image.worker).unwrap(),
            sha256(&image.dll).unwrap(),
            sha256(&image.module).unwrap()
        );
        assert!(q.check(&replacement, "node", &json!({}), None).is_err());
    }

    #[test]
    fn current_authority_context_revocation_and_protection_cannot_resurrect_qualification() {
        for protection in [false, true] {
            let node = node(
                1,
                ExecutionGate::default(),
                WorkerPolicy::bounded_probe(),
                1,
            );
            let controller = controller(&[(&node, "node")]);
            let q = qualify(&node, "node", &controller);
            let mut changed_key = key(&controller);
            changed_key.plan_version = DecimalId::new(2).unwrap();
            assert!(
                q.check(&node, "node", &json!({}), Some(&changed_key))
                    .is_err()
            );
            if protection {
                node.gate.set_protection(true, false);
                node.gate.set_protection(false, false);
            } else {
                node.gate.revoke(&q.target);
                node.gate.approve(q.target.clone());
            }
            assert!(q.check(&node, "node", &json!({}), None).is_err());
            assert!(
                ReviewedImplementation::capture(&node, false)
                    .qualify(&node, "node", &json!({}), &controller, false)
                    .is_err()
            );
        }
    }

    #[test]
    fn review_does_not_grant_permissions_or_implicitly_approve_grouping_or_unbounded_risk() {
        let node = node(
            1,
            ExecutionGate::default(),
            WorkerPolicy::bounded_probe(),
            1,
        );
        let controller = controller(&[(&node, "node")]);
        assert!(
            ReviewedImplementation::capture(&node, false)
                .qualify(&node, "node", &json!({}), &controller, true)
                .is_err()
        );
        for policy in [
            WorkerPolicy::default(),
            WorkerPolicy {
                process_memory_bytes: None,
                ..WorkerPolicy::bounded_probe()
            },
        ] {
            let unbounded = self::node(1, node.gate.clone(), policy, 1);
            assert!(!supported(&unbounded, &json!({})));
            assert!(
                ReviewedImplementation::capture(&unbounded, false)
                    .qualify(&unbounded, "node", &json!({}), &controller, false)
                    .is_err()
            );
        }
        node.gate.revoke(&node.grant_target(
            controller.session_epoch(),
            DecimalId::new(1).unwrap(),
            "node",
        ));
        assert!(
            ReviewedImplementation::capture(&node, false)
                .qualify(&node, "node", &json!({}), &controller, false)
                .is_err()
        );
    }
}
