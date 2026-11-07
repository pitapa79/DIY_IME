mod format;
pub use format::{
    CapabilityRequest, Manifest, Package, PackageLimits, PackageVerifier, RuntimeKind,
    VerifiedInstallation, VerifiedPackage,
};
mod catalog;
pub use catalog::{
    Catalog, CatalogEntry, CatalogSnapshot, ChangeKind, ImportChange, ImportConfirmation,
    ImportPlan, ImportReview,
};
/// Producer helper: lowercase SHA256 of exact binary Wasm bytes.
pub fn module_sha256(bytes: &[u8]) -> Result<String, crate::node::CoreError> {
    format::digest(bytes)
}
pub mod execution_host;
pub mod manager;
pub mod network;
pub mod permissions;
pub mod secrets;

mod official;
pub use official::{OfficialGraphReview, OfficialStage, StaticOfficialPipeline};
