//! Native-only framing for one bounded resource-aware Worker invocation.
use super::adapter::{ArtifactCapture, ResourceRangeRequest, ResourceRangeSession};
use crate::execution_control::DecimalId;
use std::io::{self, Read, Write};

pub(super) const MAGIC: &[u8; 8] = b"DIYRNG01";
const BUILD_MAGIC: &[u8; 8] = b"DIYBLD01";
pub(super) const MAX_INPUT: usize = 8 * 1024 * 1024;
pub(super) const MAX_READ: usize = 64 * 1024;
const READ: u8 = 1;
const DATA: u8 = 2;
const RESULT: u8 = 3;
const CHUNK: u8 = 4;
const ACK: u8 = 5;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid bounded resource frame")
}

fn number<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn length(input: &mut impl Read, cap: usize) -> io::Result<usize> {
    let length = u32::from_le_bytes(number(input)?) as usize;
    if length > cap {
        return Err(invalid());
    }
    Ok(length)
}

pub(super) trait ResourceReader {
    fn read_resource(&mut self, resource: u32, offset: u64, length: u32) -> io::Result<Vec<u8>>;
    fn emit_artifact(&mut self, bytes: &[u8]) -> io::Result<()>;
}

pub(super) struct Client<R, W> {
    input: R,
    output: W,
    next_request: u64,
    failed: bool,
    building: bool,
    artifact_offset: u64,
}

impl<R: Read, W: Write> Client<R, W> {
    pub(super) fn start(input: R, output: W) -> io::Result<(Self, Vec<u8>)> {
        Self::start_mode(input, output, false)
    }
    pub(super) fn start_build(input: R, output: W) -> io::Result<(Self, Vec<u8>)> {
        Self::start_mode(input, output, true)
    }
    fn start_mode(mut input: R, mut output: W, building: bool) -> io::Result<(Self, Vec<u8>)> {
        let magic = if building { BUILD_MAGIC } else { MAGIC };
        if &number::<8>(&mut input)? != magic {
            return Err(invalid());
        }
        let length = length(&mut input, MAX_INPUT)?;
        let mut request = vec![0; length];
        input.read_exact(&mut request)?;
        output.write_all(magic)?;
        output.flush()?;
        Ok((
            Self {
                input,
                output,
                next_request: 1,
                failed: false,
                building,
                artifact_offset: 0,
            },
            request,
        ))
    }

    pub(super) fn finish(mut self, reply: &[u8]) -> io::Result<()> {
        if self.failed || reply.len() > MAX_INPUT {
            return Err(invalid());
        }
        self.output.write_all(&[RESULT])?;
        self.output.write_all(&(reply.len() as u32).to_le_bytes())?;
        self.output.write_all(reply)?;
        self.output.flush()?;
        if self.input.read(&mut [0])? != 0 {
            return Err(invalid());
        }
        Ok(())
    }

    fn exchange(&mut self, resource: u32, offset: u64, requested: u32) -> io::Result<Vec<u8>> {
        if self.failed || resource == 0 || requested == 0 || requested as usize > MAX_READ {
            return Err(invalid());
        }
        let id = self.next_request;
        DecimalId::new(id).map_err(|_| invalid())?;
        self.next_request = id.checked_add(1).ok_or_else(invalid)?;
        self.output.write_all(&[READ])?;
        self.output.write_all(&id.to_le_bytes())?;
        self.output.write_all(&resource.to_le_bytes())?;
        self.output.write_all(&offset.to_le_bytes())?;
        self.output.write_all(&requested.to_le_bytes())?;
        self.output.flush()?;
        if number::<1>(&mut self.input)? != [DATA]
            || u64::from_le_bytes(number(&mut self.input)?) != id
            || length(&mut self.input, MAX_READ)? != requested as usize
        {
            return Err(invalid());
        }
        let mut bytes = vec![0; requested as usize];
        self.input.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.failed || !self.building || bytes.is_empty() || bytes.len() > MAX_READ {
            return Err(invalid());
        }
        let offset = self.artifact_offset;
        let next = offset.checked_add(bytes.len() as u64).ok_or_else(invalid)?;
        self.output.write_all(&[CHUNK])?;
        self.output.write_all(&offset.to_le_bytes())?;
        self.output.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.output.write_all(bytes)?;
        self.output.flush()?;
        if number::<1>(&mut self.input)? != [ACK]
            || u64::from_le_bytes(number(&mut self.input)?) != offset
            || length(&mut self.input, MAX_READ)? != bytes.len()
        {
            return Err(invalid());
        }
        self.artifact_offset = next;
        Ok(())
    }
}

impl<R: Read, W: Write> ResourceReader for Client<R, W> {
    fn emit_artifact(&mut self, bytes: &[u8]) -> io::Result<()> {
        let result = self.emit(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn read_resource(&mut self, resource: u32, offset: u64, length: u32) -> io::Result<Vec<u8>> {
        let result = self.exchange(resource, offset, length);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

pub(super) fn serve(
    output: impl Read,
    input: impl Write,
    request: &[u8],
    session: &mut ResourceRangeSession,
) -> io::Result<Vec<u8>> {
    serve_mode(output, input, request, session, None)
}
pub(super) fn serve_build(
    output: impl Read,
    input: impl Write,
    request: &[u8],
    session: &mut ResourceRangeSession,
    capture: &mut ArtifactCapture,
) -> io::Result<Vec<u8>> {
    serve_mode(output, input, request, session, Some(capture))
}
fn serve_mode(
    mut output: impl Read,
    mut input: impl Write,
    request: &[u8],
    session: &mut ResourceRangeSession,
    mut capture: Option<&mut ArtifactCapture>,
) -> io::Result<Vec<u8>> {
    session.check_current().map_err(|_| invalid())?;
    if request.len() > MAX_INPUT {
        return Err(invalid());
    }
    let magic = if capture.is_some() {
        BUILD_MAGIC
    } else {
        MAGIC
    };
    input.write_all(magic)?;
    input.write_all(&(request.len() as u32).to_le_bytes())?;
    input.write_all(request)?;
    input.flush()?;
    if &number::<8>(&mut output)? != magic {
        return Err(invalid());
    }
    loop {
        session.check_current().map_err(|_| invalid())?;
        match number::<1>(&mut output)?[0] {
            CHUNK => {
                let offset = u64::from_le_bytes(number(&mut output)?);
                let length = length(&mut output, MAX_READ)?;
                let capture = capture.as_deref_mut().ok_or_else(invalid)?;
                capture.reserve(offset, length).map_err(|_| invalid())?;
                let mut bytes = vec![0; length];
                output.read_exact(&mut bytes)?;
                session.check_current().map_err(|_| invalid())?;
                capture.append(&bytes).map_err(|_| invalid())?;
                session.check_current().map_err(|_| invalid())?;
                input.write_all(&[ACK])?;
                input.write_all(&offset.to_le_bytes())?;
                input.write_all(&(length as u32).to_le_bytes())?;
                input.flush()?;
            }
            READ => {
                let request_id = DecimalId::new(u64::from_le_bytes(number(&mut output)?))
                    .map_err(|_| invalid())?;
                let resource_id = u32::from_le_bytes(number(&mut output)?);
                let offset = u64::from_le_bytes(number(&mut output)?);
                let length = u32::from_le_bytes(number(&mut output)?);
                if length as usize > MAX_READ {
                    return Err(invalid());
                }
                let bytes = session
                    .read(&ResourceRangeRequest {
                        request_id,
                        resource_id,
                        offset,
                        length,
                    })
                    .map_err(|_| invalid())?;
                session.check_current().map_err(|_| invalid())?;
                input.write_all(&[DATA])?;
                input.write_all(&request_id.get().to_le_bytes())?;
                input.write_all(&length.to_le_bytes())?;
                input.write_all(&bytes)?;
                input.flush()?;
            }
            RESULT => {
                let length = length(&mut output, MAX_INPUT)?;
                let mut reply = vec![0; length];
                output.read_exact(&mut reply)?;
                session.check_current().map_err(|_| invalid())?;
                drop(input);
                if output.read(&mut [0])? != 0 {
                    return Err(invalid());
                }
                session.check_current().map_err(|_| invalid())?;
                if let Some(capture) = capture.as_deref_mut() {
                    capture.complete().map_err(|_| invalid())?;
                }
                return Ok(reply);
            }
            _ => return Err(invalid()),
        }
    }
}
