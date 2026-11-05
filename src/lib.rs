//! Compatibility facade and Host composition for DIY_IME2.
//! Node-independent execution types live in `diy-ime2-runtime`.
pub use diy_ime2_runtime::*;
pub mod builtins;
pub mod nodes;
pub mod session;

pub mod cli;
#[cfg(windows)]
pub mod isolated_worker;
