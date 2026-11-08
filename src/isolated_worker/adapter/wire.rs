//! Exact guest-v1 projection/decoding; never the Session/RunKey management wire.
use crate::{
    Packet,
    error_input::{ErrorAwareExecution, ErrorAwareValue},
    execution_control::OwnedInvocation,
    node::{CoreError, NodeDefinition},
};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
enum WireValue {
    Data { packet: Packet },
    NoOutput {},
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    code: String,
    message: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reply {
    api_version: u32,
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    #[serde(deserialize_with = "present_error")]
    error: Option<WireError>,
}
fn present_error<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<WireError>, D::Error> {
    WireError::deserialize(d).map(Some)
}
// Validate the original JSON before Value/typed decoding can erase ambiguity.
// Values are streamed; only the current objects' key sets are retained.
struct UniqueJson;
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut keys = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(de::Error::custom("ambiguous JSON"));
                    }
                    map.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
        }
        deserializer.deserialize_any(Unique)
    }
}
fn unique_json(bytes: &[u8]) -> Result<(), CoreError> {
    serde_json::from_slice::<UniqueJson>(bytes)
        .map(|_| ())
        .map_err(|_| invalid())
}
pub(super) fn reply(bytes: &[u8]) -> Result<Value, CoreError> {
    let parsed = decode_reply(bytes)?;
    match (parsed.ok, parsed.data, parsed.error) {
        (true, Some(data), None) if !data.is_null() => Ok(data),
        (false, None, Some(error)) => Err(checked_error(error).unwrap_or_else(invalid)),
        _ => Err(invalid()),
    }
}
fn decode_reply(bytes: &[u8]) -> Result<Reply, CoreError> {
    unique_json(bytes)?;
    let parsed: Reply = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if parsed.api_version != 1 {
        return Err(invalid());
    }
    Ok(parsed)
}
fn checked_error(error: WireError) -> Option<CoreError> {
    if !error.code.is_empty()
        && error.code.len() <= 64
        && error
            .code
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        && error.message.len() <= 4096
    {
        Some(CoreError::new(error.code, "Wasm Node reported an error"))
    } else {
        None
    }
}
pub(in crate::isolated_worker) fn is_error_reply(bytes: &[u8]) -> bool {
    match decode_reply(bytes) {
        Ok(Reply {
            ok: false,
            data: None,
            error: Some(error),
            ..
        }) => checked_error(error).is_some(),
        _ => false,
    }
}
fn invalid() -> CoreError {
    CoreError::new(
        "worker-reply",
        "Wasm reply does not match the declared bounded protocol",
    )
}

struct UniqueOutputs(BTreeMap<String, WireValue>);
impl<'de> Deserialize<'de> for UniqueOutputs {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = UniqueOutputs;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unique output ports")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut result = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, WireValue>()? {
                    if result.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate output port"));
                    }
                }
                Ok(UniqueOutputs(result))
            }
        }
        deserializer.deserialize_map(Unique)
    }
}
pub(super) fn outputs(
    bytes: &[u8],
    definition: &NodeDefinition,
) -> Result<ErrorAwareExecution, CoreError> {
    unique_json(bytes)?;
    // Decode the envelope as typed raw data to retain duplicate-key checks below.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Success {
        api_version: u32,
        ok: bool,
        data: UniqueOutputs,
    }
    let success: Success = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => {
            reply(bytes)?;
            return Err(invalid());
        }
    };
    if success.api_version != 1 || !success.ok {
        return Err(invalid());
    }
    checked_outputs(success.data.0, definition)
}
/// The retained decoder has already checked the original envelope/duplicates.
pub(super) fn retained_outputs(
    data: Value,
    definition: &NodeDefinition,
) -> Result<ErrorAwareExecution, CoreError> {
    let outputs: UniqueOutputs = serde_json::from_value(data).map_err(|_| invalid())?;
    checked_outputs(outputs.0, definition)
}
fn checked_outputs(
    values: BTreeMap<String, WireValue>,
    definition: &NodeDefinition,
) -> Result<ErrorAwareExecution, CoreError> {
    if values.len() != definition.outputs.len() {
        return Err(invalid());
    }
    let mut outputs = BTreeMap::new();
    for (name, value) in values {
        let port = definition
            .outputs
            .iter()
            .find(|p| p.id == name)
            .ok_or_else(invalid)?;
        let value = match value {
            WireValue::NoOutput {} => ErrorAwareValue::NoOutput,
            WireValue::Data { packet } => {
                packet.validate().map_err(|_| invalid())?;
                if !port.root.accepts(&packet.root) {
                    return Err(invalid());
                }
                ErrorAwareValue::Data(Arc::new(packet))
            }
        };
        outputs.insert(name, value);
    }
    // No guest-supplied provenance/recovery/dependency claims.
    Ok(ErrorAwareExecution::new(outputs))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NetworkCall {
    pub binding: String,
    pub body: Vec<u8>,
    #[serde(deserialize_with = "required_json")]
    pub continuation: Value,
}
fn required_json<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    Value::deserialize(deserializer)
}
pub(super) enum ExecutionStep {
    Outputs(ErrorAwareExecution),
    Network(NetworkCall),
}
pub(super) fn execution_step(
    bytes: &[u8],
    definition: &NodeDefinition,
) -> Result<ExecutionStep, CoreError> {
    unique_json(bytes)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct CallReply {
        api_version: u32,
        ok: bool,
        call: NetworkCall,
    }
    if let Ok(reply) = serde_json::from_slice::<CallReply>(bytes) {
        if reply.api_version != 1 || !reply.ok {
            return Err(invalid());
        }
        return Ok(ExecutionStep::Network(reply.call));
    }
    outputs(bytes, definition).map(ExecutionStep::Outputs)
}

pub(super) fn request(kind: &str, version: u32, operation: Value) -> Value {
    json!({"apiVersion":1,"node":{"kind":kind,"version":version},"operation":operation})
}
pub(super) fn execution(
    invocation: &OwnedInvocation,
    definition: &NodeDefinition,
) -> Result<Value, CoreError> {
    let mut inputs = BTreeMap::new();
    for (name, value) in invocation.inputs() {
        let port = definition
            .inputs
            .iter()
            .find(|p| p.port.id == *name)
            .ok_or_else(invalid)?;
        let value = match value {
            ErrorAwareValue::Data(packet) => {
                packet.validate().map_err(|_| invalid())?;
                if !port.port.root.accepts(&packet.root) {
                    return Err(invalid());
                }
                WireValue::Data {
                    packet: (**packet).clone(),
                }
            }
            ErrorAwareValue::NoOutput => WireValue::NoOutput {},
            ErrorAwareValue::Error(_) => {
                return Err(CoreError::new(
                    "worker-input",
                    "unhandled error input cannot enter guest",
                ));
            }
        };
        inputs.insert(name, value);
    }
    let mode = invocation.context().input.map(|i| i.mode);
    let mut operation = json!({"method":"execute","config":invocation.config(),"inputs":inputs});
    if let Some(mode) = mode {
        operation["mode"] = json!(mode);
    }
    Ok(request(&definition.kind, definition.version, operation))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Port, RootConstraint};
    #[test]
    fn network_steps_are_exclusive_strict_and_cannot_enter_definition_or_output_decoders() {
        let definition = NodeDefinition::new("fixture", 1, vec![], vec![Port::packet("call")]);
        let good=br#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[0,255],"continuation":null}}"#;
        assert!(matches!(
            execution_step(good, &definition).unwrap(),
            ExecutionStep::Network(_)
        ));
        assert!(reply(good).is_err());
        assert!(outputs(good, &definition).is_err());
        let output = br#"{"apiVersion":1,"ok":true,"data":{"call":{"state":"noOutput"}}}"#;
        assert!(matches!(
            execution_step(output, &definition).unwrap(),
            ExecutionStep::Outputs(_)
        ));
        for bytes in [
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[]}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[-1],"continuation":{}}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[256],"continuation":{}}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[1.5],"continuation":{}}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":"private-body","continuation":{}}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[],"continuation":{},"url":"https://elsewhere"}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[],"continuation":{},"authorization":"private-key"}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","body":[],"continuation":{"x":1,"\u0078":2}}}"#,
            r#"{"apiVersion":1,"ok":true,"call":{"binding":"api","binding":"other","body":[],"continuation":{}}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"call":{"state":"noOutput"}},"call":{"binding":"api","body":[],"continuation":{}}}"#,
            r#"{"apiVersion":1,"ok":false,"call":{"binding":"api","body":[],"continuation":{}}}"#,
            r#"{"apiVersion":2,"ok":true,"call":{"binding":"api","body":[],"continuation":{}}}"#,
        ] {
            assert_eq!(
                execution_step(bytes.as_bytes(), &definition)
                    .err()
                    .unwrap()
                    .code,
                "worker-reply"
            );
        }
    }
    #[test]
    fn rejects_forged_authority_envelopes_invalid_outputs_and_duplicate_ports() {
        let definition = NodeDefinition::new(
            "fixture",
            1,
            vec![],
            vec![Port::packet("out").with_root(RootConstraint::Text)],
        );
        for bytes in [
            r#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput"}},"trusted":true}"#,
            r#"{"apiVersion":1,"ok":true,"data":{}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"other":{"state":"noOutput"}}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"data","packet":{"version":2,"root":{"type":"text","text":"x"}}}}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"data","packet":{"version":1,"root":{"type":"sequence","items":[]}}}}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput"},"out":{"state":"noOutput"}}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput","secret":"x"}}}"#,
        ] {
            assert!(outputs(bytes.as_bytes(), &definition).is_err(), "{bytes}");
        }
        let result = outputs(
            br#"{"apiVersion":1,"ok":true,"data":{"out":{"state":"noOutput"}}}"#,
            &definition,
        )
        .unwrap();
        assert!(matches!(result.outputs["out"], ErrorAwareValue::NoOutput));
    }
    #[test]
    fn guest_errors_do_not_echo_supplied_messages() {
        let error = reply(br#"{"apiVersion":1,"ok":false,"error":{"code":"invalid-config","message":"private-canary"}}"#).unwrap_err();
        assert_eq!(error.code, "invalid-config");
        assert!(!error.message.contains("private-canary"));
    }
    #[test]
    fn rejects_nested_and_escaped_duplicate_keys_before_generic_value_decoding() {
        for bytes in [
            r#"{"apiVersion":1,"ok":true,"data":{"defaultConfig":{"mode":"echo","mode":"echo"}}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"items":[{"private-key":1,"private-key":2}]}}"#,
            r#"{"apiVersion":1,"ok":true,"data":{"mode":1,"\u006dode":1}}"#,
            r#"{"apiVersion":1,"ok":true,"ok":true,"data":{}}"#,
        ] {
            let error = reply(bytes.as_bytes()).unwrap_err();
            assert_eq!(error.code, "worker-reply");
            assert!(!error.message.contains("private-key"));
        }
        let valid = br#"{"apiVersion":1,"ok":true,"data":{"a":null,"b":true,"c":-1,"d":18446744073709551615,"e":1.25,"f":"text","g":[{"a":1},{"a":2}]}}"#;
        assert!(reply(valid).is_ok());
        let too_deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
        assert!(unique_json(too_deep.as_bytes()).is_err());
    }
}
