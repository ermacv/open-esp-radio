extern crate std;

use std::vec;
use std::vec::Vec;

use super::*;

const CHADDR: [u8; 16] = [2, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn offer() -> Fixed {
    Fixed {
        op: Op::Reply,
        htype: 1,
        hlen: 6,
        hops: 0,
        xid: 0x1234_5678,
        secs: 3,
        flags: Flags::from_bytes([0; 2]).with_broadcast(true),
        ciaddr: [0; 4],
        yiaddr: [192, 0, 2, 50],
        siaddr: [192, 0, 2, 1],
        giaddr: [0; 4],
        chaddr: CHADDR,
    }
}

/// An OFFER with a message type, a server identifier and padding.
fn written() -> Vec<u8> {
    let mut bytes = vec![0; 300];
    let mut writer = Writer::new(&mut bytes);
    offer().write(&mut writer).unwrap();
    write_option(&mut writer, option::MESSAGE_TYPE, &[2]).unwrap();
    writer.u8(option::PAD).unwrap();
    write_option(&mut writer, option::SERVER_IDENTIFIER, &[192, 0, 2, 1]).unwrap();
    write_end(&mut writer).unwrap();
    let length = writer.written();
    bytes.truncate(length);
    bytes
}

#[test]
fn a_written_message_reads_back_field_by_field() {
    let bytes = written();
    // RFC 2131, Figure 1: the fields' places.
    assert_eq!(bytes[0], 2);
    assert_eq!(&bytes[4..8], &[0x12, 0x34, 0x56, 0x78]);
    assert_eq!(&bytes[10..12], &[0x80, 0]);
    assert_eq!(&bytes[16..20], &[192, 0, 2, 50]);
    assert_eq!(&bytes[28..34], &CHADDR[..6]);
    assert_eq!(&bytes[236..240], &MAGIC_COOKIE);
    let message = DhcpMessage::parse(&bytes).unwrap();
    assert_eq!(message.fixed, offer());
    assert!(message.fixed.flags.broadcast());
    assert_eq!(message.message_type(), Some(MessageType::Offer));
    assert_eq!(
        message.option(option::SERVER_IDENTIFIER),
        Some(&[192, 0, 2, 1][..])
    );
    assert_eq!(message.option(option::ROUTER), None);
    let codes: Vec<u8> = message.options().map(|option| option.unwrap().0).collect();
    assert_eq!(codes, [option::MESSAGE_TYPE, option::SERVER_IDENTIFIER]);
}

#[test]
fn nothing_after_the_end_option_is_an_option() {
    let mut bytes = written();
    bytes.extend_from_slice(&[option::ROUTER, 4, 1, 2, 3, 4]);
    let message = DhcpMessage::parse(&bytes).unwrap();
    assert_eq!(message.option(option::ROUTER), None);
}

#[test]
fn an_options_field_without_an_end_ends_with_its_octets() {
    let mut bytes = written();
    bytes.pop();
    let message = DhcpMessage::parse(&bytes).unwrap();
    assert_eq!(message.message_type(), Some(MessageType::Offer));
}

#[test]
fn an_option_past_the_field_is_malformed_never_a_panic() {
    let mut bytes = written();
    bytes.pop();
    bytes.extend_from_slice(&[option::ROUTER, 4, 1, 2]);
    assert_eq!(DhcpMessage::parse(&bytes), Err(DhcpError::Option));
    bytes.truncate(bytes.len() - 3);
    assert_eq!(DhcpMessage::parse(&bytes), Err(DhcpError::Option));
}

#[test]
fn every_short_message_is_truncated() {
    let bytes = written();
    for length in 0..HEADER_LEN {
        assert_eq!(
            DhcpMessage::parse(&bytes[..length]),
            Err(DhcpError::Truncated)
        );
    }
}

#[test]
fn bootp_without_the_cookie_is_not_dhcp() {
    let mut bytes = written();
    bytes[236] = 0;
    assert_eq!(DhcpMessage::parse(&bytes), Err(DhcpError::Cookie));
}

#[test]
fn a_message_type_of_another_length_is_none_and_an_unknown_one_survives() {
    let mut bytes = vec![0; 260];
    let mut writer = Writer::new(&mut bytes);
    offer().write(&mut writer).unwrap();
    write_option(&mut writer, option::MESSAGE_TYPE, &[2, 2]).unwrap();
    write_end(&mut writer).unwrap();
    assert_eq!(DhcpMessage::parse(&bytes).unwrap().message_type(), None);
    let mut bytes = vec![0; 260];
    let mut writer = Writer::new(&mut bytes);
    offer().write(&mut writer).unwrap();
    write_option(&mut writer, option::MESSAGE_TYPE, &[42]).unwrap();
    assert_eq!(
        DhcpMessage::parse(&bytes).unwrap().message_type(),
        Some(MessageType::Reserved(42))
    );
}

#[test]
fn a_write_that_does_not_fit_writes_nothing() {
    let mut bytes = [0xAA; HEADER_LEN - 1];
    assert!(offer().write(&mut Writer::new(&mut bytes)).is_err());
    assert_eq!(bytes, [0xAA; HEADER_LEN - 1]);
    let mut bytes = [0xAA; 5];
    let mut writer = Writer::new(&mut bytes);
    assert!(write_option(&mut writer, option::ROUTER, &[1, 2, 3, 4]).is_err());
    assert_eq!(writer.written(), 0);
    assert_eq!(
        write_option(&mut writer, option::ROUTER, &[0; 256]),
        Err(Error::ValueOutOfRange)
    );
}
