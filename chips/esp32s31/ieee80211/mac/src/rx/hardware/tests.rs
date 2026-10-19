use super::*;

const AGREEMENT: S31RxBlockAckAgreement = S31RxBlockAckAgreement {
    hardware_index: 3,
    interface: MacInterface::AccessPoint,
    peer: [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0],
    tid: 6,
    starting_sequence: SequenceNumber::new(0x0abc).unwrap(),
    window: 16,
};

#[test]
fn validation_rejects_every_unrepresentable_field() {
    assert!(AGREEMENT.validate().is_ok());
    assert!(matches!(
        S31RxBlockAckAgreement {
            hardware_index: 8,
            ..AGREEMENT
        }
        .validate(),
        Err(S31RxBlockAckAgreementError::HardwareIndex(8))
    ));
    assert!(matches!(
        S31RxBlockAckAgreement {
            peer: [1, 0, 0, 0, 0, 0],
            ..AGREEMENT
        }
        .validate(),
        Err(S31RxBlockAckAgreementError::MulticastPeer)
    ));
    assert!(matches!(
        S31RxBlockAckAgreement {
            tid: 8,
            ..AGREEMENT
        }
        .validate(),
        Err(S31RxBlockAckAgreementError::Tid(8))
    ));
    assert!(matches!(
        S31RxBlockAckAgreement {
            window: 128,
            ..AGREEMENT
        }
        .validate(),
        Err(S31RxBlockAckAgreementError::Window(128))
    ));
}
