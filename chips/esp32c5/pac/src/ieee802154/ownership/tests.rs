use super::{
    MultipanIdentityProgrammingPort, TransmitSecurityProgrammingPort,
    execute_multipan_identity_configuration, execute_transmit_security_configuration,
    execute_transmit_security_disable,
};
use std::vec::Vec;

#[derive(Debug, Eq, PartialEq)]
enum Operation {
    AddressLow(u32),
    AddressHigh(u32),
    KeyWord(usize, u32),
    PayloadOffset(u8),
    Enabled(bool),
}

#[derive(Default)]
struct RecordingPort {
    operations: Vec<Operation>,
}

impl TransmitSecurityProgrammingPort for RecordingPort {
    fn write_address_low(&mut self, word: u32) {
        self.operations.push(Operation::AddressLow(word));
    }

    fn write_address_high(&mut self, word: u32) {
        self.operations.push(Operation::AddressHigh(word));
    }

    fn write_key_word(&mut self, index: usize, word: u32) {
        self.operations.push(Operation::KeyWord(index, word));
    }

    fn write_payload_offset(&mut self, offset: u8) {
        self.operations.push(Operation::PayloadOffset(offset));
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.operations.push(Operation::Enabled(enabled));
    }
}

#[derive(Debug, Eq, PartialEq)]
enum MultipanOperation {
    Enable(usize),
    PanId(usize, u16),
    ShortAddress(usize, u16),
    ExtendedLow(usize, u32),
    ExtendedHigh(usize, u32),
}

#[derive(Default)]
struct RecordingMultipanPort {
    operations: Vec<MultipanOperation>,
}

impl MultipanIdentityProgrammingPort for RecordingMultipanPort {
    fn enable_context(&mut self, index: usize) {
        self.operations.push(MultipanOperation::Enable(index));
    }

    fn write_pan_id(&mut self, index: usize, pan_id: u16) {
        self.operations
            .push(MultipanOperation::PanId(index, pan_id));
    }

    fn write_short_address(&mut self, index: usize, short_address: u16) {
        self.operations
            .push(MultipanOperation::ShortAddress(index, short_address));
    }

    fn write_extended_address_low(&mut self, index: usize, word: u32) {
        self.operations
            .push(MultipanOperation::ExtendedLow(index, word));
    }

    fn write_extended_address_high(&mut self, index: usize, word: u32) {
        self.operations
            .push(MultipanOperation::ExtendedHigh(index, word));
    }
}

#[test]
fn transmit_security_transaction_matches_source_order_and_little_endian_words() {
    let mut port = RecordingPort::default();
    execute_transmit_security_configuration(
        &mut port,
        &[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
        &[
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ],
        0x6d,
    );

    assert_eq!(
        port.operations,
        [
            Operation::AddressLow(0x7654_3210),
            Operation::AddressHigh(0xfedc_ba98),
            Operation::KeyWord(0, 0x3322_1100),
            Operation::KeyWord(1, 0x7766_5544),
            Operation::KeyWord(2, 0xbbaa_9988),
            Operation::KeyWord(3, 0xffee_ddcc),
            Operation::PayloadOffset(0x6d),
            Operation::Enabled(true),
        ]
    );
}

#[test]
fn transmit_security_disable_only_clears_enable() {
    let mut port = RecordingPort::default();
    execute_transmit_security_disable(&mut port);
    assert_eq!(port.operations, [Operation::Enabled(false)]);
}

#[test]
fn multipan_identity_transaction_has_exact_enable_and_write_order_for_every_context() {
    for index in 0..4 {
        let mut port = RecordingMultipanPort::default();
        execute_multipan_identity_configuration(
            &mut port,
            index,
            0x1234,
            0x5678,
            [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
        );
        assert_eq!(
            port.operations,
            [
                MultipanOperation::Enable(index),
                MultipanOperation::PanId(index, 0x1234),
                MultipanOperation::Enable(index),
                MultipanOperation::ShortAddress(index, 0x5678),
                MultipanOperation::Enable(index),
                MultipanOperation::ExtendedLow(index, 0x7654_3210),
                MultipanOperation::ExtendedHigh(index, 0xfedc_ba98),
            ]
        );
    }
}
