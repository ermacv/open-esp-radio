//! Header layouts constructed from IEEE 802.15.4 frame-control rules; the
//! expectations follow the pinned `esp_ieee802154_frame.c`.

use super::{AddressMode, FrameAddress, FrameType, FrameVersion, PhrFrame};

/// 2006 data frame, PAN-ID compression, short addresses (the transmit frame
/// of the host comparison with the vendor driver).
const DATA_2003: [u8; 13] = [
    0x0c, 0x61, 0x88, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 0x00, 0x00,
];

/// 2006 Data Request command from an extended source.
const DATA_REQUEST_2006: [u8; 19] = [
    18, 0x63, 0xd8, 0x07, 0x34, 0x12, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 0x04, 0, 0,
];

/// Secured 2015 Data Request: frame-counter-suppressed auxiliary header, a
/// three-byte header IE and HT2 termination, then a four-byte MIC.
const SECURED_DATA_REQUEST_2015_IE: [u8; 25] = [
    24, 0x4b, 0xaa, 0x09, 0x34, 0x12, 0x00, 0x00, 0x78, 0x56, 0x25, 0x03, 0x00, 1, 2, 3, 0x80,
    0x3f, 0x04, 0, 0, 0, 0, 0, 0,
];

/// The same Data Request without security.
const DATA_REQUEST_2015_IE: [u8; 20] = [
    19, 0x43, 0xaa, 0x09, 0x34, 0x12, 0x00, 0x00, 0x78, 0x56, 0x03, 0x00, 1, 2, 3, 0x80, 0x3f,
    0x04, 0, 0,
];

#[test]
fn frame_control_fields_follow_the_vendor_masks() {
    let frame = PhrFrame::new(&DATA_2003);
    assert_eq!(frame.length(), 12);
    assert_eq!(frame.frame_type(), FrameType::Data);
    assert_eq!(frame.version(), FrameVersion::V2003);
    assert!(frame.ack_required());
    assert!(!frame.security_enabled());
    assert_eq!(frame.destination_mode(), AddressMode::Short);
    assert_eq!(frame.source_mode(), AddressMode::Short);
    assert_eq!(frame.destination_panid(), Some([0x34, 0x12]));
    assert_eq!(frame.source_panid(), None);
    assert_eq!(
        frame.destination_address(),
        Ok(Some(FrameAddress::Short([0xff, 0xff])))
    );
    assert_eq!(
        frame.source_address(),
        Ok(Some(FrameAddress::Short([0x78, 0x56])))
    );
    assert!(!frame.is_data_request());
}

#[test]
fn a_command_frame_is_a_data_request_after_its_addresses() {
    let frame = PhrFrame::new(&DATA_REQUEST_2006);
    assert_eq!(frame.version(), FrameVersion::V2006);
    assert_eq!(
        frame.source_address(),
        Ok(Some(FrameAddress::Extended([1, 2, 3, 4, 5, 6, 7, 8])))
    );
    assert!(frame.is_data_request());
    assert_eq!(frame.security_payload_offset(), Some(16));
}

#[test]
fn header_ies_are_skipped_up_to_the_ht2_terminator() {
    let frame = PhrFrame::new(&SECURED_DATA_REQUEST_2015_IE);
    assert_eq!(frame.version(), FrameVersion::V2015);
    assert!(frame.security_enabled());
    assert!(frame.is_data_request());
    assert_eq!(frame.security_payload_offset(), Some(17));
}

/// ESP-IDF 4d80c1b2 `ieee802154_frame_payload_offset` skips auxiliary
/// security only when enabled; an unsecured IE scan starts at the first IE.
#[test]
fn an_unsecured_ie_scan_starts_at_the_first_ie() {
    let frame = PhrFrame::new(&DATA_REQUEST_2015_IE);
    assert!(!frame.security_enabled());
    assert!(frame.is_data_request());
    assert_eq!(frame.security_payload_offset(), Some(16));
}

/// 2015 PAN ID presence: extended/extended with compression carries neither
/// PAN ID; without compression only the destination PAN ID.
#[test]
fn version_2015_pan_id_presence_follows_the_compression_table() {
    let compressed = [13, 0x41, 0xec, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let frame = PhrFrame::new(&compressed);
    assert_eq!(frame.destination_panid(), None);
    assert_eq!(frame.source_panid(), None);
    assert_eq!(
        frame.destination_address(),
        Ok(Some(FrameAddress::Extended([1, 2, 3, 4, 5, 6, 7, 8])))
    );

    let uncompressed = [13, 0x01, 0xec, 0x00, 0x34, 0x12, 1, 2, 3, 4, 5, 6, 7, 8];
    let frame = PhrFrame::new(&uncompressed);
    assert_eq!(frame.destination_panid(), Some([0x34, 0x12]));
    assert_eq!(frame.source_panid(), None);
}

#[test]
fn unsupported_types_and_reserved_modes_have_no_addresses() {
    let mut multipurpose = DATA_2003;
    multipurpose[1] = 0x65;
    let frame = PhrFrame::new(&multipurpose);
    assert!(!frame.frame_type().is_supported());
    assert!(!frame.ack_required());
    assert_eq!(frame.source_address(), Err(()));

    let mut reserved = DATA_2003;
    reserved[2] = 0x44;
    let frame = PhrFrame::new(&reserved);
    assert_eq!(frame.source_mode(), AddressMode::Reserved);
    assert_eq!(frame.destination_address(), Err(()));
    assert_eq!(frame.source_address(), Err(()));
    assert_eq!(frame.security_payload_offset(), None);
}

#[test]
fn a_truncated_image_reports_absent_fields() {
    let frame = PhrFrame::new(&DATA_REQUEST_2006[..10]);
    assert_eq!(frame.source_address(), Err(()));
    assert!(!frame.is_data_request());
}

#[test]
fn phr_bounds_exclude_fcs_and_allocation_tail_from_header_fields() {
    let mut frame = DATA_REQUEST_2006;
    frame[0] = 3;
    assert_eq!(PhrFrame::new(&frame).security_payload_offset(), None);
    assert_eq!(PhrFrame::new(&frame).destination_address(), Err(()));
    assert!(!PhrFrame::new(&frame).ack_required());
    // The command byte exists in the allocation but occupies the FCS extent.
    frame[0] = 17;
    assert!(!PhrFrame::new(&frame).is_data_request());
    assert_eq!(PhrFrame::new(&frame).security_payload_offset(), None);
    for end in 0..SECURED_DATA_REQUEST_2015_IE.len() - 2 {
        assert_eq!(
            PhrFrame::new(&SECURED_DATA_REQUEST_2015_IE[..end]).security_payload_offset(),
            None
        );
    }
}

#[test]
fn security_and_ie_bodies_must_fit_before_the_footer() {
    let mut frame = SECURED_DATA_REQUEST_2015_IE;
    frame[0] = 12;
    assert_eq!(PhrFrame::new(&frame).security_payload_offset(), None);
    frame[0] = 24;
    frame[11] = 127; // IE body extends through the MIC and FCS.
    assert_eq!(PhrFrame::new(&frame).security_payload_offset(), None);
    assert_eq!(PhrFrame::new(&frame).header_ie(0), None);
    // An incomplete descriptor cannot borrow its high byte from the MIC.
    frame[11] = 3;
    frame[0] = 18;
    assert_eq!(PhrFrame::new(&frame).security_payload_offset(), None);
}

#[test]
fn payload_termination_keeps_the_pinned_vendor_projection() {
    let mut image = DATA_REQUEST_2015_IE;
    image[16] |= 0x40;
    // The vendor's termination mask ignores descriptor bit 14. Full header
    // IE lookup still uses the standard's eight-bit element identifier.
    assert!(PhrFrame::new(&image).is_data_request());
    assert_eq!(PhrFrame::new(&image).header_ie(0x7f), None);
}
