//! Owned copies of the portable outcomes.

use oer_bluetooth_radio::{
    ConnectionId, EventId, EventResult, LeInstant, RadioOutcome, ReceivedPdu, TestReport,
};
use oer_esp32s31_bluetooth_memory::BLUETOOTH_LE_RX_PAYLOAD_CAPACITY;

/// The longest Link Layer PDU: a two-byte header and the longest payload
/// one receive allocation holds, which covers 251 octets of data and an
/// encrypted PDU's MIC.
pub const MAX_PDU_BYTES: usize = 2 + BLUETOOTH_LE_RX_PAYLOAD_CAPACITY;

/// A received PDU copied out of its receive chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothReceivedPdu {
    bytes: [u8; MAX_PDU_BYTES],
    len: u8,
    /// Receive strength.
    pub rssi_dbm: i8,
    /// The on-air start, or `None` when the hardware captured none.
    pub captured_at: Option<LeInstant>,
}

impl BluetoothReceivedPdu {
    #[cfg(any(target_arch = "riscv32", test))]
    fn copy(pdu: ReceivedPdu<'_>) -> Option<Self> {
        let len = u8::try_from(pdu.pdu.len())
            .ok()
            .filter(|len| usize::from(*len) <= MAX_PDU_BYTES)?;
        let mut bytes = [0; MAX_PDU_BYTES];
        bytes[..usize::from(len)].copy_from_slice(pdu.pdu);
        Some(Self {
            bytes,
            len,
            rssi_dbm: pdu.rssi_dbm,
            captured_at: pdu.captured_at,
        })
    }

    /// The complete PDU: two-byte header and payload.
    pub fn pdu(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Lend the PDU as a portable value.
    pub fn portable(&self) -> ReceivedPdu<'_> {
        ReceivedPdu {
            pdu: self.pdu(),
            rssi_dbm: self.rssi_dbm,
            captured_at: self.captured_at,
        }
    }
}

/// An owned portable [`RadioOutcome`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the no-alloc outcome queue stores received PDUs inline"
)]
pub enum BluetoothOutcome {
    /// A PDU received during the event.
    Received {
        /// The event.
        id: EventId,
        /// The PDU.
        pdu: BluetoothReceivedPdu,
    },
    /// The event ended.
    EventEnded {
        /// The event.
        id: EventId,
        /// How it ended.
        result: EventResult,
    },
    /// The peer acknowledged the connection's queued PDU.
    TransmitAcknowledged(ConnectionId),
    /// A Direct Test Mode receiver event's result.
    TestReport {
        /// The event.
        id: EventId,
        /// The result.
        report: TestReport,
    },
}

impl BluetoothOutcome {
    /// Copy `outcome`; `None` for a PDU longer than any Link Layer PDU,
    /// which the runtime takes as a memory-inconsistency fault.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn copy(outcome: RadioOutcome<'_>) -> Option<Self> {
        Some(match outcome {
            RadioOutcome::Received { id, pdu } => Self::Received {
                id,
                pdu: BluetoothReceivedPdu::copy(pdu)?,
            },
            RadioOutcome::EventEnded { id, result } => Self::EventEnded { id, result },
            RadioOutcome::TransmitAcknowledged(connection) => {
                Self::TransmitAcknowledged(connection)
            }
            RadioOutcome::TestReport { id, report } => Self::TestReport { id, report },
        })
    }

    /// Lend the outcome as a portable value.
    pub fn portable(&self) -> RadioOutcome<'_> {
        match self {
            Self::Received { id, pdu } => RadioOutcome::Received {
                id: *id,
                pdu: pdu.portable(),
            },
            Self::EventEnded { id, result } => RadioOutcome::EventEnded {
                id: *id,
                result: *result,
            },
            Self::TransmitAcknowledged(connection) => {
                RadioOutcome::TransmitAcknowledged(*connection)
            }
            Self::TestReport { id, report } => RadioOutcome::TestReport {
                id: *id,
                report: *report,
            },
        }
    }
}
