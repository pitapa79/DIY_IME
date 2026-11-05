//! Host entrypoint retaining the legacy default Registry.
pub use diy_ime2_runtime::session::*;
use std::io::{self, BufRead, Write};

/// Serve the standard Host Registry through the Node-independent v2 codec.
pub fn serve(reader: impl BufRead, writer: impl Write) -> io::Result<()> {
    let mut registry = crate::node::Registry::new();
    crate::builtins::register_builtin_nodes(&mut registry).map_err(io::Error::other)?;
    serve_session(Session::new(registry), reader, writer)
}
