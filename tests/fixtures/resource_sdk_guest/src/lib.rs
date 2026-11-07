//! SDK integration fixture, not a dictionary implementation or registered Node.
use diy_ime2_wasm_resource_sdk::{MAX_CHUNK_BYTES, Operation, Request, Resource};
use serde_json::{Value, json};
use std::cell::RefCell;

thread_local! {static REPLY:RefCell<Vec<u8>>=const {RefCell::new(Vec::new())};}

#[unsafe(no_mangle)]
pub extern "C" fn diyime_api_version() -> u32 {
    1
}
#[unsafe(no_mangle)]
pub extern "C" fn diyime_alloc(length: u32) -> u32 {
    if length == 0 || length > 8 * 1024 * 1024 {
        return 0;
    }
    let mut bytes = vec![0u8; length as usize].into_boxed_slice();
    let pointer = bytes.as_mut_ptr() as u32;
    std::mem::forget(bytes);
    pointer
}
#[unsafe(no_mangle)]
/// # Safety
/// Pointer and length must be the live allocation returned by `diyime_alloc`,
/// passed exactly once by this fixture's native caller.
pub unsafe extern "C" fn diyime_dealloc(pointer: u32, length: u32) {
    if pointer != 0 && length > 0 {
        // SAFETY: Native calls this exactly once with this fixture's alloc result.
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                pointer as *mut u8,
                length as usize,
            )));
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn diyime_release_reply() {
    REPLY.with(|reply| reply.borrow_mut().clear());
}

fn read_all(resource: Resource) -> Result<Vec<u8>, ()> {
    let length = usize::try_from(resource.len()).map_err(|_| ())?;
    if length > 8 * 1024 * 1024 {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        let next = resource
            .read_exact(
                bytes.len() as u64,
                (length - bytes.len()).min(MAX_CHUNK_BYTES),
            )
            .map_err(|_| ())?;
        bytes.extend(next);
    }
    Ok(bytes)
}
fn invoke(input: &[u8]) -> Result<Value, ()> {
    let request: Request<Value, Value> = serde_json::from_slice(input).map_err(|_| ())?;
    request.validate().map_err(|_| ())?;
    if request.node.kind != "fixture.resource" || request.node.version != 1 {
        return Err(());
    }
    #[cfg(feature = "producer")]
    {
        use diy_ime2_wasm_resource_sdk::{ArtifactWriter, PreparationAcknowledgement};
        use std::io::Write;
        let Operation::PrepareArtifact { resources, .. } = request.operation else {
            return Err(());
        };
        let descriptor = resources.0.get("source.bin").ok_or(())?;
        let bytes = read_all(Resource::try_from(*descriptor).map_err(|_| ())?)?;
        let mut writer = ArtifactWriter::new();
        writer.write_all(&bytes).map_err(|_| ())?;
        writer.flush().map_err(|_| ())?;
        let result =
            PreparationAcknowledgement::new("fixture.sdk-bytes".into(), 1, writer.bytes_written())
                .map_err(|_| ())?;
        serde_json::to_value(result).map_err(|_| ())
    }
    #[cfg(not(feature = "producer"))]
    {
        let Operation::ExecutePrepared { artifact, .. } = request.operation else {
            return Err(());
        };
        if artifact.format != "fixture.sdk-bytes" || artifact.version != 1 {
            return Err(());
        }
        let bytes = read_all(Resource::try_from(artifact.resource).map_err(|_| ())?)?;
        Ok(json!({"copied":bytes}))
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Pointer and length must name the live request allocation returned by alloc.
pub unsafe extern "C" fn diyime_call(pointer: u32, length: u32) -> u64 {
    // SAFETY: Native passed the live bounded allocation returned by alloc.
    let input = unsafe { std::slice::from_raw_parts(pointer as *const u8, length as usize) };
    let reply = match invoke(input) {
        Ok(data) => json!({"apiVersion":1,"ok":true,"data":data}),
        Err(()) => {
            json!({"apiVersion":1,"ok":false,"error":{"code":"fixture-resource","message":"SDK resource operation rejected"}})
        }
    };
    REPLY.with(|reply_slot| {
        let mut slot = reply_slot.borrow_mut();
        *slot = serde_json::to_vec(&reply).unwrap();
        ((slot.len() as u64) << 32) | (slot.as_ptr() as u32 as u64)
    })
}
