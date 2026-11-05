use diy_ime2_core::{
    execution_control::{DecimalId, SessionControlState, protocol::*},
    node::CoreError,
};
use serde_json::{Value, json};

fn id(n: u64) -> DecimalId {
    DecimalId::new(n).unwrap()
}
fn state(revision: u64) -> SessionControlState {
    SessionControlState {
        revision,
        active_plan_version: None,
        has_prepared: false,
        can_rollback: false,
        latest_input_revision: None,
        delivery_generation: id(1),
    }
}
fn decode(value: Value) -> Result<RequestEnvelope, EnvelopeError> {
    decode_envelope(&serde_json::to_vec(&value).unwrap())
}
#[test]
fn canonical_u64_envelope_ids_never_pass_through_json_numbers() {
    for text in ["1", "9007199254740993", "18446744073709551615"] {
        let request = decode(json!({"protocol":3,"id":text,"request":{"method":"hello"}})).unwrap();
        assert_eq!(request.id.to_string(), text);
        assert_eq!(request.request, json!({"method":"hello"}));
    }
    for value in [
        json!(0),
        json!(1),
        json!(9007199254740993u64),
        json!(1.5),
        json!(null),
        json!(true),
        json!(""),
        json!("0"),
        json!("01"),
        json!("-1"),
        json!("+1"),
        json!("1e2"),
        json!(" 1"),
        json!("18446744073709551616"),
        json!([]),
    ] {
        let error =
            decode(json!({"protocol":3,"id":value,"request":{"method":"hello"}})).unwrap_err();
        assert_eq!(error.error.code, "protocol-envelope", "{value}");
        assert_eq!(error.id, None);
    }
}
#[test]
fn malformed_envelopes_cannot_supply_a_correlated_method_error() {
    for bytes in [
        b"null".as_slice(),
        b"{}",
        b"{",
        b"[]",
        b"\xff",
        br#"{"protocol":3,"id":"1","request":{},"extra":true}"#,
        br#"{"protocol":3,"id":"1","id":"2","request":{}}"#,
        br#"{"protocol":3,"id":"1"}"#,
        br#"{"protocol":"3","id":"1","request":{}}"#,
        br#"{"protocol":3,"id":"1","request":{}} {}"#,
    ] {
        let error = decode_envelope(bytes).unwrap_err();
        assert_eq!(error.id, None);
        assert_eq!(error.error.code, "protocol-envelope");
    }
    // The envelope is readable even if the method layer must reject its payload.
    for payload in [
        Value::Null,
        json!(42),
        json!({"method":"unknown","extra":1}),
    ] {
        let request = decode(json!({"protocol":3,"id":"7","request":payload})).unwrap();
        assert_eq!(request.id, id(7));
        assert_eq!(request.request, payload);
    }
}
#[test]
fn explicit_version_is_checked_before_method_or_v2_numeric_id_interpretation() {
    let error = decode(json!({"protocol":2,"id":10,"request":{"method":"run"}})).unwrap_err();
    assert_eq!(error.error.code, "protocol-version");
    assert_eq!(error.id, None);
    for protocol in [0, 1, 2, 4] {
        let error = decode(json!({"protocol":protocol,"id":"9","request":null})).unwrap_err();
        assert_eq!(error.error.code, "protocol-version");
        assert_eq!(error.id, Some(id(9)));
    }
}
#[test]
fn request_boundary_counts_bytes_and_does_not_parse_a_prefix_of_an_oversized_frame() {
    let mut bytes = br#"{"protocol":3,"id":"1","request":{"method":"hello"}}"#.to_vec();
    bytes.resize(MAX_REQUEST_BYTES, b' ');
    assert_eq!(decode_envelope(&bytes).unwrap().id, id(1));
    bytes.push(b' ');
    let error = decode_envelope(&bytes).unwrap_err();
    assert_eq!(error.error.code, "request-limit");
    assert_eq!(error.id, None);
    // A multibyte character is counted in UTF-8 bytes, not scalar values.
    let mut bytes =
        serde_json::to_vec(&json!({"protocol":3,"id":"1","request":{"text":"😀"}})).unwrap();
    let used = bytes.len();
    bytes.resize(MAX_REQUEST_BYTES, b' ');
    assert!(decode_envelope(&bytes).is_ok());
    assert!(
        used > String::from_utf8(bytes[..used].to_vec())
            .unwrap()
            .chars()
            .count()
    );
}
#[test]
fn every_wire_state_field_is_present_and_all_u64_state_ids_are_exact_strings() {
    let empty = WireState::from_control(state(0), None).unwrap();
    assert_eq!(
        serde_json::to_value(&empty).unwrap(),
        json!({
            "revision":"0","activePlanVersion":null,"preparedToken":null,
            "canRollback":false,"latestInputRevision":null,"deliveryGeneration":"1"
        })
    );
    let mut core = state(u64::MAX);
    core.active_plan_version = Some(id(u64::MAX));
    core.has_prepared = true;
    core.can_rollback = true;
    core.latest_input_revision = Some(9_007_199_254_740_991);
    core.delivery_generation = id(u64::MAX);
    assert!(WireState::from_control(core.clone(), None).is_err());
    assert!(WireState::from_control(state(0), Some("token".into())).is_err());
    let wire = WireState::from_control(core, Some("opaque-preparation".into())).unwrap();
    assert_eq!(
        serde_json::to_value(wire).unwrap(),
        json!({
            "revision":"18446744073709551615","activePlanVersion":"18446744073709551615",
            "preparedToken":"opaque-preparation","canRollback":true,
            "latestInputRevision":9_007_199_254_740_991u64,"deliveryGeneration":"18446744073709551615"
        })
    );
}
#[test]
fn success_and_error_replies_are_exclusive_correlated_and_have_no_line_delimiter() {
    let state = WireState::from_control(state(0), None).unwrap();
    let payload = json!({"text":"あ😀\n次"});
    let bytes = encode_reply(Some(id(u64::MAX)), Ok(&payload), &state).unwrap();
    assert!(!bytes.contains(&b'\n'));
    let reply: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply["protocol"], 3);
    assert_eq!(reply["id"], "18446744073709551615");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["data"], payload);
    assert!(reply.get("error").is_none());
    let error = CoreError::new("protocol-request", "unknown method");
    let failed: Value =
        serde_json::from_slice(&encode_reply(Some(id(7)), Err(&error), &state).unwrap()).unwrap();
    assert_eq!(failed["id"], "7");
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["error"]["code"], "protocol-request");
    assert!(failed.get("data").is_none());
    assert_eq!(failed["state"], reply["state"]);
    let malformed = decode_envelope(b"{").unwrap_err();
    let failed: Value =
        serde_json::from_slice(&encode_reply(malformed.id, Err(&malformed.error), &state).unwrap())
            .unwrap();
    assert!(failed["id"].is_null());
    assert!(encode_reply(None, Ok(&payload), &state).is_err());
}
#[test]
fn response_writer_accepts_exactly_eight_mib_and_returns_no_partial_frame_on_overflow() {
    let state = WireState::from_control(state(0), None).unwrap();
    let empty = json!("");
    let overhead = encode_reply(Some(id(1)), Ok(&empty), &state).unwrap().len();
    let payload = json!("x".repeat(MAX_RESPONSE_BYTES - overhead));
    assert_eq!(
        encode_reply(Some(id(1)), Ok(&payload), &state)
            .unwrap()
            .len(),
        MAX_RESPONSE_BYTES
    );
    let oversized = json!("x".repeat(MAX_RESPONSE_BYTES - overhead + 1));
    assert_eq!(
        encode_reply(Some(id(1)), Ok(&oversized), &state)
            .unwrap_err()
            .code,
        "response-limit"
    );
    // Escaped control characters expand during JSON encoding; source length
    // alone must not be used to decide whether a response fits.
    let escaped = json!("\n".repeat(MAX_RESPONSE_BYTES / 2));
    assert_eq!(
        encode_reply(Some(id(1)), Ok(&escaped), &state)
            .unwrap_err()
            .code,
        "response-limit"
    );
}
#[test]
fn legacy_v2_serve_remains_explicit_and_does_not_accept_a_v3_method() {
    let input = b"{\"protocol\":3,\"id\":1,\"request\":{\"method\":\"start-run\"}}\n{\"protocol\":2,\"id\":2,\"request\":{\"method\":\"status\"}}\n";
    let mut output = vec![];
    diy_ime2_core::session::serve(std::io::Cursor::new(input), &mut output).unwrap();
    let replies: Vec<Value> = output
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["protocol"], 2);
    assert_eq!(replies[0]["error"]["code"], "protocol-version");
    assert_eq!(replies[1]["id"], 2);
    assert_eq!(replies[1]["ok"], true);
    assert_eq!(replies[0]["state"], replies[1]["state"]);
}

#[test]
fn adopted_contract_request_and_reply_examples_match_the_actual_codec() {
    let doc = include_str!("../docs/execution-control-contract-v0.2.md").replace("\r\n", "\n");
    let examples: Vec<Value> = doc
        .split("```json\n")
        .skip(1)
        .map(|part| serde_json::from_str(part.split("```").next().unwrap().trim()).unwrap())
        .collect();
    assert_eq!(examples.len(), 2);
    let request = decode(examples[0].clone()).unwrap();
    assert_eq!(request.id, id(12));
    assert_eq!(request.request, examples[0]["request"]);
    let core = SessionControlState {
        revision: 4,
        active_plan_version: Some(id(3)),
        has_prepared: false,
        can_rollback: false,
        latest_input_revision: Some(8),
        delivery_generation: id(2),
    };
    let wire = WireState::from_control(core, None).unwrap();
    let encoded = encode_reply(Some(request.id), Ok(&examples[1]["data"]), &wire).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&encoded).unwrap(),
        examples[1]
    );
}
