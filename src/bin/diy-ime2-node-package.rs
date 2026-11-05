//! Package producer and Host import tool. No execution grants are issued here.
#[cfg(all(windows, target_arch = "x86_64"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use diy_ime2_core::{
        execution_control::CancellationToken,
        isolated_worker::{
            adapter::{ExecutionGate, WasmArtifact, WorkerPolicy},
            package::{
                Catalog, ImportConfirmation, Manifest, Package, PackageLimits, PackageVerifier,
            },
        },
    };
    use std::{
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        path::Path,
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    let limits = PackageLimits::default();
    match args.first().map(String::as_str) {
        Some("pack") if args.len()==4=>{
            let mut json=Vec::new();File::open(&args[1])?.take(limits.manifest_bytes as u64+1).read_to_end(&mut json)?;
            if json.len()>limits.manifest_bytes{return Err("manifest exceeds selected limit".into());}
            let manifest:Manifest=serde_json::from_slice(&json)?;
            let mut module=Vec::new();File::open(&args[2])?.take(limits.module_bytes as u64+1).read_to_end(&mut module)?;
            if module.len()>limits.module_bytes{return Err("module exceeds selected limit".into());}
            let package=Package::encode(manifest,&module,&limits)?;
            let mut output=OpenOptions::new().write(true).create_new(true).open(&args[3])?;
            output.write_all(package.bytes())?;output.sync_all()?;
            println!("{}",serde_json::json!({"packageSha256":package.digest(),"authorizationSha256":package.authorization_digest(),"manifest":package.manifest()}));
        },
        Some("inspect") if args.len()==2=>{
            let package=Package::read(File::open(&args[1])?,&limits)?;
            println!("{}",serde_json::json!({"packageSha256":package.digest(),"authorizationSha256":package.authorization_digest(),"manifest":package.manifest(),"isolatedVerified":false}));
        },
        Some("import") if args.len()>=6=>{
            let mut confirmation=ImportConfirmation::default();let mut commit=false;let mut packages=Vec::new();
            for arg in &args[5..] {
                match arg.as_str() {
                    "--commit"=>commit=true,
                    "--allow-downgrade"=>confirmation.allow_downgrade=true,
                    "--allow-same-version-rebuild"=>confirmation.allow_same_version_rebuild=true,
                    _ if arg.starts_with("--")=>return Err("unknown import option".into()),
                    _=>packages.push(Package::read(File::open(arg)?,&limits)?),
                }
            }
            if packages.iter().try_fold(0usize,|n,p|n.checked_add(p.bytes().len())).is_none_or(|n|n>limits.batch_bytes){return Err("import batch exceeds selected limit".into());}
            if packages.is_empty(){return Err("no package files supplied".into());}
            fs::create_dir_all(&args[1])?;fs::create_dir_all(&args[2])?;
            let policy=WorkerPolicy::bounded_probe();
            let runtime=WasmArtifact::capture(Path::new(&args[2]),Path::new(&args[3]),Path::new(&args[4]),b"\0asm\x01\0\0\0".to_vec(),policy.clone())?;
            let verifier=PackageVerifier::new(runtime,policy);
            let catalog=Catalog::open(Path::new(&args[1]),limits)?;
            let plan=catalog.review(packages,&verifier,ExecutionGate::default(),CancellationToken::default())?;
            println!("{}",serde_json::json!({"review":plan.review(),"executionPermissionGranted":false}));
            if commit {println!("{}",serde_json::json!({"installed":catalog.commit(plan,confirmation)?,"executionPermissionGranted":false}));}
        },
        _=>return Err("usage: diy-ime2-node-package pack MANIFEST WASM OUTPUT | inspect PACKAGE | import DATA_ROOT WORKSPACE WORKER DLL PACKAGE... [--commit] [--allow-downgrade] [--allow-same-version-rebuild]".into()),
    }
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
