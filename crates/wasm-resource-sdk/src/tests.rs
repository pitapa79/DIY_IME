use super::*;

#[test]
fn descriptors_and_ranges_reject_arithmetic_errors_before_ffi() {
    for descriptor in [
        ResourceDescriptor { id: 0, length: 8 },
        ResourceDescriptor {
            id: u32::MAX,
            length: 8,
        },
        ResourceDescriptor {
            id: 1,
            length: u64::MAX,
        },
    ] {
        assert_eq!(Resource::try_from(descriptor), Err(Error::InvalidResource));
    }
    let resource = Resource::try_from(ResourceDescriptor { id: 1, length: 8 }).unwrap();
    assert_eq!(resource.read_exact(0, 0), Err(Error::InvalidLength));
    assert_eq!(
        resource.read_exact(0, MAX_CHUNK_BYTES + 1),
        Err(Error::InvalidLength)
    );
    for (offset, length) in [(u64::MAX, 1), (7, 2), (9, 1)] {
        assert_eq!(
            resource.read_exact(offset, length),
            Err(Error::InvalidRange)
        );
    }
    let empty = Resource::try_from(ResourceDescriptor { id: 1, length: 0 }).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.read_exact(0, 1), Err(Error::InvalidRange));
    #[cfg(not(target_arch = "wasm32"))]
    assert_eq!(resource.read_exact(0, 2), Err(Error::UnsupportedTarget));
}

#[cfg(all(feature = "artifact-output", not(target_arch = "wasm32")))]
#[test]
fn native_target_has_no_fallback_and_failed_writer_never_retries() {
    use std::io::Write;
    let mut writer = ArtifactWriter::new();
    assert_eq!(writer.write(&[]).unwrap(), 0);
    assert!(writer.write(b"ab").is_err());
    assert_eq!(writer.bytes_written(), 0);
    let error = writer.write(b"cd").unwrap_err();
    assert_eq!(
        error.get_ref().unwrap().downcast_ref::<Error>(),
        Some(&Error::PoisonedWriter)
    );
    assert!(writer.flush().is_err());
    assert_eq!(emit_chunk(&[]), Err(Error::InvalidLength));
    assert_eq!(
        emit_chunk(&vec![0; MAX_CHUNK_BYTES + 1]),
        Err(Error::InvalidLength)
    );
}

#[cfg(feature = "serde")]
mod messages {
    use super::*;
    use serde_json::{Value, json};
    fn prepare() -> Value {
        json!({"apiVersion":1,"node":{"kind":"fixture.resource","version":1},"operation":{"method":"prepareArtifact","config":{},"resources":{"source.bin":{"id":1,"length":8}}}})
    }
    fn execute() -> Value {
        json!({"apiVersion":1,"node":{"kind":"fixture.resource","version":1},"operation":{"method":"executePrepared","config":{},"inputs":{},"artifact":{"resource":{"id":1,"length":8},"format":"fixture","version":1},"mode":"compose"}})
    }
    fn valid(value: Value) -> bool {
        serde_json::from_value::<Request<Value, Value>>(value).is_ok_and(|r| r.validate().is_ok())
    }
    #[test]
    fn exact_additional_requests_roundtrip_and_legacy_operations_are_distinct() {
        for value in [prepare(), execute()] {
            let request: Request<Value, Value> = serde_json::from_value(value.clone()).unwrap();
            request.validate().unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), value);
        }
        for method in ["prepare", "execute", "describe", "network"] {
            let mut value = prepare();
            value["operation"]["method"] = method.into();
            assert!(!valid(value));
        }
    }
    #[test]
    fn explicit_null_unknown_fields_and_descriptor_self_claims_are_invalid() {
        let cases = [
            ("/apiVersion", json!(2)),
            ("/node/version", json!(0)),
            ("/node/kind", json!("")),
            ("/operation/mode", Value::Null),
            ("/operation/mode", json!("other")),
            ("/operation/artifact/version", json!(0)),
            ("/operation/artifact/format", json!("Invalid")),
            ("/operation/artifact/resource/id", json!(0)),
            ("/operation/artifact/resource/length", json!(u64::MAX)),
        ];
        for (pointer, replacement) in cases {
            let mut value = execute();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(!valid(value), "{pointer}");
        }
        let mut value = execute();
        value["operation"].as_object_mut().unwrap().remove("mode");
        assert!(valid(value));
        let mut value = execute();
        value["operation"]["resourceRoot"] = json!("forbidden");
        assert!(!valid(value));
        let mut value = prepare();
        value["operation"]["resources"]["other.bin"] = json!({"id":1,"length":8});
        assert!(!valid(value));
        let mut value = prepare();
        value["operation"]["resources"]["source.bin"]["path"] = json!("forbidden");
        assert!(!valid(value));
    }
    #[test]
    fn duplicate_logical_references_and_top_level_fields_are_refused() {
        for bytes in [
            r#"{"apiVersion":1,"apiVersion":1,"node":{"kind":"fixture","version":1},"operation":{"method":"prepareArtifact","config":{},"resources":{"a":{"id":1,"length":8}}}}"#,
            r#"{"apiVersion":1,"node":{"kind":"fixture","version":1},"operation":{"method":"prepareArtifact","config":{},"resources":{"a":{"id":1,"length":8},"\u0061":{"id":2,"length":8}}}}"#,
        ] {
            assert!(serde_json::from_str::<Request<Value, Value>>(bytes).is_err());
        }
    }
    #[test]
    fn acknowledgement_requires_complete_positive_versioned_output() {
        let valid = PreparationAcknowledgement::new("fixture".into(), 1, 8).unwrap();
        let bytes = serde_json::to_vec(&valid).unwrap();
        assert_eq!(
            serde_json::from_slice::<PreparationAcknowledgement>(&bytes).unwrap(),
            valid
        );
        for bad in [
            PreparationAcknowledgement {
                valid: false,
                ..valid.clone()
            },
            PreparationAcknowledgement {
                prepared: false,
                ..valid.clone()
            },
            PreparationAcknowledgement {
                artifact_bytes: 0,
                ..valid.clone()
            },
            PreparationAcknowledgement {
                artifact_bytes: u64::MAX,
                ..valid.clone()
            },
            PreparationAcknowledgement {
                artifact_version: 0,
                ..valid.clone()
            },
            PreparationAcknowledgement {
                artifact_format: "x".repeat(129),
                ..valid.clone()
            },
        ] {
            assert_eq!(bad.validate(), Err(Error::InvalidMessage));
        }
        let mut value = serde_json::to_value(valid).unwrap();
        value["trusted"] = true.into();
        assert!(serde_json::from_value::<PreparationAcknowledgement>(value).is_err());
    }
}
