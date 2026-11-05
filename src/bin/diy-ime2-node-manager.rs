#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    use diy_ime2_core::isolated_worker::package::manager::{ManagerConfig, run};
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.len() == 1 {
        ManagerConfig::read(std::path::Path::new(&args[0])).and_then(run)
    } else {
        Err(diy_ime2_core::node::CoreError::new(
            "manager-config",
            "one Host configuration path required",
        ))
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("Windows x64 required");
    std::process::exit(1);
}
