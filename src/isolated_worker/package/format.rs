//! One framed file / one Wasm Node. Metadata declares requests, never authority.
use super::super::adapter::{
    self, DictionaryBindings, ExecutionGate, WasmArtifact, WasmNode, WorkerPolicy,
};
use crate::{
    execution_control::CancellationToken,
    node::{CoreError, Inputs, Node, NodeDefinition, Outputs, Registry, RunContext},
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
    sync::Arc,
};

pub const MAGIC: &[u8; 8] = b"DIY2NODE";
pub const FORMAT_VERSION: u32 = 1;
const HEADER_BYTES: usize = 24;
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageLimits {
    pub manifest_bytes: usize,
    pub module_bytes: usize,
    pub package_bytes: usize,
    pub catalog_bytes: usize,
    pub batch_bytes: usize,
}
impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: 1024 * 1024,
            module_bytes: 64 * 1024 * 1024,
            package_bytes: 65 * 1024 * 1024 + HEADER_BYTES,
            catalog_bytes: 16 * 1024 * 1024,
            batch_bytes: 512 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeKind {
    Wasm32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityRequest {
    pub id: String,
    pub binding: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub package_id: String,
    pub version: Version,
    pub display_name: String,
    pub runtime: RuntimeKind,
    pub guest_api_version: u32,
    pub node: NodeDefinition,
    pub module_sha256: String,
    pub capabilities: Vec<CapabilityRequest>,
}
fn error(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
pub(super) fn digest(bytes: &[u8]) -> Result<String, CoreError> {
    adapter::sha256(bytes).map_err(|_| error("package-digest", "SHA256 unavailable"))
}
pub(super) fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}
struct MetadataOnly(NodeDefinition);
impl Node for MetadataOnly {
    fn definition(&self) -> NodeDefinition {
        self.0.clone()
    }
    fn validate_config(&self, _: &serde_json::Value) -> Result<(), CoreError> {
        Ok(())
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        _: &Inputs,
        _: &serde_json::Value,
    ) -> Result<Outputs, CoreError> {
        Err(error(
            "package-metadata",
            "metadata checker cannot execute Nodes",
        ))
    }
}
impl Manifest {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !identifier(&self.package_id)
            || self.display_name.trim().is_empty()
            || self.display_name.len() > 4096
            || self.display_name.contains('\0')
        {
            return Err(error(
                "package-manifest",
                "invalid package identity/display metadata",
            ));
        }
        if self.guest_api_version != 1 || !is_hash(&self.module_sha256) {
            return Err(error(
                "package-version",
                "unsupported guest API or digest format",
            ));
        }
        if matches!(
            self.node.kind.as_str(),
            "input.text" | "demo.packet" | "e2e.input" | "e2e.output"
        ) {
            return Err(error(
                "package-kind",
                "system Nodes cannot be replaced by a package",
            ));
        }
        let mut capabilities = BTreeSet::new();
        for capability in &self.capabilities {
            if !identifier(&capability.id)
                || !identifier(&capability.binding)
                || !capabilities.insert(capability)
            {
                return Err(error(
                    "package-capability",
                    "invalid or duplicate capability declaration",
                ));
            }
        }
        let mut registry = Registry::new();
        registry
            .register(MetadataOnly(self.node.clone()))
            .map_err(|_| {
                error(
                    "package-definition",
                    "invalid ports/type/default definition",
                )
            })?;
        Ok(())
    }
    pub fn authorization_digest(&self) -> Result<String, CoreError> {
        let mut capabilities = self.capabilities.clone();
        capabilities.sort();
        let subject = serde_json::json!({"packageId":self.package_id,"moduleSha256":self.module_sha256,"node":self.node,"capabilities":capabilities});
        digest(
            &serde_json::to_vec(&subject)
                .map_err(|_| error("package-manifest", "identity encoding failed"))?,
        )
    }
}
#[derive(Clone)]
pub struct Package {
    manifest: Manifest,
    bytes: Arc<[u8]>,
    module: Arc<[u8]>,
    digest: String,
    authorization: String,
}
impl Package {
    pub fn parse(bytes: Vec<u8>, limits: &PackageLimits) -> Result<Self, CoreError> {
        if bytes.len() > limits.package_bytes {
            return Err(error(
                "package-limit",
                "package exceeds selected size limit",
            ));
        }
        let mut cursor = Cursor::new(bytes.as_slice());
        let mut header = [0u8; HEADER_BYTES];
        cursor
            .read_exact(&mut header)
            .map_err(|_| error("package-frame", "truncated package header"))?;
        if &header[..8] != MAGIC {
            return Err(error("package-frame", "unknown package magic"));
        }
        if u32::from_le_bytes(header[8..12].try_into().unwrap()) != FORMAT_VERSION {
            return Err(error("package-version", "unsupported package format"));
        }
        let manifest_len = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
        let module_len = usize::try_from(u64::from_le_bytes(header[16..24].try_into().unwrap()))
            .map_err(|_| error("package-limit", "module length overflow"))?;
        let total = HEADER_BYTES
            .checked_add(manifest_len)
            .and_then(|n| n.checked_add(module_len))
            .ok_or_else(|| error("package-limit", "package length overflow"))?;
        if manifest_len == 0
            || manifest_len > limits.manifest_bytes
            || module_len < 8
            || module_len > limits.module_bytes
            || total != bytes.len()
        {
            return Err(error(
                "package-frame",
                "package lengths do not match bounded framing",
            ));
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes[HEADER_BYTES..HEADER_BYTES + manifest_len])
                .map_err(|_| error("package-manifest", "invalid/unknown manifest fields"))?;
        manifest.validate()?;
        let module = &bytes[HEADER_BYTES + manifest_len..];
        if !module.starts_with(b"\0asm\x01\0\0\0") || digest(module)? != manifest.module_sha256 {
            return Err(error(
                "package-digest",
                "Wasm payload/type does not match the declared digest",
            ));
        }
        let module = Arc::from(module);
        let digest = digest(&bytes)?;
        let authorization = manifest.authorization_digest()?;
        Ok(Self {
            manifest,
            module,
            bytes: Arc::from(bytes),
            digest,
            authorization,
        })
    }
    pub fn read(mut input: impl Read, limits: &PackageLimits) -> Result<Self, CoreError> {
        let mut bytes = Vec::new();
        input
            .by_ref()
            .take((limits.package_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| error("package-read", "package read failed"))?;
        Self::parse(bytes, limits)
    }
    pub fn encode(
        manifest: Manifest,
        module: &[u8],
        limits: &PackageLimits,
    ) -> Result<Self, CoreError> {
        manifest.validate()?;
        let json = serde_json::to_vec(&manifest)
            .map_err(|_| error("package-manifest", "manifest encoding failed"))?;
        if json.len() > limits.manifest_bytes
            || module.len() > limits.module_bytes
            || HEADER_BYTES
                .checked_add(json.len())
                .and_then(|n| n.checked_add(module.len()))
                .is_none_or(|n| n > limits.package_bytes)
        {
            return Err(error(
                "package-limit",
                "encoded package exceeds selected limits",
            ));
        }
        let manifest_len = u32::try_from(json.len())
            .map_err(|_| error("package-limit", "manifest length overflow"))?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&manifest_len.to_le_bytes());
        bytes.extend_from_slice(&(module.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&json);
        bytes.extend_from_slice(module);
        Self::parse(bytes, limits)
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn authorization_digest(&self) -> &str {
        &self.authorization
    }
}
#[derive(Clone)]
pub struct VerifiedPackage {
    pub(super) package: Package,
    pub(super) node: WasmNode,
}
impl VerifiedPackage {
    pub fn package(&self) -> &Package {
        &self.package
    }
    pub fn node(&self) -> WasmNode {
        self.node.clone()
    }
}
/// Isolated installation evidence only. Cannot supply a Node to PermissionPlan.
///
/// ```compile_fail
/// fn executable(p: diy_ime2_core::isolated_worker::package::VerifiedInstallation)
///     -> diy_ime2_core::isolated_worker::adapter::WasmNode {
///     p.node()
/// }
/// ```
pub struct VerifiedInstallation {
    package: Package,
}
impl VerifiedInstallation {
    pub fn package(&self) -> &Package {
        &self.package
    }
}
/// Guest-v1 verifier. Capability declarations require a configured typed
/// Host broker provider; declaration strings never create authority.
pub struct PackageVerifier {
    dictionary_bindings: Arc<DictionaryBindings>,
    network: Option<Arc<crate::isolated_worker::adapter::NetworkProvider>>,
    runtime: WasmArtifact,
    policy: WorkerPolicy,
}
impl PackageVerifier {
    pub fn new(runtime: WasmArtifact, policy: WorkerPolicy) -> Self {
        Self {
            runtime,
            policy,
            dictionary_bindings: Arc::new(DictionaryBindings::default()),
            network: None,
        }
    }
    pub fn with_network_provider(
        mut self,
        provider: Arc<crate::isolated_worker::adapter::NetworkProvider>,
    ) -> Self {
        self.network = Some(provider);
        self
    }
    fn capabilities(package: &Package) -> Result<Vec<(String, String)>, CoreError> {
        let capabilities: Vec<_> = package
            .manifest
            .capabilities
            .iter()
            .map(|c| (c.id.clone(), c.binding.clone()))
            .collect();
        if capabilities.iter().any(|(id, binding)| {
            !matches!(
                id.as_str(),
                "dictionary.read" | "network.request" | "secret.use"
            ) || id == "secret.use"
                && !capabilities
                    .iter()
                    .any(|(other, name)| other == "network.request" && name == binding)
        }) {
            return Err(error(
                "package-capability",
                "unsupported or mismatched Host broker capability",
            ));
        }
        Ok(capabilities)
    }
    pub fn with_dictionary_bindings(mut self, bindings: Arc<DictionaryBindings>) -> Self {
        self.dictionary_bindings = bindings;
        self
    }
    /// Installation checks describe/default config in real isolation without
    /// touching Host resources. It creates no executable Node/grant proof.
    pub fn verify_installation(
        &self,
        package: Package,
        gate: ExecutionGate,
        cancel: CancellationToken,
    ) -> Result<VerifiedInstallation, CoreError> {
        Self::capabilities(&package)?;
        let _definition = self.verify_node(&package, vec![], gate, cancel)?;
        Ok(VerifiedInstallation { package })
    }
    fn verify_node(
        &self,
        package: &Package,
        configs: Vec<serde_json::Value>,
        gate: ExecutionGate,
        cancel: CancellationToken,
    ) -> Result<WasmNode, CoreError> {
        let artifact = self
            .runtime
            .with_module(package.module.to_vec(), self.policy.clone())?;
        let node = artifact.prepare(
            &package.manifest.node.kind,
            package.manifest.node.version,
            configs,
            gate,
            cancel,
        )?;
        use crate::execution_control::DeferredNode;
        if serde_json::to_value(node.definition()).ok()
            != serde_json::to_value(&package.manifest.node).ok()
        {
            return Err(error(
                "package-definition",
                "isolated guest definition differs from manifest",
            ));
        }
        Ok(node)
    }
    pub fn verify(
        &self,
        package: Package,
        gate: ExecutionGate,
        cancel: CancellationToken,
    ) -> Result<VerifiedPackage, CoreError> {
        self.verify_configs(package, vec![], gate, cancel)
    }
    /// Host preparation thread only: validate exact graph configs before Core activation.
    pub fn verify_configs(
        &self,
        package: Package,
        configs: Vec<serde_json::Value>,
        gate: ExecutionGate,
        cancel: CancellationToken,
    ) -> Result<VerifiedPackage, CoreError> {
        self.verify_configs_with_vault(package, configs, gate, cancel, None)
    }
    pub fn verify_configs_with_vault(
        &self,
        package: Package,
        configs: Vec<serde_json::Value>,
        gate: ExecutionGate,
        cancel: CancellationToken,
        vault: Option<&super::secrets::SecretVault>,
    ) -> Result<VerifiedPackage, CoreError> {
        let capabilities = Self::capabilities(&package)?;
        let network = if let Some(provider) = &self.network {
            provider.capture(&capabilities, vault)?
        } else if capabilities
            .iter()
            .any(|(id, _)| id == "network.request" || id == "secret.use")
        {
            return Err(error(
                "package-capability",
                "required network provider is not configured",
            ));
        } else {
            None
        };
        let mut resource_configs = vec![package.manifest.node.default_config.clone()];
        resource_configs.extend(configs.iter().cloned());
        let dictionaries: Vec<_> = capabilities
            .iter()
            .filter(|(id, _)| id == "dictionary.read")
            .cloned()
            .collect();
        let dictionary = self
            .dictionary_bindings
            .plans(&dictionaries, &resource_configs)?;
        let node = self.verify_node(&package, configs, gate, cancel)?;
        let node = node
            .bind_package(package.authorization.clone())
            .bind_dictionary(dictionary)
            .bind_network(network);
        Ok(VerifiedPackage { package, node })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Package {
        let module = b"\0asm\x01\0\0\0";
        let manifest = Manifest {
            package_id: "test.node".into(),
            version: Version::parse("1.0.0").unwrap(),
            display_name: "試験".into(),
            runtime: RuntimeKind::Wasm32,
            guest_api_version: 1,
            node: NodeDefinition::new("test.node", 1, vec![], vec![]),
            module_sha256: digest(module).unwrap(),
            capabilities: vec![],
        };
        Package::encode(manifest, module, &PackageLimits::default()).unwrap()
    }
    #[test]
    fn framing_rejects_truncation_trailing_bytes_lengths_version_and_payload_mutation() {
        let package = sample();
        let limits = PackageLimits::default();
        for length in [0, 7, 23, 24, package.bytes().len() - 1] {
            assert!(Package::parse(package.bytes()[..length].to_vec(), &limits).is_err());
        }
        for offset in [0, 8, 12, 16, package.bytes().len() - 1] {
            let mut bytes = package.bytes().to_vec();
            bytes[offset] ^= 0xff;
            assert!(Package::parse(bytes, &limits).is_err(), "{offset}");
        }
        let mut trailing = package.bytes().to_vec();
        trailing.push(0);
        assert!(Package::parse(trailing, &limits).is_err());
        assert!(
            Package::read(
                package.bytes(),
                &PackageLimits {
                    package_bytes: 24,
                    ..limits
                }
            )
            .is_err()
        );
    }
    #[test]
    fn identity_is_code_descriptor_capabilities_bound_and_metadata_independent() {
        let package = sample();
        let mut manifest = package.manifest().clone();
        manifest.version = Version::parse("2.0.0+label").unwrap();
        manifest.display_name = "renamed".into();
        assert_eq!(
            manifest.authorization_digest().unwrap(),
            package.authorization_digest()
        );
        manifest.capabilities.push(CapabilityRequest {
            id: "dictionary.read".into(),
            binding: "selected".into(),
        });
        assert_ne!(
            manifest.authorization_digest().unwrap(),
            package.authorization_digest()
        );
        manifest.capabilities.push(CapabilityRequest {
            id: "network.request".into(),
            binding: "profile".into(),
        });
        let hash = manifest.authorization_digest().unwrap();
        manifest.capabilities.reverse();
        assert_eq!(manifest.authorization_digest().unwrap(), hash);
        manifest.capabilities.push(manifest.capabilities[0].clone());
        assert!(manifest.validate().is_err());
    }
    #[test]
    fn strict_manifest_refuses_native_unknown_fields_paths_system_nodes_and_invalid_api() {
        let package = sample();
        let mut value = serde_json::to_value(package.manifest()).unwrap();
        value["trusted"] = true.into();
        assert!(serde_json::from_value::<Manifest>(value).is_err());
        let mut value = serde_json::to_value(package.manifest()).unwrap();
        value["runtime"] = "native".into();
        assert!(serde_json::from_value::<Manifest>(value).is_err());
        for id in ["../escape", "C:\\escape", "https://host", "a/b", ""] {
            let mut manifest = package.manifest().clone();
            manifest.package_id = id.into();
            assert!(manifest.validate().is_err());
        }
        for kind in ["input.text", "demo.packet", "e2e.input", "e2e.output"] {
            let mut manifest = package.manifest().clone();
            manifest.node.kind = kind.into();
            assert!(manifest.validate().is_err());
        }
        let mut manifest = package.manifest().clone();
        manifest.guest_api_version = 2;
        assert!(manifest.validate().is_err());
    }
}
