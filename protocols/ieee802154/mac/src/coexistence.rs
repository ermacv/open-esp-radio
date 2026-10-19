//! What the radio is doing, for the radios it shares the air with.

use oer_radio_coex::{AirClass, Operation, RadioClient};

/// An operation of the IEEE 802.15.4 radio on the shared air.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioOperation {
    /// Nothing runs; the radio keeps only its idle claim.
    Idle,
    /// Immediate transmission.
    Tx,
    /// Reception, energy detection and CCA.
    Rx,
    /// Timed transmission.
    TxAt,
    /// Timed reception.
    RxAt,
    /// The acknowledgement of a received frame.
    Ack,
    /// One Coordinated Sampled Listening receive slot.
    CslSlot,
}

impl Operation for RadioOperation {
    const CLIENT: RadioClient = RadioClient::Ieee802154;

    fn class(&self) -> AirClass {
        match self {
            Self::TxAt | Self::RxAt | Self::CslSlot => AirClass::HardDeadline,
            Self::Idle | Self::Tx | Self::Rx | Self::Ack => AirClass::PtiOnly,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_operations_and_csl_slots_have_hard_deadlines() {
        for operation in [
            RadioOperation::TxAt,
            RadioOperation::RxAt,
            RadioOperation::CslSlot,
        ] {
            assert_eq!(operation.class(), AirClass::HardDeadline);
        }
        for operation in [
            RadioOperation::Idle,
            RadioOperation::Tx,
            RadioOperation::Rx,
            RadioOperation::Ack,
        ] {
            assert_eq!(operation.class(), AirClass::PtiOnly);
        }
    }
}
