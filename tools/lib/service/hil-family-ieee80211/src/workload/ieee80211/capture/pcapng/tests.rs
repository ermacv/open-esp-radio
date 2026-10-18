use std::{fs, time::SystemTime};

use super::*;

#[test]
fn writes_balanced_blocks_and_ieee80211_linktype() {
    let path = std::env::temp_dir().join(format!(
        "open-radio-capture-{}-{}.pcapng",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let packet = CapturedPacket {
        generation: 2,
        frame_sequence: 0,
        dequeued_at_micros: 7,
        logical_length: 4,
        channel: None,
        rssi_dbm: None,
        rate: None,
        bytes: vec![0x08, 0x00, 0xaa, 0xbb],
    };
    write_capture(&path, &[packet], 1_000_000).unwrap();
    let bytes = fs::read(&path).unwrap();
    fs::remove_file(path).unwrap();

    assert_eq!(
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        SECTION_HEADER_BLOCK
    );
    let section_length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!(
        u32::from_le_bytes(
            bytes[section_length - 4..section_length]
                .try_into()
                .unwrap()
        ),
        section_length as u32
    );
    assert_eq!(
        u32::from_le_bytes(
            bytes[section_length..section_length + 4]
                .try_into()
                .unwrap()
        ),
        INTERFACE_DESCRIPTION_BLOCK
    );
    assert_eq!(
        u16::from_le_bytes(
            bytes[section_length + 8..section_length + 10]
                .try_into()
                .unwrap()
        ),
        LINKTYPE_IEEE802_11
    );
    let interface_length = u32::from_le_bytes(
        bytes[section_length + 4..section_length + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    let packet_offset = section_length + interface_length;
    assert_eq!(
        u32::from_le_bytes(bytes[packet_offset..packet_offset + 4].try_into().unwrap()),
        ENHANCED_PACKET_BLOCK
    );
}
