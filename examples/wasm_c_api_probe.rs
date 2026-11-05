//! Trusted developer probe. Production invocation requires AppContainer.
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = diy_ime2_core::isolated_worker::runtime::run_probe() {
        eprintln!("wasm-probe: {error}");
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("wasm-probe requires x64 Windows");
        std::process::exit(1);
    }
}
