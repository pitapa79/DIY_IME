//! Guest-side ABI helpers. IDs and lengths are claims, never authorization.
//! Build producer and consumer modules separately: only producers enable
//! `artifact-output`. Neither this crate nor these imports interpret dictionaries.
use std::fmt;

pub const MAX_CHUNK_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidResource,
    InvalidRange,
    InvalidLength,
    InvalidMessage,
    UnexpectedLength,
    UnsupportedTarget,
    PoisonedWriter,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidResource => "invalid resource descriptor",
            Self::InvalidRange => "invalid resource range",
            Self::InvalidLength => "invalid resource chunk length",
            Self::InvalidMessage => "invalid resource message",
            Self::UnexpectedLength => "unexpected native resource reply length",
            Self::UnsupportedTarget => "resource ABI requires wasm32",
            Self::PoisonedWriter => "artifact writer already failed",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ResourceDescriptor {
    pub id: u32,
    pub length: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    id: u32,
    length: u64,
}
impl TryFrom<ResourceDescriptor> for Resource {
    type Error = Error;
    fn try_from(value: ResourceDescriptor) -> Result<Self, Error> {
        if value.id == 0 || value.id > i32::MAX as u32 || value.length > i64::MAX as u64 {
            Err(Error::InvalidResource)
        } else {
            Ok(Self {
                id: value.id,
                length: value.length,
            })
        }
    }
}
impl Resource {
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    /// One positive bounded read. Failure is never a short successful read or
    /// an implicit retry; native traps terminate the invocation.
    pub fn read_exact(&self, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        if length == 0 || length > MAX_CHUNK_BYTES {
            return Err(Error::InvalidLength);
        }
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::InvalidRange);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let mut bytes = vec![0; length];
            // SAFETY: the owned buffer is live and writable for exactly length
            // bytes. The native callback independently verifies linear memory.
            let actual = unsafe {
                ffi::read(
                    self.id as i32,
                    offset as i64,
                    length as i32,
                    bytes.as_mut_ptr(),
                )
            };
            if actual != length as i32 {
                return Err(Error::UnexpectedLength);
            }
            Ok(bytes)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(Error::UnsupportedTarget)
        }
    }
}

#[cfg(feature = "artifact-output")]
pub fn emit_chunk(bytes: &[u8]) -> Result<(), Error> {
    if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
        return Err(Error::InvalidLength);
    }
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: the immutable slice remains live through the synchronous call.
        let actual = unsafe { ffi::emit(bytes.as_ptr(), bytes.len() as i32) };
        if actual != bytes.len() as i32 {
            return Err(Error::UnexpectedLength);
        }
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err(Error::UnsupportedTarget)
    }
}

#[cfg(feature = "artifact-output")]
#[derive(Default)]
pub struct ArtifactWriter {
    written: u64,
    failed: bool,
}
#[cfg(feature = "artifact-output")]
impl ArtifactWriter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn bytes_written(&self) -> u64 {
        self.written
    }
}
#[cfg(feature = "artifact-output")]
impl std::io::Write for ArtifactWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.failed {
            return Err(std::io::Error::other(Error::PoisonedWriter));
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        let length = bytes.len().min(MAX_CHUNK_BYTES);
        let Some(next) = self
            .written
            .checked_add(length as u64)
            .filter(|n| *n <= u32::MAX as u64)
        else {
            self.failed = true;
            return Err(std::io::Error::other(Error::InvalidLength));
        };
        if let Err(error) = emit_chunk(&bytes[..length]) {
            self.failed = true;
            return Err(std::io::Error::other(error));
        }
        self.written = next;
        Ok(length)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.failed {
            Err(std::io::Error::other(Error::PoisonedWriter))
        } else {
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod ffi {
    #[link(wasm_import_module = "diyime_resource_v1")]
    unsafe extern "C" {
        pub(super) fn read(resource: i32, offset: i64, length: i32, destination: *mut u8) -> i32;
    }
    #[cfg(feature = "artifact-output")]
    #[link(wasm_import_module = "diyime_prepared_v1")]
    unsafe extern "C" {
        pub(super) fn emit(source: *const u8, length: i32) -> i32;
    }
}

#[cfg(feature = "serde")]
mod message;
#[cfg(feature = "serde")]
pub use message::{
    ArtifactDescriptor, Mode, NodeIdentity, Operation, PreparationAcknowledgement, Request,
    ResourceMap,
};

#[cfg(test)]
mod tests;
