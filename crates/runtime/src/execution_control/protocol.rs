//! Protocol-v3 framing values only. Decoding an envelope does not validate or
//! execute its method payload. Legacy session-v2 remains a separate entry.
use super::{DecimalId, SessionControlState};
use crate::node::CoreError;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::io::{self, Write};

pub const PROTOCOL_VERSION: u32 = 3;
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct RequestEnvelope {
    pub id: DecimalId,
    /// Still untrusted: the method layer must reject unknown fields and validate
    /// all operation-specific values before calling the controller.
    pub request: Value,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeError {
    pub id: Option<DecimalId>,
    pub error: CoreError,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvelope {
    protocol: u32,
    id: Value,
    request: Value,
}
/// Decode one complete JSON frame, without a line delimiter. A stream owner must
/// bound its own reads and must not interpret the rest of an oversized line as
/// another request. No method is interpreted here, even on another protocol.
pub fn decode_envelope(frame: &[u8]) -> Result<RequestEnvelope, EnvelopeError> {
    if frame.len() > MAX_REQUEST_BYTES {
        return Err(EnvelopeError {
            id: None,
            error: CoreError::new("request-limit", "session request exceeds 1 MiB"),
        });
    }
    let raw: RawEnvelope = serde_json::from_slice(frame).map_err(|error| EnvelopeError {
        id: None,
        error: CoreError::new("protocol-envelope", error.to_string()),
    })?;
    let id = serde_json::from_value::<DecimalId>(raw.id);
    // Check the explicit protocol before treating a v2 method or numeric id as
    // v3. A valid canonical id can still correlate a version error.
    if raw.protocol != PROTOCOL_VERSION {
        return Err(EnvelopeError {
            id: id.ok(),
            error: CoreError::new("protocol-version", "expected session protocol 3"),
        });
    }
    let id = id.map_err(|error| EnvelopeError {
        id: None,
        error: CoreError::new("protocol-envelope", error.to_string()),
    })?;
    Ok(RequestEnvelope {
        id,
        request: raw.request,
    })
}

/// Wire state is deliberately separate from both v2 and the raw Core view.
/// The serialized form always contains all six fields, including explicit nulls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireState {
    #[serde(serialize_with = "revision_string")]
    revision: u64,
    active_plan_version: Option<DecimalId>,
    prepared_token: Option<String>,
    can_rollback: bool,
    latest_input_revision: Option<u64>,
    delivery_generation: DecimalId,
}
fn revision_string<S: Serializer>(revision: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&revision.to_string())
}
impl WireState {
    /// The session method owner supplies its actual prepared token, not a token
    /// inferred from the revision. Inconsistent Core/token state is an error.
    pub fn from_control(
        state: SessionControlState,
        prepared_token: Option<String>,
    ) -> Result<Self, CoreError> {
        if state.has_prepared != prepared_token.is_some() {
            return Err(CoreError::new(
                "protocol-state",
                "prepared token and Core state disagree",
            ));
        }
        Ok(Self {
            revision: state.revision,
            active_plan_version: state.active_plan_version,
            prepared_token,
            can_rollback: state.can_rollback,
            latest_input_revision: state.latest_input_revision,
            delivery_generation: state.delivery_generation,
        })
    }
}
#[derive(Serialize)]
struct Reply<'a> {
    protocol: u32,
    id: Option<DecimalId>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a CoreError>,
    state: &'a WireState,
}
struct FrameWriter {
    bytes: Vec<u8>,
    exceeded: bool,
}
impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_RESPONSE_BYTES - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::other("session response exceeds 8 MiB"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// Produce a complete frame (no newline), or no frame on failure. Callers must
/// not publish a partially serialized success. Limits count UTF-8 bytes.
pub fn encode_reply(
    id: Option<DecimalId>,
    outcome: Result<&Value, &CoreError>,
    state: &WireState,
) -> Result<Vec<u8>, CoreError> {
    if id.is_none() && outcome.is_ok() {
        return Err(CoreError::new(
            "protocol-envelope",
            "successful replies require an id",
        ));
    }
    let (data, error) = match outcome {
        Ok(data) => (Some(data), None),
        Err(error) => (None, Some(error)),
    };
    let reply = Reply {
        protocol: PROTOCOL_VERSION,
        id,
        ok: error.is_none(),
        data,
        error,
        state,
    };
    let mut writer = FrameWriter {
        bytes: vec![],
        exceeded: false,
    };
    serde_json::to_writer(&mut writer, &reply).map_err(|error| {
        CoreError::new(
            if writer.exceeded {
                "response-limit"
            } else {
                "protocol-response"
            },
            error.to_string(),
        )
    })?;
    Ok(writer.bytes)
}
