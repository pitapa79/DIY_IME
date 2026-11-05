use diy_ime2_core::{Candidate, ChoiceOption, Packet, PacketJsonError, ValidationError};
use serde_json::json;

#[test]
fn reads_spec_format_and_preserves_unicode_metadata_and_optional_scores() {
    let value = json!({"version":1,"root":{"type":"choice","options":[
        {"score":85,"value":{"type":"sequence","items":[
            {"type":"text","text":"𠮷野家","meta":{"reading":"よしのや","sourceSpan":[0,4]}},
            {"type":"text","text":"へ"}
        ]}},
        {"value":{"type":"text","text":"よしのやへ"}}
    ]}});
    let packet = Packet::from_json(&value.to_string()).unwrap();
    let serialized: serde_json::Value = serde_json::from_str(&packet.to_json().unwrap()).unwrap();
    assert_eq!(serialized["root"]["options"][0]["score"], 85.0);
    assert_eq!(
        serialized["root"]["options"][0]["value"],
        value["root"]["options"][0]["value"]
    );
    assert!(serialized["root"]["options"][1].get("score").is_none());
    assert_eq!(
        Packet::from_json(&packet.to_json().unwrap()).unwrap(),
        packet
    );
}

#[test]
fn rejects_wrong_types_unknown_fields_nulls_and_trailing_data() {
    for json in [
        r#"{"version":1,"root":{"type":"unknown"}}"#,
        r#"{"version":1,"root":{"type":"text"}}"#,
        r#"{"version":1,"root":{"type":"text","text":42}}"#,
        r#"{"version":1,"root":{"type":"text","text":"a","lang":"ja"}}"#,
        r#"{"version":1,"root":{"type":"text","text":"a","meta":null}}"#,
        r#"{"version":1,"root":{"type":"text","text":"a","meta":{"source_span":[0,1]}}}"#,
        r#"{"version":1,"root":{"type":"text","text":"a","meta":{"sourceSpan":[-1,1]}}}"#,
        r#"{"version":1,"root":{"type":"text","text":"a","meta":{"sourceSpan":[0,1,2]}}}"#,
        r#"{"version":1,"root":{"type":"choice","options":[{"value":{"type":"text","text":"a"},"score":null}]}}"#,
        r#"{"version":1,"root":{"type":"text","text":"a"}} {}"#,
    ] {
        assert!(Packet::from_json(json).is_err(), "accepted {json}");
    }
}

#[test]
fn validates_on_both_read_and_write() {
    assert!(matches!(
        Packet::from_json(r#"{"version":2,"root":{"type":"text","text":"a"}}"#),
        Err(PacketJsonError::Validation(
            ValidationError::UnsupportedVersion { .. }
        ))
    ));
    assert!(matches!(
        Packet::from_json(
            r#"{"version":1,"root":{"type":"text","text":"a","meta":{"sourceSpan":[4,1]}}}"#
        ),
        Err(PacketJsonError::Validation(
            ValidationError::ReversedSourceSpan { .. }
        ))
    ));
    let packet = Packet::new(Candidate::Choice {
        options: vec![ChoiceOption {
            value: Candidate::Text {
                text: "a".into(),
                meta: None,
            },
            score: Some(f64::NAN),
        }],
    });
    assert!(matches!(
        packet.to_json(),
        Err(PacketJsonError::Validation(
            ValidationError::NonFiniteScore { .. }
        ))
    ));
}

#[test]
fn enforces_json_resource_depth_on_both_read_and_write() {
    let mut root = json!({"type":"text","text":"a"});
    for _ in 0..32 {
        root = json!({"type":"sequence","items":[root]});
    }
    let input = json!({"version":1,"root":root}).to_string();
    assert!(matches!(
        Packet::from_json(&input),
        Err(PacketJsonError::DepthLimit)
    ));
    let packet: Packet = serde_json::from_str(&input).unwrap();
    assert!(matches!(packet.to_json(), Err(PacketJsonError::DepthLimit)));
}

#[test]
fn json_accepts_the_documented_depth_boundary_for_nested_choices() {
    let mut root = json!({"type":"text","text":"a"});
    for _ in 1..diy_ime2_core::MAX_JSON_CANDIDATE_DEPTH {
        root = json!({"type":"choice","options":[{"value":root}]});
    }
    let input = json!({"version":1,"root":root}).to_string();
    let packet = Packet::from_json(&input).unwrap();
    assert_eq!(
        Packet::from_json(&packet.to_json().unwrap()).unwrap(),
        packet
    );
}
