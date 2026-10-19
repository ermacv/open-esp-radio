use super::{
    Ieee802154RxStateCode, Ieee802154TxSecurityError, Ieee802154TxSecurityErrorObservation,
};

#[test]
fn transmit_security_error_uses_the_public_ll_reasons() {
    assert_eq!(
        Ieee802154TxSecurityErrorObservation::from_field(0),
        Ieee802154TxSecurityErrorObservation::None
    );
    for (value, reason) in [
        (1, Ieee802154TxSecurityError::FrameControlNotSet),
        (2, Ieee802154TxSecurityError::ReservedSecurityLevel),
        (3, Ieee802154TxSecurityError::HeaderParse),
        (4, Ieee802154TxSecurityError::PayloadError),
        (5, Ieee802154TxSecurityError::FrameCounterSuppression),
    ] {
        assert_eq!(
            Ieee802154TxSecurityErrorObservation::from_field(value),
            Ieee802154TxSecurityErrorObservation::Named(reason)
        );
    }
    for value in 6..=0x0f {
        assert_eq!(
            Ieee802154TxSecurityErrorObservation::from_field(value),
            Ieee802154TxSecurityErrorObservation::Unclassified
        );
    }
}

/// `ieee802154_ll_is_current_rx_frame` is `rx_state > RECEIVE_SFD`.
#[test]
fn a_frame_is_in_progress_only_after_the_sfd_state() {
    for state in 0..=Ieee802154RxStateCode::MAX {
        assert_eq!(
            Ieee802154RxStateCode::from_field(state).is_after_receive_sfd(),
            state > Ieee802154RxStateCode::RECEIVE_SFD
        );
    }
}
