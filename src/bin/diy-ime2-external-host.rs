#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    use diy_ime2_core::isolated_worker::package::execution_host::{Configuration, run};
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.len() == 1 {
        Configuration::read(std::path::Path::new(&args[0]))
            .and_then(|config| run(config, &format!("external-host-{}", std::process::id())))
    } else {
        Err(diy_ime2_core::node::CoreError::new(
            "external-config",
            "one fixed Host configuration path required",
        ))
    };
    if let Err(error) = result {
        // Reserved for permission lock contention before Worker setup.
        let exit = if error.code == "permission-in-use" {
            75
        } else {
            1
        };
        eprintln!("{error}");
        std::process::exit(exit);
    }
}
#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("Windows x64 required");
    std::process::exit(1);
}
