//! JSON Lines transport. A shared stream closes only its Session; a standalone
//! Host root additionally waits for actual physical ending before it returns.
use super::protocol::MAX_REQUEST_BYTES;
use super::{ExecutionHost, ExecutionService, ResourcePolicy, session::SessionV3};
use crate::node::Registry;
use std::{
    io::{self, BufRead, Read, Write},
    time::Duration,
};

/// Serve one Session on an existing Host. The outer process owner must retain
/// the Host/service while any other streams or physical jobs are still alive.
pub fn serve(
    service: &ExecutionService,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
) -> io::Result<()> {
    serve_with_resource_root(service, registry, reader, writer, None)
}
pub fn serve_with_resource_root(
    service: &ExecutionService,
    registry: Registry,
    mut reader: impl BufRead,
    mut writer: impl Write,
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
) -> io::Result<()> {
    let mut session = SessionV3::with_registry_and_resource_root(
        service,
        std::sync::Arc::new(registry),
        resource_root,
    )
    .map_err(io::Error::other)?;
    loop {
        let Some(frame) = read_frame(&mut reader)? else {
            return Ok(());
        };
        let oversized = frame.len() > MAX_REQUEST_BYTES;
        let reply = session.reply(&frame).map_err(io::Error::other)?;
        writer.write_all(&reply)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        if oversized {
            // Never treat the unconsumed suffix of this line as another request.
            return Ok(());
        }
    }
}
/// A bounded read never leaves an oversized suffix eligible as a new request.
/// Callers must close their stream after a returned frame exceeds the limit.
pub(crate) fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    let count = reader
        .by_ref()
        .take((MAX_REQUEST_BYTES + 2) as u64)
        .read_until(b'\n', &mut frame)?;
    if count == 0 {
        return Ok(None);
    }
    if frame.last() == Some(&b'\n') {
        frame.pop();
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
    }
    Ok(Some(frame))
}
pub(crate) struct HostDrain(pub(crate) ExecutionHost);
impl Drop for HostDrain {
    fn drop(&mut self) {
        // Outside the control owner: polling resource counters does not adopt
        // results or observe a Node. There is intentionally no forced timeout.
        loop {
            let usage = self.0.usage();
            if usage.physical_jobs == 0
                && usage.queued_jobs == 0
                && usage.active_runs == 0
                && usage.observers == 0
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
/// Explicit standalone Host, not a per-Session bridge process constructor.
/// The caller supplies the complete policy; no fixture/product defaults apply.
/// EOF, I/O errors and unwinding close the Session before the root drain runs.
pub fn serve_host(
    policy: ResourcePolicy,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
) -> io::Result<()> {
    serve_host_with_resource_root(policy, registry, reader, writer, None)
}
pub fn serve_host_with_resource_root(
    policy: ResourcePolicy,
    registry: Registry,
    reader: impl BufRead,
    writer: impl Write,
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
) -> io::Result<()> {
    policy.validate().map_err(io::Error::other)?;
    let domain = format!(
        "host-{}",
        super::epoch::generate().map_err(io::Error::other)?.as_str()
    );
    let host = ExecutionHost::new(domain, policy).map_err(io::Error::other)?;
    let _drain = HostDrain(host.clone());
    let service = ExecutionService::new(host).map_err(io::Error::other)?;
    serve_with_resource_root(&service, registry, reader, writer, resource_root)
}
