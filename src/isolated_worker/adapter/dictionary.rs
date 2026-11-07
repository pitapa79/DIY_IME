//! Fixed Host dictionary bindings. Declarations/reviews do not read file bodies.
use super::{WasmArtifact, fail, sha256, wire};
use crate::{
    execution_control::CancellationToken,
    node::{CoreError, NodeDefinition, PrepareResources},
    resources::{ResourceBatch, ResourceLimits, ResourceRoot},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictionaryBindingConfig {
    pub root: PathBuf,
    pub reference_pointer: String,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub max_files: usize,
    pub budget_ms: u64,
}
struct Binding {
    root: ResourceRoot,
    pointer: String,
    policy: String,
    maximum: usize,
}
#[derive(Default)]
pub struct DictionaryBindings {
    entries: BTreeMap<String, Binding>,
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}
fn pointer(value: &str) -> bool {
    if !value.starts_with('/') || value.len() > 4096 {
        return false;
    }
    let mut bytes = value.bytes();
    while let Some(c) = bytes.next() {
        if c == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return false;
        }
    }
    true
}
pub(super) fn reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value
            .chars()
            .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
        && value.split('/').all(|p| {
            let base = p.split('.').next().unwrap_or("").to_ascii_uppercase();
            !p.is_empty()
                && p != "."
                && p != ".."
                && !p.ends_with('.')
                && !p.ends_with(' ')
                && !matches!(
                    base.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                )
                && !((base.starts_with("COM") || base.starts_with("LPT"))
                    && matches!(
                        base.get(3..),
                        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
                    ))
        })
}
impl DictionaryBindings {
    pub fn open(
        configs: BTreeMap<String, DictionaryBindingConfig>,
    ) -> Result<Arc<Self>, CoreError> {
        let mut entries = BTreeMap::new();
        for (name, mut config) in configs {
            if !id(&name) || !pointer(&config.reference_pointer) {
                return Err(fail("dictionary-binding", "invalid fixed Host binding"));
            }
            let root = ResourceRoot::open(
                &config.root,
                ResourceLimits {
                    max_file_bytes: config.max_file_bytes,
                    max_total_bytes: config.max_total_bytes,
                    max_files: config.max_files,
                    budget: Duration::from_millis(config.budget_ms),
                },
            )?;
            config.root = config
                .root
                .canonicalize()
                .map_err(|_| fail("dictionary-binding", "fixed resource root unavailable"))?;
            // Native root stays in trusted memory. Only its policy fingerprint is exposed.
            let policy = sha256(
                &serde_json::to_vec(&config)
                    .map_err(|_| fail("dictionary-binding", "binding identity unavailable"))?,
            )?;
            entries.insert(
                name,
                Binding {
                    root,
                    pointer: config.reference_pointer,
                    policy,
                    maximum: config.max_file_bytes,
                },
            );
        }
        Ok(Arc::new(Self { entries }))
    }
    pub(in crate::isolated_worker) fn plans(
        self: &Arc<Self>,
        capabilities: &[(String, String)],
        configs: &[Value],
    ) -> Result<Option<DictionaryPlans>, CoreError> {
        if capabilities.is_empty() {
            return Ok(None);
        }
        let mut selected = BTreeMap::new();
        for (capability, name) in capabilities {
            if capability != "dictionary.read" {
                return Err(fail(
                    "package-capability",
                    "required Host broker is not installed",
                ));
            }
            let binding = self.entries.get(name).ok_or_else(|| {
                fail(
                    "package-capability",
                    "required dictionary binding is not configured",
                )
            })?;
            selected.insert(name.clone(), binding);
        }
        let identity = sha256(
            &serde_json::to_vec(
                &selected
                    .iter()
                    .map(|(id, b)| (id, &b.policy))
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| fail("dictionary-binding", "binding identity unavailable"))?,
        )?;
        let mut scopes = Vec::new();
        for config in configs {
            if scopes
                .iter()
                .any(|(known, _): &(Value, Arc<Scope>)| known == config)
            {
                continue;
            }
            let mut descriptions = Vec::new();
            let mut references = BTreeMap::new();
            for (name, binding) in &selected {
                let path = config
                    .pointer(&binding.pointer)
                    .and_then(Value::as_str)
                    .filter(|r| reference(r))
                    .ok_or_else(|| {
                        fail(
                            "dictionary-reference",
                            "configuration has no safe relative resource reference",
                        )
                    })?;
                if references.insert(path.to_owned(), name.clone()).is_some() {
                    return Err(fail(
                        "dictionary-binding",
                        "ambiguous reference across bindings",
                    ));
                }
                descriptions.push(ResourceDescription {
                    binding: name.clone(),
                    reference: path.into(),
                    policy_sha256: binding.policy.clone(),
                });
            }
            scopes.push((
                config.clone(),
                Arc::new(Scope {
                    descriptions,
                    references,
                    ready: OnceLock::new(),
                }),
            ));
        }
        Ok(Some(DictionaryPlans {
            bindings: self.clone(),
            identity,
            scopes,
        }))
    }
    pub(in crate::isolated_worker) fn batch(&self) -> DictionaryBatch<'_> {
        DictionaryBatch {
            provider: self,
            snapshots: BTreeMap::new(),
            batches: self
                .entries
                .iter()
                .map(|(id, b)| (id.clone(), b.root.batch()))
                .collect(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDescription {
    pub binding: String,
    pub reference: String,
    pub policy_sha256: String,
}
struct Scope {
    descriptions: Vec<ResourceDescription>,
    references: BTreeMap<String, String>,
    ready: OnceLock<BTreeMap<String, Arc<[u8]>>>,
}
#[derive(Clone)]
pub(in crate::isolated_worker) struct DictionaryPlans {
    pub bindings: Arc<DictionaryBindings>,
    pub identity: String,
    scopes: Vec<(Value, Arc<Scope>)>,
}
pub(in crate::isolated_worker) struct DictionaryBatch<'a> {
    provider: &'a DictionaryBindings,
    batches: BTreeMap<String, ResourceBatch<'a>>,
    snapshots: BTreeMap<(String, String), Arc<[u8]>>,
}
impl DictionaryPlans {
    fn scope(&self, config: &Value) -> Result<&Scope, CoreError> {
        self.scopes
            .iter()
            .find(|(value, _)| value == config)
            .map(|(_, scope)| scope.as_ref())
            .ok_or_else(|| {
                fail(
                    "dictionary-config",
                    "resource configuration is not captured",
                )
            })
    }
    pub fn descriptions(&self, config: &Value) -> Result<Vec<ResourceDescription>, CoreError> {
        Ok(self.scope(config)?.descriptions.clone())
    }
    pub fn fresh(&self) -> Self {
        Self {
            bindings: self.bindings.clone(),
            identity: self.identity.clone(),
            scopes: self
                .scopes
                .iter()
                .map(|(config, scope)| {
                    (
                        config.clone(),
                        Arc::new(Scope {
                            descriptions: scope.descriptions.clone(),
                            references: scope.references.clone(),
                            ready: OnceLock::new(),
                        }),
                    )
                })
                .collect(),
        }
    }
    pub fn merge(&mut self, other: &Self) -> Result<(), CoreError> {
        if self.identity != other.identity || !Arc::ptr_eq(&self.bindings, &other.bindings) {
            return Err(fail(
                "dictionary-binding",
                "resource proofs use different Host bindings",
            ));
        }
        // A merged proof may have belonged to an older prepared Session.
        // Carry declarations only; every new config needs a new snapshot.
        for (config, scope) in &other.fresh().scopes {
            if !self.scopes.iter().any(|(known, _)| known == config) {
                self.scopes.push((config.clone(), scope.clone()));
            }
        }
        Ok(())
    }
    pub fn capture(
        &self,
        config: &Value,
        batch: &mut DictionaryBatch<'_>,
        cancel: &CancellationToken,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if !std::ptr::eq(self.bindings.as_ref(), batch.provider) {
            return Err(fail(
                "dictionary-binding",
                "resource batch belongs to another Host binding",
            ));
        }
        for (reference, name) in &self.scope(config)?.references {
            if cancel.is_cancelled() {
                return Err(fail("worker-cancelled", "resource preparation cancelled"));
            }
            check()?;
            let key = (name.clone(), reference.clone());
            if batch.snapshots.contains_key(&key) {
                continue;
            }
            let selected = &self.bindings.entries[name];
            let bytes = batch
                .batches
                .get_mut(name)
                .expect("selected binding")
                .read(reference, selected.maximum)?;
            if cancel.is_cancelled() {
                return Err(fail("worker-cancelled", "resource preparation cancelled"));
            }
            check()?;
            batch.snapshots.insert(key, bytes);
        }
        Ok(())
    }
    pub fn initialize(
        &self,
        artifact: &WasmArtifact,
        definition: &NodeDefinition,
        config: &Value,
        batch: &mut DictionaryBatch<'_>,
        cancel: CancellationToken,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "resource preparation cancelled"));
        }
        check()?;
        if !std::ptr::eq(self.bindings.as_ref(), batch.provider) {
            return Err(fail(
                "dictionary-binding",
                "resource batch belongs to another Host binding",
            ));
        }
        let scope = self.scope(config)?;
        if scope.ready.get().is_some() {
            return Ok(());
        }
        let mut snapshots = BTreeMap::new();
        for (reference, name) in &scope.references {
            if cancel.is_cancelled() {
                return Err(fail("worker-cancelled", "resource preparation cancelled"));
            }
            check()?;
            let bytes = batch
                .snapshots
                .get(&(name.clone(), reference.clone()))
                .cloned()
                .ok_or_else(|| fail("dictionary-not-prepared", "resource capture is incomplete"))?;
            snapshots.insert(reference.clone(), bytes);
        }
        let payload: BTreeMap<_, _> = snapshots
            .iter()
            .map(|(id, bytes)| (id, bytes.as_ref()))
            .collect();
        let valid = wire::reply(&artifact.call(
            &wire::request(
                &definition.kind,
                definition.version,
                json!({"method":"prepare","config":config,"resources":payload}),
            ),
            cancel.clone(),
        )?)?;
        if valid.as_object().is_none_or(|v| v.len() != 2)
            || valid["valid"] != true
            || !valid["prepared"].is_boolean()
        {
            return Err(fail("worker-reply", "invalid resource preparation reply"));
        }
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "resource preparation cancelled"));
        }
        check()?;
        let _ = scope.ready.set(snapshots);
        Ok(())
    }
    pub fn payload(&self, config: &Value) -> Result<Value, CoreError> {
        let ready = self.scope(config)?.ready.get().ok_or_else(|| {
            fail(
                "dictionary-not-prepared",
                "approved resource snapshots are not prepared",
            )
        })?;
        let payload: BTreeMap<_, _> = ready
            .iter()
            .map(|(id, bytes)| (id, bytes.as_ref()))
            .collect();
        serde_json::to_value(payload)
            .map_err(|_| fail("dictionary-binding", "resource projection unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "diyime-dictionary-binding-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn config(&self) -> DictionaryBindingConfig {
            DictionaryBindingConfig {
                root: self.0.clone(),
                reference_pointer: "/path".into(),
                max_file_bytes: 1024,
                max_total_bytes: 2048,
                max_files: 2,
                budget_ms: 5000,
            }
        }
        fn bindings(&self) -> Arc<DictionaryBindings> {
            DictionaryBindings::open(BTreeMap::from([("dictionary".into(), self.config())]))
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for name in ["sample.tsv", "other.tsv"] {
                let p = self.0.join(name);
                if p.exists() {
                    fs::remove_file(p).unwrap();
                }
            }
            fs::remove_dir(&self.0).unwrap();
        }
    }
    #[test]
    fn review_describes_only_relative_references_without_reading_or_minting_ready_payloads() {
        let fixture = Fixture::new();
        let bindings = fixture.bindings();
        let config = json!({"path":"sample.tsv"});
        let plans = bindings
            .plans(
                &[("dictionary.read".into(), "dictionary".into())],
                std::slice::from_ref(&config),
            )
            .unwrap()
            .unwrap();
        let description = serde_json::to_string(&plans.descriptions(&config).unwrap()).unwrap();
        assert!(description.contains("sample.tsv"));
        assert!(!description.contains(&fixture.0.to_string_lossy().to_string()));
        assert!(plans.payload(&config).is_err());
        let mut batch = bindings.batch();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(
            plans
                .capture(&config, &mut batch, &cancelled, &mut || panic!(
                    "cancelled policy check"
                ))
                .is_err()
        );
    }
    #[test]
    fn stable_capture_is_bound_to_config_provider_and_scope_and_does_not_publish_before_guest_validation()
     {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("sample.tsv"), b"before").unwrap();
        let bindings = fixture.bindings();
        let config = json!({"path":"sample.tsv"});
        let plans = bindings
            .plans(
                &[("dictionary.read".into(), "dictionary".into())],
                std::slice::from_ref(&config),
            )
            .unwrap()
            .unwrap();
        let mut batch = bindings.batch();
        plans
            .capture(
                &config,
                &mut batch,
                &CancellationToken::default(),
                &mut || Ok(()),
            )
            .unwrap();
        assert!(fs::write(fixture.0.join("sample.tsv"), b"writer").is_err());
        let captured = batch
            .snapshots
            .get(&("dictionary".into(), "sample.tsv".into()))
            .unwrap()
            .clone();
        assert!(plans.payload(&config).is_err());
        assert!(plans.descriptions(&json!({"path":"other.tsv"})).is_err());
        let other = fixture.bindings();
        assert!(
            plans
                .capture(
                    &config,
                    &mut other.batch(),
                    &CancellationToken::default(),
                    &mut || Ok(())
                )
                .is_err()
        );
        drop(batch);
        fs::write(fixture.0.join("sample.tsv"), b"after").unwrap();
        assert_eq!(&*captured, b"before");
    }
    #[test]
    fn unsafe_references_unknown_capabilities_bindings_and_invalid_policy_are_refused() {
        let fixture = Fixture::new();
        let bindings = fixture.bindings();
        for path in [
            "",
            "/absolute",
            "../outside",
            "a/../x",
            "a//b",
            "a\\b",
            "a:",
            "nul",
            "COM¹.txt",
            "LPT9",
            "trailing.",
            "space ",
            "a\nb",
        ] {
            assert!(
                bindings
                    .plans(
                        &[("dictionary.read".into(), "dictionary".into())],
                        &[json!({"path":path})]
                    )
                    .is_err(),
                "{path}"
            );
        }
        for capability in [
            ("network.fetch", "dictionary"),
            ("secret.use", "dictionary"),
            ("dictionary.read", "other"),
        ] {
            assert!(
                bindings
                    .plans(
                        &[(capability.0.into(), capability.1.into())],
                        &[json!({"path":"sample.tsv"})]
                    )
                    .is_err()
            );
        }
        for pointer_value in ["", "path", "/~", "/~2"] {
            let mut config = fixture.config();
            config.reference_pointer = pointer_value.into();
            assert!(
                DictionaryBindings::open(BTreeMap::from([("dictionary".into(), config)])).is_err()
            );
        }
        let mut config = fixture.config();
        config.max_file_bytes = 0;
        assert!(DictionaryBindings::open(BTreeMap::from([("dictionary".into(), config)])).is_err());
    }
    #[test]
    fn reused_verification_proofs_start_new_plan_snapshots_and_policy_changes_change_resource_identity()
     {
        let fixture = Fixture::new();
        let bindings = fixture.bindings();
        let config = json!({"path":"sample.tsv"});
        let plans = bindings
            .plans(
                &[("dictionary.read".into(), "dictionary".into())],
                std::slice::from_ref(&config),
            )
            .unwrap()
            .unwrap();
        plans
            .scope(&config)
            .unwrap()
            .ready
            .set(BTreeMap::from([(
                "sample.tsv".into(),
                Arc::from(b"old".as_slice()),
            )]))
            .unwrap();
        assert!(plans.payload(&config).is_ok());
        assert!(plans.fresh().payload(&config).is_err());
        let mut merged = bindings
            .plans(
                &[("dictionary.read".into(), "dictionary".into())],
                &[json!({"path":"another.tsv"})],
            )
            .unwrap()
            .unwrap();
        merged.merge(&plans).unwrap();
        assert!(merged.payload(&config).is_err());
        assert!(plans.payload(&config).is_ok());
        let mut changed = fixture.config();
        changed.max_file_bytes += 1;
        let other =
            DictionaryBindings::open(BTreeMap::from([("dictionary".into(), changed)])).unwrap();
        assert_ne!(
            plans.identity,
            other
                .plans(
                    &[("dictionary.read".into(), "dictionary".into())],
                    &[config]
                )
                .unwrap()
                .unwrap()
                .identity
        );
    }
}
