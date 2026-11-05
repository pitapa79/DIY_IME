//! Synthetic protocol fixture, never part of the end-user distribution.
#![allow(dead_code)]
mod demo;
fn main() -> std::process::ExitCode {
    let mut registry = diy_ime2_core::node::Registry::new();
    demo::register_builtin_nodes(&mut registry).unwrap();
    diy_ime2_core::cli::run(registry)
}
