//! What the port's requests are, for the radio they share the air with.
//!
//! A request names the operation it asks the air for; the chip family's
//! coexistence owner maps it to an arbitration priority. There is no urgency
//! level.

use oer_radio_coex::{AirClass, Operation as CoexOperation, RadioClient};

/// A frame of the connection's setup or keys, which a coexisting radio lets
/// through first.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConnectionFrame {
    /// A Probe Request.
    ProbeRequest,
    /// An Authentication frame, SAE included.
    Authentication,
    /// An Association or Reassociation Request.
    Association,
    /// An EAPOL frame, flagged by its EtherType at encapsulation.
    Eapol,
}

/// What one transmit attempt is.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxOperation {
    /// An ordinary frame, served at the priority of the attempt's access
    /// category.
    Data,
    /// A frame of the connection's setup or keys.
    Connection(ConnectionFrame),
}

/// An operation of the IEEE 802.11 port on the shared air.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    /// The window after a TBTT that a dozing station keeps open for the
    /// beacon.
    BeaconWindow,
    /// One transmit attempt.
    Transmit(TxOperation),
}

impl CoexOperation for Operation {
    const CLIENT: RadioClient = RadioClient::Wifi;

    fn class(&self) -> AirClass {
        match self {
            Self::BeaconWindow => AirClass::HardDeadline,
            Self::Transmit(_) => AirClass::PtiOnly,
        }
    }
}

impl From<TxOperation> for Operation {
    fn from(operation: TxOperation) -> Self {
        Self::Transmit(operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_beacon_window_has_a_hard_deadline() {
        assert_eq!(Operation::BeaconWindow.class(), AirClass::HardDeadline);
        for operation in [
            TxOperation::Data,
            TxOperation::Connection(ConnectionFrame::Authentication),
        ] {
            assert_eq!(Operation::from(operation).class(), AirClass::PtiOnly);
        }
    }
}
