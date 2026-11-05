//! Real AppContainer package verification and atomic catalog integration.
#[cfg(all(windows, target_arch = "x86_64"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use diy_ime2_core::{
        execution_control::{CancellationToken, DeferredNode},
        isolated_worker::{
            adapter::{ExecutionGate, WasmArtifact, WorkerPolicy},
            package::{
                Catalog, ChangeKind, ImportConfirmation, Manifest, Package, PackageLimits,
                PackageVerifier, RuntimeKind,
            },
        },
    };
    use std::{fs, path::PathBuf};
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: package_probe WORKER DLL FIXTURE_WASM".into());
    }
    let root = std::env::temp_dir().join(format!("diyime-package-probe-{}", std::process::id()));
    fs::create_dir(&root)?;
    // Only synthetic packages live here; leave diagnostics on failure.
    let workspace = root.join("workers");
    fs::create_dir(&workspace)?;
    let data = root.join("data");
    fs::create_dir(&data)?;
    let module = fs::read(&args[3])?;
    let policy = WorkerPolicy::bounded_probe();
    let runtime = WasmArtifact::capture(
        &workspace,
        &PathBuf::from(&args[1]),
        &PathBuf::from(&args[2]),
        module.clone(),
        policy.clone(),
    )?;
    let gate = ExecutionGate::default();
    let actual = runtime
        .prepare(
            "fixture.wasm",
            1,
            vec![],
            gate.clone(),
            CancellationToken::default(),
        )?
        .definition();
    let limits = PackageLimits::default();
    // SHA256 is recorded by the producer; use Windows CNG through certutil-free Host helper.
    let module_hash = diy_ime2_core::isolated_worker::package::module_sha256(&module)?;
    let manifest = Manifest {
        package_id: "test.fixture".into(),
        version: "1.0.0".parse()?,
        display_name: "Fixture".into(),
        runtime: RuntimeKind::Wasm32,
        guest_api_version: 1,
        node: actual,
        module_sha256: module_hash,
        capabilities: vec![],
    };
    let cli = PathBuf::from(&args[1])
        .parent()
        .unwrap()
        .join("diy-ime2-node-package.exe");
    let manifest_path = root.join("manifest.json");
    let packed = root.join("fixture.diynode");
    fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    let output = std::process::Command::new(&cli)
        .arg("pack")
        .arg(&manifest_path)
        .arg(&args[3])
        .arg(&packed)
        .output()?;
    assert!(
        output.status.success(),
        "pack: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(&cli)
        .arg("inspect")
        .arg(&packed)
        .output()?;
    assert!(output.status.success());
    let inspected: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(inspected["isolatedVerified"], false);
    assert!(
        !std::process::Command::new(&cli)
            .arg("pack")
            .arg(&manifest_path)
            .arg(&args[3])
            .arg(&packed)
            .output()?
            .status
            .success()
    );
    let output = std::process::Command::new(&cli)
        .arg("import")
        .arg(root.join("cli-data"))
        .arg(root.join("cli-workers"))
        .arg(&args[1])
        .arg(&args[2])
        .arg(&packed)
        .arg("--commit")
        .output()?;
    assert!(
        output.status.success(),
        "import: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8(output.stdout)?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        assert_eq!(value["executionPermissionGranted"], false);
    }
    let make = |manifest: Manifest| Package::encode(manifest, &module, &limits);
    let verifier = PackageVerifier::new(runtime.clone(), policy);
    let catalog = Catalog::open(&data, limits.clone())?;
    let review = |packages| {
        catalog.review(
            packages,
            &verifier,
            gate.clone(),
            CancellationToken::default(),
        )
    };
    let package = make(manifest.clone())?;
    let original = package.digest().to_string();
    let first = review(vec![package.clone()])?;
    assert!(!data.join("catalog.json").exists());
    assert!(!data.join("objects").exists());
    assert_eq!(first.review().changes[0].change, ChangeKind::New);
    assert!(first.review().changes[0].requires_permission_review);
    let stale = review(vec![package.clone()])?;
    assert_eq!(
        catalog
            .commit(first, ImportConfirmation::default())?
            .revision,
        "1"
    );
    assert!(
        catalog
            .commit(stale, ImportConfirmation::default())
            .is_err()
    );
    assert_eq!(catalog.read_installed("test.fixture")?.digest(), original);
    assert!(review(vec![package.clone(), package.clone()]).is_err());
    let unchanged = review(vec![package.clone()])?;
    assert_eq!(unchanged.review().changes[0].change, ChangeKind::Unchanged);
    let mut alternate = manifest.clone();
    alternate.package_id = "test.alternate".into();
    alternate.node = runtime
        .prepare(
            "fixture.wasm.alt",
            1,
            vec![],
            gate.clone(),
            CancellationToken::default(),
        )?
        .definition();
    let multi = review(vec![package.clone(), make(alternate.clone())?])?;
    catalog.commit(multi, ImportConfirmation::default())?;
    assert_eq!(catalog.snapshot()?.entries.len(), 2);
    let mut collision = alternate;
    collision.package_id = "test.collision".into();
    assert!(review(vec![make(collision)?]).is_err());
    let busy = review(vec![package.clone()])?;
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(data.join("catalog.lock"))?;
        assert!(catalog.commit(busy, ImportConfirmation::default()).is_err());
    }
    let mut renamed = manifest.clone();
    renamed.display_name = "renamed".into();
    let metadata = review(vec![make(renamed)?])?;
    assert_eq!(metadata.review().changes[0].change, ChangeKind::Metadata);
    assert!(!metadata.review().changes[0].requires_permission_review);
    // Malformed batch must leave the exact old catalog visible.
    let before = fs::read(data.join("catalog.json"))?;
    let mut wrong = manifest.clone();
    wrong.node.default_config = serde_json::json!({"mode":"noOutput"});
    assert!(review(vec![package.clone(), make(wrong.clone())?]).is_err());
    assert!(review(vec![make(wrong)?]).is_err());
    assert_eq!(fs::read(data.join("catalog.json"))?, before);
    let mut unsupported = manifest.clone();
    unsupported
        .capabilities
        .push(diy_ime2_core::isolated_worker::package::CapabilityRequest {
            id: "network.unknown".into(),
            binding: "selected".into(),
        });
    assert!(review(vec![make(unsupported)?]).is_err());
    let mut upgraded = manifest.clone();
    upgraded.version = "2.0.0".parse()?;
    let upgrade = review(vec![make(upgraded)?])?;
    assert_eq!(upgrade.review().changes[0].change, ChangeKind::Upgrade);
    catalog.commit(upgrade, ImportConfirmation::default())?;
    let downgrade = review(vec![package.clone()])?;
    assert_eq!(downgrade.review().changes[0].change, ChangeKind::Downgrade);
    assert!(
        catalog
            .commit(downgrade, ImportConfirmation::default())
            .is_err()
    );
    assert_eq!(
        catalog
            .snapshot()?
            .entries
            .iter()
            .find(|e| e.manifest.package_id == "test.fixture")
            .unwrap()
            .manifest
            .version
            .to_string(),
        "2.0.0"
    );
    catalog.commit(
        review(vec![package.clone()])?,
        ImportConfirmation {
            allow_downgrade: true,
            ..Default::default()
        },
    )?;
    // Valid different Wasm bytes: custom section, unchanged behavior/version.
    let mut rebuilt = module.clone();
    rebuilt.extend_from_slice(&[0, 2, 1, b'x']);
    let mut rebuild_manifest = manifest.clone();
    rebuild_manifest.module_sha256 =
        diy_ime2_core::isolated_worker::package::module_sha256(&rebuilt)?;
    let rebuild = Package::encode(rebuild_manifest, &rebuilt, &limits)?;
    let planned = review(vec![rebuild.clone()])?;
    assert_eq!(
        planned.review().changes[0].change,
        ChangeKind::SameVersionRebuild
    );
    assert!(planned.review().changes[0].requires_permission_review);
    assert!(
        catalog
            .commit(planned, ImportConfirmation::default())
            .is_err()
    );
    catalog.commit(
        review(vec![rebuild.clone()])?,
        ImportConfirmation {
            allow_same_version_rebuild: true,
            ..Default::default()
        },
    )?;
    assert_eq!(
        catalog.read_installed("test.fixture")?.digest(),
        rebuild.digest()
    );
    // Installation evidence cannot become runtime authority. A supported
    // capability-bearing package may be shipped before a user selects a root.
    let mut dictionary_manifest = manifest.clone();
    dictionary_manifest.package_id = "test.dictionary-installation".into();
    dictionary_manifest.capabilities.push(
        diy_ime2_core::isolated_worker::package::CapabilityRequest {
            id: "dictionary.read".into(),
            binding: "not-configured".into(),
        },
    );
    let dictionary = make(dictionary_manifest.clone())?;
    let cap_data = root.join("capability-data");
    fs::create_dir(&cap_data)?;
    let cap_catalog = Catalog::open(&cap_data, limits.clone())?;
    let installed = cap_catalog.review(
        vec![dictionary.clone()],
        &verifier,
        gate.clone(),
        CancellationToken::default(),
    )?;
    cap_catalog.commit(installed, ImportConfirmation::default())?;
    let captured = cap_catalog.read_installed("test.dictionary-installation")?;
    assert_eq!(captured.digest(), dictionary.digest());
    assert!(
        verifier
            .verify(captured, gate.clone(), CancellationToken::default())
            .is_err()
    );
    let cap_manifest = root.join("capability-manifest.json");
    let cap_package = root.join("capability.diynode");
    fs::write(&cap_manifest, serde_json::to_vec(&dictionary_manifest)?)?;
    assert!(
        std::process::Command::new(&cli)
            .arg("pack")
            .arg(&cap_manifest)
            .arg(&args[3])
            .arg(&cap_package)
            .output()?
            .status
            .success()
    );
    let cap_import = std::process::Command::new(&cli)
        .arg("import")
        .arg(root.join("cap-cli-data"))
        .arg(root.join("cap-cli-workers"))
        .arg(&args[1])
        .arg(&args[2])
        .arg(&cap_package)
        .arg("--commit")
        .output()?;
    assert!(
        cap_import.status.success(),
        "capability import: {}",
        String::from_utf8_lossy(&cap_import.stderr)
    );
    for line in String::from_utf8(cap_import.stdout)?.lines() {
        let reply: serde_json::Value = serde_json::from_str(line)?;
        assert_eq!(reply["executionPermissionGranted"], false);
    }
    // Tampered object never loads, and a failed rewrite keeps the prior index.
    let object = data
        .join("objects")
        .join(format!("{}.diynode", rebuild.digest()));
    fs::write(&object, b"corrupt")?;
    assert!(catalog.read_installed("test.fixture").is_err());
    let before = fs::read(data.join("catalog.json"))?;
    assert!(
        catalog
            .commit(review(vec![rebuild])?, ImportConfirmation::default())
            .is_err()
    );
    assert_eq!(fs::read(data.join("catalog.json"))?, before);
    println!(
        "Package verification/catalog: isolated definition, atomic batch, stale review, duplicate, metadata, upgrade, downgrade, rebuild, unsupported capability and corrupt object checks passed; diagnostics {}",
        root.display()
    );
    Ok(())
}
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("Windows x64 required");
        std::process::exit(1);
    }
}
