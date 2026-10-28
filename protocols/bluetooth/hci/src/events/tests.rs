use super::*;
use bt_hci::{cmd::OpcodeGroup, param::Error};

#[test]
fn hardware_errors_preserve_every_backend_and_framing_reason_byte() {
    for code in u8::MIN..=u8::MAX {
        let event = HardwareErrorEvent::new(code);
        assert_eq!(event.kind(), PacketKind::Event);
        assert_eq!(event.as_bytes(), [0x10, 1, code]);
        assert_eq!(
            crate::validate_controller_to_host_packet(event.kind(), event.as_bytes()),
            Ok(())
        );
    }
}

#[test]
fn stop_completion_keeps_its_opcode_credit_and_success_status() {
    let stop = Opcode::new(OpcodeGroup::new(63), 0);
    let event = CommandCompleteStatusEvent::new(stop, Status::SUCCESS);
    assert_eq!(event.kind(), PacketKind::Event);
    assert_eq!(event.as_bytes(), [0x0e, 4, 1, 0, 0xfc, 0]);
    let rejected = CommandCompleteStatusEvent::new(
        Opcode::new(OpcodeGroup::new(4), 0x234),
        Error::UNKNOWN_CMD.to_status(),
    );
    assert_eq!(rejected.as_bytes(), [0x0e, 4, 1, 0x34, 0x12, 1]);
    assert_eq!(
        crate::validate_controller_to_host_packet(event.kind(), event.as_bytes()),
        Ok(())
    );
}

#[test]
fn vendor_shutdown_observations_keep_both_full_octets_and_round_trip() {
    for software in u8::MIN..=u8::MAX {
        for rollover in u8::MIN..=u8::MAX {
            let event = VendorShutdownObservationEvent::new([software, rollover]);
            assert_eq!(event.kind(), PacketKind::Event);
            assert_eq!(event.as_bytes(), [0xff, 3, 1, software, rollover]);
            let decoded = VendorShutdownObservationEvent::from_hci_bytes(event.as_bytes()).unwrap();
            assert_eq!(decoded.pending(), [software, rollover]);
        }
    }
}

#[test]
fn shutdown_decoder_rejects_wrong_envelopes_subevents_and_extents() {
    let valid = [0xff, 3, 1, 7, 3];
    for length in 0..valid.len() {
        assert_eq!(
            VendorShutdownObservationEvent::from_hci_bytes(&valid[..length]),
            None
        );
    }
    for malformed in [
        &[0xff, 3, 1, 7, 3, 0][..],
        &[0x10, 3, 1, 7, 3],
        &[0xff, 2, 1, 7, 3],
        &[0xff, 3, 2, 7, 3],
    ] {
        assert_eq!(
            VendorShutdownObservationEvent::from_hci_bytes(malformed),
            None
        );
    }
}
