use std::process::ExitCode;
fn main() -> ExitCode {
    let mut registry = diy_ime2_core::node::Registry::new();
    if let Err(error) = diy_ime2_core::builtins::register_builtin_nodes(&mut registry) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    diy_ime2_core::cli::run(registry)
}
