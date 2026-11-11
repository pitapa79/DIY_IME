//! Isolated stdin-only Wasm worker.
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = diy_ime2_core::isolated_worker::runtime::run_worker() {
        eprintln!("wasm-worker: {error}");
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("wasm-worker requires x64 Windows");
        std::process::exit(1);
    }
}
