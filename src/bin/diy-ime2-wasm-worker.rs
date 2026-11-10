//! Isolated stdin-only Wasm worker.
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = diy_ime2_core::isolated_worker::runtime::run_worker() {
        eprintln!("wasm-worker: {error}");
        std::process::exit(1);
    }
    #[cfg(all(windows, target_arch = "x86_64", feature = "worker-exit-probe"))]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--prepare-jit-v1")) {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

        // Probe only: run_worker flushed the reply and dropped the runtime.
        // The Host still waits on the original process handle before cleanup.
        unsafe {
            TerminateProcess(GetCurrentProcess(), 0);
        }
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("wasm-worker requires x64 Windows");
        std::process::exit(1);
    }
}
