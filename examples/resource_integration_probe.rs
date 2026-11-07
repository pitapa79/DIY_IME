//! Explicit synthetic-fixture runner; not distributed or linked by default.
#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    use diy_ime2_core::isolated_worker::adapter::integration_probe;
    let result = (|| {
        let mut args = std::env::args_os().skip(1);
        let path = args
            .next()
            .ok_or("expected one synthetic fixture manifest")?;
        if args.next().is_some() {
            return Err("expected one synthetic fixture manifest".to_owned());
        }
        let bytes = integration_probe::read_bounded(std::path::Path::new(&path), 8 * 1024 * 1024)?;
        let spec =
            serde_json::from_slice(&bytes).map_err(|_| "invalid probe manifest".to_owned())?;
        integration_probe::run(spec)
    })();
    match result {
        Ok(value) => println!("{}", serde_json::to_string(&value).unwrap()),
        Err(error) => {
            eprintln!("resource integration probe: {error}");
            std::process::exit(1);
        }
    }
}
#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("Windows x86_64 required");
    std::process::exit(1);
}
