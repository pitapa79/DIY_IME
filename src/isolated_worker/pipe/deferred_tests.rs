use super::*;

fn frame(reply: &[u8], metadata: &[u8]) -> Vec<u8> {
    let mut bytes = (reply.len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(reply);
    bytes.extend_from_slice(metadata);
    bytes
}

#[test]
fn inline_metadata_never_reencodes_or_repairs_guest_wire() {
    let reply = b"{\"duplicate\":1,\"duplicate\":2,\"body\":\"DIYTRC1\"}";
    for metadata in [b"".as_slice(), b"invalid private metadata".as_slice()] {
        let received = read_inline_reply(std::io::Cursor::new(frame(reply, metadata))).unwrap();
        assert_eq!(received.bytes, reply);
        assert!(
            super::super::worker_trace::decode(&received.diagnostics.unwrap().unwrap()).is_err()
        );
    }
}

#[test]
fn oversized_metadata_is_drained_without_enlarging_or_discarding_the_reply() {
    let bytes = frame(
        b"exact reply",
        &vec![0; super::super::worker_trace::LIMIT * 3],
    );
    let mut cursor = std::io::Cursor::new(bytes);
    let received = read_inline_reply(&mut cursor).unwrap();
    assert_eq!(received.bytes, b"exact reply");
    assert!(received.diagnostics.unwrap().is_err());
    assert_eq!(cursor.position() as usize, cursor.get_ref().len());
}

#[test]
fn reply_bound_and_full_body_are_required_independently_of_metadata() {
    for bytes in [
        vec![],
        vec![0; 3],
        (WIRE_LIMIT as u32 + 1).to_le_bytes().to_vec(),
        vec![5, 0, 0, 0, b'x'],
    ] {
        assert!(read_inline_reply(std::io::Cursor::new(bytes)).is_err());
    }
    let reply = vec![b'x'; WIRE_LIMIT];
    let received = read_inline_reply(std::io::Cursor::new(frame(&reply, b""))).unwrap();
    assert_eq!(received.bytes.len(), WIRE_LIMIT);
}
