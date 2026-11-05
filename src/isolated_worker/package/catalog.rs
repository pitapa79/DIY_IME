//! Host-owned installed index. Installation never grants execution authority.
use super::{
    Package, PackageLimits, PackageVerifier, VerifiedInstallation,
    format::{digest, is_hash},
};
use crate::{
    execution_control::CancellationToken, isolated_worker::adapter::ExecutionGate, node::CoreError,
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

fn err(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
fn ioerr(_: std::io::Error) -> CoreError {
    err("catalog-io", "Host package storage operation failed")
}
pub(super) fn regular(path: &Path, directory: bool) -> Result<(), CoreError> {
    let metadata = fs::symlink_metadata(path).map_err(ioerr)?;
    if metadata.file_attributes() & 0x400 != 0
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(err(
            "catalog-path",
            "unexpected storage object or reparse point",
        ));
    }
    Ok(())
}
pub(super) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, CoreError> {
    regular(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)
        .map_err(ioerr)?;
    if file.metadata().map_err(ioerr)?.file_attributes() & 0x400 != 0 {
        return Err(err("catalog-path", "reparse point refused"));
    }
    let mut bytes = Vec::new();
    file.take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(ioerr)?;
    if bytes.len() > limit {
        return Err(err(
            "catalog-limit",
            "storage object exceeds selected limit",
        ));
    }
    Ok(bytes)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogEntry {
    pub manifest: super::Manifest,
    pub package_sha256: String,
    pub authorization_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSnapshot {
    pub format_version: u32,
    /// Decimal string, never a rounded JavaScript number.
    pub revision: String,
    pub entries: Vec<CatalogEntry>,
}
impl CatalogSnapshot {
    fn empty() -> Self {
        Self {
            format_version: 1,
            revision: "0".into(),
            entries: vec![],
        }
    }
    fn validate(&self) -> Result<u64, CoreError> {
        let revision = self
            .revision
            .parse::<u64>()
            .map_err(|_| err("catalog-index", "invalid revision"))?;
        if self.format_version != 1 || revision.to_string() != self.revision {
            return Err(err("catalog-index", "unsupported index/revision"));
        }
        let mut ids = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        for entry in &self.entries {
            entry.manifest.validate()?;
            if !is_hash(&entry.package_sha256)
                || entry.manifest.authorization_digest()? != entry.authorization_sha256
                || !ids.insert(&entry.manifest.package_id)
                || !kinds.insert(&entry.manifest.node.kind)
            {
                return Err(err(
                    "catalog-index",
                    "invalid digest or duplicate identity/kind",
                ));
            }
        }
        Ok(revision)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    New,
    Unchanged,
    Metadata,
    Upgrade,
    Downgrade,
    SameVersionRebuild,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportChange {
    pub package_id: String,
    pub node_kind: String,
    pub previous_version: Option<String>,
    pub version: String,
    pub change: ChangeKind,
    pub requires_permission_review: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReview {
    pub expected_revision: String,
    pub changes: Vec<ImportChange>,
}
/// Opaque proof tied to this catalog and its exact prior contents.
/// GUI JSON cannot create or modify this plan.
pub struct ImportPlan {
    root: PathBuf,
    prior_digest: String,
    prior_revision: u64,
    next: CatalogSnapshot,
    packages: Vec<VerifiedInstallation>,
    review: ImportReview,
}
impl ImportPlan {
    pub fn review(&self) -> &ImportReview {
        &self.review
    }
}
#[derive(Default)]
pub struct ImportConfirmation {
    pub allow_downgrade: bool,
    pub allow_same_version_rebuild: bool,
}
pub struct Catalog {
    root: PathBuf,
    limits: PackageLimits,
}
impl Catalog {
    /// Host selects and creates the data directory, never package metadata.
    pub fn open(root: &Path, limits: PackageLimits) -> Result<Self, CoreError> {
        let root = root.canonicalize().map_err(ioerr)?;
        regular(&root, true)?;
        Ok(Self { root, limits })
    }
    fn load(&self) -> Result<(CatalogSnapshot, String), CoreError> {
        regular(&self.root, true)?;
        let path = self.root.join("catalog.json");
        let bytes = match fs::symlink_metadata(&path) {
            Ok(_) => read_bounded(&path, self.limits.catalog_bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(ioerr(e)),
        };
        let snapshot = if bytes.is_empty() {
            if path.exists() {
                return Err(err("catalog-index", "empty existing catalog"));
            }
            CatalogSnapshot::empty()
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|_| err("catalog-index", "invalid catalog fields"))?
        };
        snapshot.validate()?;
        Ok((snapshot, digest(&bytes)?))
    }
    pub fn snapshot(&self) -> Result<CatalogSnapshot, CoreError> {
        Ok(self.load()?.0)
    }
    pub fn read_installed(&self, package_id: &str) -> Result<Package, CoreError> {
        let (snapshot, _) = self.load()?;
        let entry = snapshot
            .entries
            .iter()
            .find(|e| e.manifest.package_id == package_id)
            .ok_or_else(|| err("catalog-missing", "package not installed"))?;
        regular(&self.root.join("objects"), true)?;
        let bytes = read_bounded(
            &self.object_path(&entry.package_sha256),
            self.limits.package_bytes,
        )?;
        let package = Package::parse(bytes, &self.limits)?;
        if package.digest() != entry.package_sha256
            || serde_json::to_vec(package.manifest()).ok()
                != serde_json::to_vec(&entry.manifest).ok()
        {
            return Err(err("catalog-object", "installed object differs from index"));
        }
        Ok(package)
    }
    /// No disk mutation. Every candidate is checked in an actual isolated Worker.
    pub fn review(
        &self,
        packages: Vec<Package>,
        verifier: &PackageVerifier,
        gate: ExecutionGate,
        cancel: CancellationToken,
    ) -> Result<ImportPlan, CoreError> {
        let total = packages
            .iter()
            .try_fold(0usize, |n, p| n.checked_add(p.bytes().len()))
            .ok_or_else(|| err("catalog-limit", "batch length overflow"))?;
        if total > self.limits.batch_bytes || packages.is_empty() {
            return Err(err("catalog-limit", "empty/oversized import batch"));
        }
        let (prior, prior_digest) = self.load()?;
        let revision = prior.validate()?;
        let mut next: BTreeMap<String, CatalogEntry> = prior
            .entries
            .iter()
            .cloned()
            .map(|e| (e.manifest.package_id.clone(), e))
            .collect();
        let mut seen = BTreeSet::new();
        let mut verified = Vec::new();
        let mut changes = Vec::new();
        for package in packages {
            if cancel.is_cancelled() {
                return Err(err("catalog-cancelled", "import cancelled"));
            }
            let m = package.manifest();
            if !seen.insert(m.package_id.clone()) {
                return Err(err("catalog-duplicate", "duplicate package ID in import"));
            }
            let old = next.get(&m.package_id);
            let authorization_changed =
                old.is_none_or(|e| e.authorization_sha256 != package.authorization_digest());
            let change = classify(old, &package);
            changes.push(ImportChange {
                package_id: m.package_id.clone(),
                node_kind: m.node.kind.clone(),
                previous_version: old.map(|e| e.manifest.version.to_string()),
                version: m.version.to_string(),
                change,
                requires_permission_review: authorization_changed,
            });
            next.insert(
                m.package_id.clone(),
                CatalogEntry {
                    manifest: m.clone(),
                    package_sha256: package.digest().into(),
                    authorization_sha256: package.authorization_digest().into(),
                },
            );
            verified.push(verifier.verify_installation(package, gate.clone(), cancel.clone())?);
        }
        let next = CatalogSnapshot {
            format_version: 1,
            revision: revision
                .checked_add(1)
                .ok_or_else(|| err("catalog-index", "revision exhausted"))?
                .to_string(),
            entries: next.into_values().collect(),
        };
        next.validate()?;
        let encoded =
            serde_json::to_vec(&next).map_err(|_| err("catalog-index", "index encoding failed"))?;
        if encoded.len() > self.limits.catalog_bytes {
            return Err(err("catalog-limit", "new catalog exceeds selected limit"));
        }
        Ok(ImportPlan {
            root: self.root.clone(),
            prior_digest,
            prior_revision: revision,
            next,
            packages: verified,
            review: ImportReview {
                expected_revision: prior.revision,
                changes,
            },
        })
    }
    fn object_path(&self, sha: &str) -> PathBuf {
        self.root.join("objects").join(format!("{sha}.diynode"))
    }
    /// Atomic index visibility only; no promise of power-loss durability.
    /// Objects may remain unreferenced after a failed commit, and are never executed.
    pub fn commit(
        &self,
        plan: ImportPlan,
        confirmation: ImportConfirmation,
    ) -> Result<CatalogSnapshot, CoreError> {
        if plan.root != self.root {
            return Err(err("catalog-plan", "plan belongs to another catalog"));
        }
        for change in &plan.review.changes {
            if (change.change == ChangeKind::Downgrade && !confirmation.allow_downgrade)
                || (change.change == ChangeKind::SameVersionRebuild
                    && !confirmation.allow_same_version_rebuild)
            {
                return Err(err(
                    "catalog-confirmation",
                    "downgrade/rebuild requires explicit confirmation",
                ));
            }
        }
        regular(&self.root, true)?;
        let _lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(0x00200000)
            .open(self.root.join("catalog.lock"))
            .map_err(|_| err("catalog-busy", "catalog is locked/unavailable"))?;
        if _lock.metadata().map_err(ioerr)?.file_attributes() & 0x400 != 0 {
            return Err(err("catalog-path", "lock reparse point refused"));
        }
        let (current, current_digest) = self.load()?;
        if current_digest != plan.prior_digest || current.validate()? != plan.prior_revision {
            return Err(err(
                "catalog-conflict",
                "catalog changed; review import again",
            ));
        }
        let objects = self.root.join("objects");
        match fs::create_dir(&objects) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(ioerr(e)),
        }
        regular(&objects, true)?;
        for verified in &plan.packages {
            let package = verified.package();
            let path = self.object_path(package.digest());
            if path.try_exists().map_err(ioerr)? {
                let existing = read_bounded(&path, self.limits.package_bytes)?;
                if digest(&existing)? != package.digest() {
                    return Err(err("catalog-object", "existing object digest mismatch"));
                }
            } else {
                let temp = write_temp(&objects, package.bytes())?;
                if let Err(error) = replace(&temp, &path) {
                    let _ = fs::remove_file(&temp);
                    return Err(error);
                }
            }
        }
        let bytes = serde_json::to_vec(&plan.next)
            .map_err(|_| err("catalog-index", "index encoding failed"))?;
        let temp = write_temp(&self.root, &bytes)?;
        if let Err(error) = replace(&temp, self.root.join("catalog.json").as_path()) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        Ok(plan.next)
    }
}
fn classify(old: Option<&CatalogEntry>, new: &Package) -> ChangeKind {
    let Some(old) = old else {
        return ChangeKind::New;
    };
    if old.package_sha256 == new.digest() {
        return ChangeKind::Unchanged;
    }
    match new.manifest().version.cmp_precedence(&old.manifest.version) {
        Ordering::Less => ChangeKind::Downgrade,
        Ordering::Greater => ChangeKind::Upgrade,
        Ordering::Equal if old.authorization_sha256 != new.authorization_digest() => {
            ChangeKind::SameVersionRebuild
        }
        Ordering::Equal => ChangeKind::Metadata,
    }
}
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
pub(super) fn write_temp(root: &Path, bytes: &[u8]) -> Result<PathBuf, CoreError> {
    for _ in 0..128 {
        let serial = NEXT_TEMP.fetch_add(1, AtomicOrdering::Relaxed);
        let path = root.join(format!(".catalog-{}-{serial}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(ioerr(error));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(ioerr(e)),
        }
    }
    Err(err("catalog-temp", "temporary catalog names unavailable"))
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
}
pub(super) fn replace(from: &Path, to: &Path) -> Result<(), CoreError> {
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(ioerr(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(version: &str, changed: bool) -> Package {
        let module = if changed {
            b"\0asm\x01\0\0\0\0\x02\x01x".as_slice()
        } else {
            b"\0asm\x01\0\0\0".as_slice()
        };
        Package::encode(
            super::super::Manifest {
                package_id: "fixture".into(),
                version: version.parse().unwrap(),
                display_name: "Fixture".into(),
                runtime: super::super::RuntimeKind::Wasm32,
                guest_api_version: 1,
                node: crate::node::NodeDefinition::new("fixture", 1, vec![], vec![]),
                module_sha256: digest(module).unwrap(),
                capabilities: vec![],
            },
            module,
            &PackageLimits::default(),
        )
        .unwrap()
    }
    fn entry(p: &Package) -> CatalogEntry {
        CatalogEntry {
            manifest: p.manifest().clone(),
            package_sha256: p.digest().into(),
            authorization_sha256: p.authorization_digest().into(),
        }
    }
    #[test]
    fn version_order_build_labels_and_code_changes_do_not_conflate() {
        let original = package("1.0.0+one", false);
        let old = entry(&original);
        assert_eq!(classify(None, &original), ChangeKind::New);
        assert_eq!(classify(Some(&old), &original), ChangeKind::Unchanged);
        assert_eq!(
            classify(Some(&old), &package("1.0.0+two", false)),
            ChangeKind::Metadata
        );
        assert_eq!(
            classify(Some(&old), &package("1.0.0+two", true)),
            ChangeKind::SameVersionRebuild
        );
        assert_eq!(
            classify(Some(&old), &package("1.0.1", false)),
            ChangeKind::Upgrade
        );
        assert_eq!(
            classify(Some(&old), &package("1.0.0-rc.1", false)),
            ChangeKind::Downgrade
        );
    }
    #[test]
    fn index_rejects_noncanonical_revision_duplicate_kind_and_forged_authorization() {
        let item = entry(&package("1.0.0", false));
        for revision in ["01", "-1", "18446744073709551616", "1.0"] {
            assert!(
                CatalogSnapshot {
                    format_version: 1,
                    revision: revision.into(),
                    entries: vec![]
                }
                .validate()
                .is_err()
            );
        }
        let mut snapshot = CatalogSnapshot {
            format_version: 1,
            revision: "1".into(),
            entries: vec![item.clone(), item],
        };
        assert!(snapshot.validate().is_err());
        snapshot.entries.pop();
        snapshot.entries[0].authorization_sha256 = "0".repeat(64);
        assert!(snapshot.validate().is_err());
    }
}
