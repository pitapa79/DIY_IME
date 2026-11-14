//! Private single-use runtime-only bootstrap. No guest code exists at readiness.
use super::{Api, Limits, RUNTIME_READY, Recorder, WorkerTracePhase, compiled_jit};
use std::io::{Read, Write};

pub(super) fn run(
    api: &Api,
    limits: &Limits,
    diagnostics: Option<&Recorder>,
    mapped: bool,
) -> Result<(), String> {
    let public = compiled_jit::PublicRuntime::new(api, limits)?;
    let mut output = std::io::stdout().lock();
    output
        .write_all(if mapped {
            super::RUNTIME_MAPPED_READY
        } else {
            RUNTIME_READY
        })
        .and_then(|()| output.flush())
        .map_err(|_| "runtime readiness pipe failed")?;
    let mut input = std::io::stdin().lock();
    let inline = if let Some(recorder) = diagnostics.filter(|d| d.is_deferred()) {
        let mut control = [0];
        input
            .read_exact(&mut control)
            .map_err(|_| "runtime control truncated")?;
        match control[0] {
            0 => false,
            1 => {
                recorder.enable_inline();
                true
            }
            _ => return Err("invalid runtime diagnostic control".into()),
        }
    } else {
        false
    };
    let read = diagnostics.map(|d| d.span(WorkerTracePhase::ModuleRead));
    let mut header = [0; 4];
    input
        .read_exact(&mut header)
        .map_err(|_| "runtime module header truncated")?;
    let length = u32::from_le_bytes(header) as usize;
    if !(1..=8 * 1024 * 1024).contains(&length) {
        return Err("runtime module length exceeds bound".into());
    }
    enum Code {
        Bytes(Vec<u8>, String),
        Mapped(super::code_mapping::CodeView),
    }
    let code = if mapped {
        let mut handle = [0; 8];
        input
            .read_exact(&mut handle)
            .map_err(|_| "runtime code capability truncated")?;
        // SAFETY: the private trusted Parent duplicates a read-only mapping of
        // its exact settled compiler output only after normal delivery checks.
        // Parent pins the immutable backing/file/budget through real exit/IO.
        Code::Mapped(unsafe {
            super::code_mapping::CodeView::from_parent_handle(u64::from_le_bytes(handle), length)?
        })
    } else {
        let mut digest = [0; 64];
        input
            .read_exact(&mut digest)
            .map_err(|_| "runtime digest truncated")?;
        let digest = std::str::from_utf8(&digest).map_err(|_| "invalid runtime module digest")?;
        if !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid runtime module digest".into());
        }
        let mut module = Vec::new();
        module
            .try_reserve_exact(length)
            .map_err(|_| "runtime module allocation failed")?;
        module.resize(length, 0);
        input
            .read_exact(&mut module)
            .map_err(|_| "runtime module truncated")?;
        Code::Bytes(module, digest.to_owned())
    };
    drop(read);
    let read = diagnostics.map(|d| d.span(WorkerTracePhase::InputRead));
    let mut request = Vec::new();
    input
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut request)
        .map_err(|_| "runtime request pipe failed")?;
    if request.is_empty() || request.len() > 8 * 1024 * 1024 {
        return Err("invalid runtime request size".into());
    }
    drop(read);
    // SAFETY: trusted Parent sends only its own captured compile-only output,
    // after that original compiler's process/IO/ACL/profile cleanup. This private
    // header is integrity/binding, never permission to load package artifacts.
    let mut guest = match &code {
        Code::Bytes(module, digest) => unsafe { public.prepare(diagnostics, module, digest)? },
        Code::Mapped(view) => unsafe { public.prepare_immutable(diagnostics, view)? },
    };
    guest.instantiate()?;
    let reply = guest.invoke(&request)?;
    let write = diagnostics.map(|d| d.span(WorkerTracePhase::ReplyWrite));
    if inline {
        output
            .write_all(
                &u32::try_from(reply.len())
                    .map_err(|_| "reply length overflow")?
                    .to_le_bytes(),
            )
            .map_err(|_| "runtime reply frame failed")?;
    }
    output
        .write_all(&reply)
        .and_then(|()| output.flush())
        .map_err(|_| "runtime reply failed")?;
    drop(write);
    drop(guest);
    if inline && let Some(bytes) = diagnostics.and_then(Recorder::finish_inline) {
        let _ = output.write_all(&bytes).and_then(|()| output.flush());
    }
    Ok(())
}
