use super::*;

#[test]
fn the_layout_writes_the_entry_and_zeroes_the_room_after_the_mpdu() {
    let mut buffer = [0xaa_u8; 64];
    let frame = TxFrame::lay_out(
        &mut buffer,
        20,
        8,
        TxEntry {
            single: true,
            delimiters: 2,
            htc: true,
        },
        TxCapacity::Minimal,
    )
    .expect("a frame that fits");
    // The MPDU with its MIC and FCS; the entry; the transfer rounded to a word.
    assert_eq!(frame.psdu_length(), 32);
    assert_eq!(frame.transfer_length(), 40);
    assert_eq!(frame.descriptor_capacity(), 40);
    let entry = crate::layouts::WifiTxMetadataEntry::new(
        buffer.first_chunk::<TX_ENTRY_BYTES>().expect("the entry"),
    );
    assert_eq!(entry.control().mpdu_len(), 32);
    assert!(entry.control().single());
    assert_eq!(entry.delimiters().deli_num(), 2);
    assert!(entry.delimiters().htc());
    // The MPDU is untouched; the room after it, to the capacity, is zero.
    assert!(
        buffer[TX_ENTRY_BYTES..TX_ENTRY_BYTES + 20]
            .iter()
            .all(|&b| b == 0xaa)
    );
    assert!(buffer[TX_ENTRY_BYTES + 20..40].iter().all(|&b| b == 0));
    assert!(buffer[40..].iter().all(|&b| b == 0xaa));
}

#[test]
fn an_ordinary_entry_holds_its_length_alone() {
    let mut buffer = [0xff_u8; 32];
    let frame = TxFrame::lay_out(&mut buffer, 10, 0, TxEntry::default(), TxCapacity::Minimal)
        .expect("a frame that fits");
    assert_eq!(frame.psdu_length(), 14);
    assert_eq!(buffer[..TX_ENTRY_BYTES], [14, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn a_frame_the_buffer_or_the_entry_cannot_hold_is_refused_before_any_write() {
    let mut buffer = [0x11_u8; 32];
    assert_eq!(
        TxFrame::lay_out(&mut buffer, 24, 0, TxEntry::default(), TxCapacity::Minimal),
        Err(TxFrameRefused::BufferTooSmall)
    );
    assert_eq!(
        TxFrame::lay_out(
            &mut buffer,
            10,
            0,
            TxEntry::default(),
            TxCapacity::Exactly(20)
        ),
        Err(TxFrameRefused::BufferTooSmall)
    );
    let mut large = std::vec![0x11_u8; 0x5000];
    assert_eq!(
        TxFrame::lay_out(
            &mut large,
            0x3ffc,
            0,
            TxEntry::default(),
            TxCapacity::Minimal
        ),
        Err(TxFrameRefused::TooLong)
    );
    assert!(buffer.iter().all(|&b| b == 0x11));
    assert!(large.iter().all(|&b| b == 0x11));
}

#[test]
fn an_asked_capacity_is_kept_and_its_room_zeroed() {
    let mut buffer = [0x22_u8; 64];
    let frame = TxFrame::lay_out(
        &mut buffer,
        10,
        0,
        TxEntry::default(),
        TxCapacity::Exactly(48),
    )
    .expect("a frame that fits");
    assert_eq!(frame.descriptor_capacity(), 48);
    assert_eq!(frame.transfer_length(), 22);
    assert!(buffer[TX_ENTRY_BYTES + 10..48].iter().all(|&b| b == 0));
    assert!(buffer[48..].iter().all(|&b| b == 0x22));
}
