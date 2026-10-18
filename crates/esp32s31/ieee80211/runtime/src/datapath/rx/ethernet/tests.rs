use super::*;

#[test]
fn batch_preserves_complete_ordered_frames() {
    let first_payload = [1, 2, 3];
    let second_payload = [4, 5];
    let first = EthernetFrameParts {
        destination: [0x10; 6],
        source: [0x20; 6],
        ether_type: 0x0800,
        payload: &first_payload,
    };
    let second = EthernetFrameParts {
        destination: [0x30; 6],
        source: [0x40; 6],
        ether_type: 0x0806,
        payload: &second_payload,
    };
    let mut storage = [0_u8; 64];
    let used = {
        let mut writer = PackedEthernetWriter::new(&mut storage);
        writer.push(first).unwrap();
        writer.push(second).unwrap();
        writer.used()
    };

    let first_record = record_at(&storage, used, 0).unwrap().unwrap();
    assert_eq!(first_record.frame, first);
    let second_record = record_at(&storage, used, first_record.next_offset)
        .unwrap()
        .unwrap();
    assert_eq!(second_record.frame, second);
    assert!(
        record_at(&storage, used, second_record.next_offset)
            .unwrap()
            .is_none()
    );
}

#[test]
fn exhausted_batch_fails_without_advancing_the_cursor() {
    let payload = [0_u8; 32];
    let frame = EthernetFrameParts {
        destination: [1; 6],
        source: [2; 6],
        ether_type: 0x0800,
        payload: &payload,
    };
    let mut storage = [0_u8; 32];
    let mut writer = PackedEthernetWriter::new(&mut storage);
    assert_eq!(
        writer.push(frame),
        Err(PackedEthernetError::StorageExhausted)
    );
    assert_eq!(writer.used(), 0);
}
