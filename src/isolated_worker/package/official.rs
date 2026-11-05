//! Fixed distribution data, not a package-supplied trust assertion or grant.
use super::{
    Catalog, CatalogSnapshot, ChangeKind, ImportConfirmation, Package, PackageLimits,
    PackageVerifier,
    catalog::{read_bounded, regular},
    format::{FORMAT_VERSION, MAGIC, digest},
};
use crate::{
    execution_control::CancellationToken, isolated_worker::adapter::ExecutionGate, node::CoreError,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

const PIN: &str = include_str!("../../../distribution/official/pin.json");
const MAX_INDEX: usize = 1024 * 1024;
const BATCH_COUNT: usize = 16;
fn error(message: &str) -> CoreError {
    CoreError::new("official-distribution", message)
}
fn cancelled(cancel: &CancellationToken) -> Result<(), CoreError> {
    if cancel.is_cancelled() {
        Err(CoreError::new(
            "official-cancelled",
            "official package setup cancelled",
        ))
    } else {
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pin {
    format_version: u32,
    source_repository: String,
    source_commit: String,
    module_sha256: String,
    index_sha256: String,
    receipt_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Source {
    repository: String,
    commit: String,
    cargo_lock_sha256: String,
    inventory_sha256: String,
    migration_provenance_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Module {
    bytes: usize,
    guest_api_version: u32,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    file: String,
    package_id: String,
    kind: String,
    node_version: u32,
    classification: String,
    status: String,
    sha256: String,
    bytes: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Index {
    format_version: u32,
    source: Source,
    module: Module,
    package_bytes: usize,
    packages: Vec<Entry>,
}
struct Bundle {
    source_commit: String,
    packages: Vec<Package>,
}
fn simple_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 256
        && !kind.starts_with('.')
        && !kind.ends_with('.')
        && kind
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}
impl Bundle {
    fn capture(
        directory: &Path,
        limits: &PackageLimits,
        cancel: &CancellationToken,
    ) -> Result<Self, CoreError> {
        cancelled(cancel)?;
        let pin: Pin =
            serde_json::from_str(PIN).map_err(|_| error("invalid compiled distribution pin"))?;
        if pin.format_version != 1
            || pin.source_repository != "pitapa79/DIY_IME2_node"
            || pin.source_commit.len() != 40
            || !pin
                .source_commit
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(error("unsupported compiled distribution pin"));
        }
        let directory = directory
            .canonicalize()
            .map_err(|_| error("distribution directory unavailable"))?;
        regular(&directory, true)?;
        let index_bytes = read_bounded(&directory.join("official-index.json"), MAX_INDEX)?;
        let receipt = read_bounded(&directory.join("build-receipt.json"), MAX_INDEX)?;
        if digest(&index_bytes)? != pin.index_sha256 || digest(&receipt)? != pin.receipt_sha256 {
            return Err(error(
                "distribution index or build receipt differs from compiled pin",
            ));
        }
        let index: Index =
            serde_json::from_slice(&index_bytes).map_err(|_| error("invalid pinned index"))?;
        if index.format_version != 1
            || index.source.repository != pin.source_repository
            || index.source.commit != pin.source_commit
            || index.module.sha256 != pin.module_sha256
            || index.module.guest_api_version != 1
            || index.packages.len() != 48
            || [
                &index.source.cargo_lock_sha256,
                &index.source.inventory_sha256,
                &index.source.migration_provenance_sha256,
            ]
            .iter()
            .any(|h| !super::format::is_hash(h))
        {
            return Err(error("unsupported pinned distribution identity"));
        }
        let module = read_bounded(
            &directory.join("official.wasm"),
            limits.module_bytes.min(64 * 1024 * 1024),
        )?;
        if module.len() != index.module.bytes || digest(&module)? != pin.module_sha256 {
            return Err(error("distribution module differs from compiled pin"));
        }
        let mut packages = Vec::new();
        let mut kinds = BTreeSet::new();
        let mut total = 0usize;
        let mut prior = String::new();
        for entry in index.packages {
            cancelled(cancel)?;
            if !simple_kind(&entry.kind)
                || entry.file != format!("{}.diynode", entry.kind)
                || entry.file <= prior
                || entry.package_id != format!("official.{}", entry.kind)
                || !kinds.insert(entry.kind.clone())
                || !matches!(
                    entry.classification.as_str(),
                    "primitive" | "standard-function"
                )
                || !matches!(
                    entry.status.as_str(),
                    "compatibility" | "experimental" | "practical"
                )
            {
                return Err(error("invalid pinned package inventory"));
            }
            prior = entry.file;
            let manifest = read_bounded(
                &directory.join(format!("{}.manifest.json", entry.kind)),
                limits.manifest_bytes.min(1024 * 1024),
            )?;
            let manifest_len =
                u32::try_from(manifest.len()).map_err(|_| error("manifest length overflow"))?;
            let size = 24usize
                .checked_add(manifest.len())
                .and_then(|n| n.checked_add(module.len()))
                .ok_or_else(|| error("frame size overflow"))?;
            total = total
                .checked_add(size)
                .ok_or_else(|| error("distribution size overflow"))?;
            if size != entry.bytes || size > limits.package_bytes || total > 512 * 1024 * 1024 {
                return Err(error("distribution exceeds selected bounds"));
            }
            // Preserve original manifest bytes/order: the complete frame digest is pinned.
            let mut frame = Vec::with_capacity(size);
            frame.extend_from_slice(MAGIC);
            frame.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
            frame.extend_from_slice(&manifest_len.to_le_bytes());
            frame.extend_from_slice(&(module.len() as u64).to_le_bytes());
            frame.extend_from_slice(&manifest);
            frame.extend_from_slice(&module);
            let package = Package::parse(frame, limits)?;
            let metadata = package.manifest();
            if package.digest() != entry.sha256
                || metadata.package_id != entry.package_id
                || metadata.node.kind != entry.kind
                || metadata.node.version != entry.node_version
                || metadata.module_sha256 != pin.module_sha256
            {
                return Err(error("pinned package frame or definition differs"));
            }
            packages.push(package);
        }
        if total != index.package_bytes {
            return Err(error("pinned aggregate size differs"));
        }
        Ok(Self {
            source_commit: pin.source_commit,
            packages,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapConflict {
    pub package_id: String,
    pub node_kind: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapReport {
    pub source_commit: String,
    pub installed: Vec<String>,
    pub present: Vec<String>,
    pub pending: Vec<String>,
    pub conflicts: Vec<BootstrapConflict>,
    pub complete: bool,
    pub execution_permission_granted: bool,
}
enum Disposition {
    Present,
    Missing,
    Conflict,
}
fn disposition(package: &Package, snapshot: &CatalogSnapshot) -> Disposition {
    let m = package.manifest();
    if let Some(entry) = snapshot
        .entries
        .iter()
        .find(|e| e.manifest.package_id == m.package_id)
    {
        if entry.package_sha256 == package.digest() {
            Disposition::Present
        } else {
            Disposition::Conflict
        }
    } else if snapshot
        .entries
        .iter()
        .any(|e| e.manifest.node.kind == m.node.kind)
    {
        Disposition::Conflict
    } else {
        Disposition::Missing
    }
}
/// One bounded setup step. It adds only missing identities, never replaces an entry.
/// Call again until complete or conflicts require an explicit user import review.
/// Cancellation may leave earlier committed batches; restarting preserves them.
pub(super) fn bootstrap_batch(
    directory: &Path,
    catalog: &Catalog,
    verifier: &PackageVerifier,
    limits: &PackageLimits,
    gate: ExecutionGate,
    cancel: CancellationToken,
) -> Result<BootstrapReport, CoreError> {
    let bundle = Bundle::capture(directory, limits, &cancel)?;
    let snapshot = catalog.snapshot()?;
    let mut report = BootstrapReport {
        source_commit: bundle.source_commit,
        installed: vec![],
        present: vec![],
        pending: vec![],
        conflicts: vec![],
        complete: false,
        execution_permission_granted: false,
    };
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    for package in bundle.packages {
        cancelled(&cancel)?;
        let m = package.manifest();
        match disposition(&package, &snapshot) {
            Disposition::Present => {
                // The index alone is insufficient; verify the exact stored object too.
                let stored = catalog.read_installed(&m.package_id)?;
                if stored.digest() != package.digest() {
                    return Err(error("installed distribution object differs"));
                }
                report.present.push(m.package_id.clone());
            }
            Disposition::Conflict => report.conflicts.push(BootstrapConflict {
                package_id: m.package_id.clone(),
                node_kind: m.node.kind.clone(),
            }),
            Disposition::Missing => {
                if package.bytes().len() > limits.batch_bytes {
                    return Err(error("package exceeds selected review batch bound"));
                }
                if selected.len() < BATCH_COUNT
                    && bytes
                        .checked_add(package.bytes().len())
                        .is_some_and(|n| n <= limits.batch_bytes)
                {
                    bytes += package.bytes().len();
                    report.installed.push(m.package_id.clone());
                    selected.push(package);
                } else {
                    report.pending.push(m.package_id.clone());
                }
            }
        }
    }
    if !selected.is_empty() {
        let plan = catalog.review(selected, verifier, gate, cancel.clone())?;
        // A concurrent import between snapshot and review may have claimed an ID.
        // Never use bootstrap as implicit upgrade/rebuild/unchanged confirmation.
        if plan
            .review()
            .changes
            .iter()
            .any(|c| c.change != ChangeKind::New)
        {
            return Err(CoreError::new(
                "official-conflict",
                "catalog changed; restart official setup",
            ));
        }
        cancelled(&cancel)?;
        catalog.commit(plan, ImportConfirmation::default())?;
    }
    report.complete = report.pending.is_empty() && report.conflicts.is_empty();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_vendored_inventory_reconstructs_all_original_frames_without_grants() {
        let bundle = Bundle::capture(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/distribution/official"
            )),
            &PackageLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(bundle.packages.len(), 48);
        assert_eq!(
            bundle.source_commit,
            "ed6c97cd8bc22a2a26b4bc71c141fbc4f20e936e"
        );
        assert_eq!(
            bundle
                .packages
                .iter()
                .map(|p| p.bytes().len())
                .sum::<usize>(),
            49_207_129
        );
        assert!(bundle.packages.iter().all(|p| {
            p.manifest().capabilities.iter().all(|c| {
                c.id == "dictionary.read"
                    && c.binding == "dictionary"
                    && p.manifest().node.kind == "text.dictionary-exact"
            })
        }));
    }
    #[test]
    fn cancelled_and_tighter_bounds_cannot_capture_distribution() {
        let directory = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/distribution/official"
        ));
        let token = CancellationToken::default();
        token.cancel();
        assert_eq!(
            Bundle::capture(directory, &PackageLimits::default(), &token)
                .err()
                .unwrap()
                .code,
            "official-cancelled"
        );
        let limits = PackageLimits {
            module_bytes: 64,
            ..PackageLimits::default()
        };
        assert!(Bundle::capture(directory, &limits, &CancellationToken::default()).is_err());
    }
    #[test]
    fn existing_ids_and_kinds_are_preserved_and_exact_matches_are_only_data_presence() {
        use super::super::CatalogEntry;
        let bundle = Bundle::capture(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/distribution/official"
            )),
            &PackageLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap();
        let package = &bundle.packages[0];
        let mut snapshot = CatalogSnapshot {
            format_version: 1,
            revision: "0".into(),
            entries: vec![],
        };
        assert!(matches!(
            disposition(package, &snapshot),
            Disposition::Missing
        ));
        snapshot.entries.push(CatalogEntry {
            manifest: package.manifest().clone(),
            package_sha256: package.digest().into(),
            authorization_sha256: package.authorization_digest().into(),
        });
        assert!(matches!(
            disposition(package, &snapshot),
            Disposition::Present
        ));
        snapshot.entries[0].package_sha256 = "0".repeat(64);
        assert!(matches!(
            disposition(package, &snapshot),
            Disposition::Conflict
        ));
        snapshot.entries[0].manifest.package_id = "custom.same-kind".into();
        assert!(matches!(
            disposition(package, &snapshot),
            Disposition::Conflict
        ));
    }
    #[test]
    fn an_imported_index_cannot_redeclare_itself_official() {
        use std::fs;
        let root = std::env::temp_dir().join(format!(
            "diyime-official-pin-{}",
            super::super::manager::nonce().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let source = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/distribution/official"
        ));
        fs::copy(
            source.join("build-receipt.json"),
            root.join("build-receipt.json"),
        )
        .unwrap();
        fs::write(
            root.join("official-index.json"),
            br#"{"formatVersion":1,"official":true}"#,
        )
        .unwrap();
        let result = Bundle::capture(
            &root,
            &PackageLimits::default(),
            &CancellationToken::default(),
        );
        assert_eq!(result.err().unwrap().code, "official-distribution");
        fs::remove_file(root.join("build-receipt.json")).unwrap();
        fs::remove_file(root.join("official-index.json")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
