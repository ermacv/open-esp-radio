//! Host-controlled DTM on LE 1M, channel 0, 37-byte PRBS9 payloads.
//! An active test has a fixed 30-second lease; expiry is a failed observation.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Exact ACL payload used by the peripheral recovery workload.
///
/// The first four bytes form one complete L2CAP-shaped header. The full packet
/// reaches the Controller's declared 251-byte ACL limit and therefore crosses
/// ten legacy 27-byte Link Layer Data PDUs in the no-DLE test profile.
pub const BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES: usize = 251;

/// Number of legacy Data PDUs needed for the recovery workload's ACL payload.
pub const BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS: u32 = 10;

/// Exact post-establishment connection interval requested by the Linux central.
pub const BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS: u16 = 120;

/// Construct one sequenced deterministic payload shared by both HIL Hosts.
pub const fn bluetooth_peripheral_acl_payload_for_sequence(
    sequence: u8,
) -> [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] {
    let mut payload = [0; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES];
    let l2cap_payload = (BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES - 4) as u16;
    let length = l2cap_payload.to_le_bytes();
    payload[0] = length[0];
    payload[1] = length[1];
    payload[2] = 0xff;
    payload[3] = 0xff;
    let mut index = 4;
    while index < payload.len() {
        payload[index] = (((index as u16 * 73 + 19) & 0xff) as u8) ^ sequence;
        index += 1;
    }
    payload
}

/// Construct the first deterministic peripheral ACL payload.
pub const fn bluetooth_peripheral_acl_payload() -> [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] {
    bluetooth_peripheral_acl_payload_for_sequence(0)
}

/// How one peripheral HIL cycle asks the established connection to end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum BluetoothPeripheralTermination {
    #[default]
    PeerReset,
    PeerRfkill,
    TargetDisconnect,
    TargetReset,
}

/// Public, non-secret key material for the finite encrypted ACL fixture only.
pub const BLUETOOTH_TEST_LTK: [u8; 16] = [0x35; 16];
pub const BLUETOOTH_TEST_RAND: [u8; 8] = [0x27; 8];
pub const BLUETOOTH_TEST_EDIV: u16 = 0x1937;
/// Distinct public key identity for in-connection refresh of the diagnostic session.
pub const BLUETOOTH_REFRESH_LTK: [u8; 16] = [0x6a; 16];
pub const BLUETOOTH_REFRESH_RAND: [u8; 8] = [0x58; 8];
pub const BLUETOOTH_REFRESH_EDIV: u16 = 0x2849;

/// One deliberate initial-key failure or missing refresh key; subsequent connections use valid keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum BluetoothSecurityFailure {
    MissingKey,
    WrongKey,
    MissingRefreshKey,
    /// Corrupt one active data MIC after RF receive, before software authentication.
    ActiveDataMic,
}

impl core::str::FromStr for BluetoothSecurityFailure {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "missing-key" => Ok(Self::MissingKey),
            "wrong-key" => Ok(Self::WrongKey),
            "missing-refresh-key" => Ok(Self::MissingRefreshKey),
            "active-data-mic" => Ok(Self::ActiveDataMic),
            _ => Err("expected missing-key, wrong-key, missing-refresh-key or active-data-mic"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum BluetoothDtmOperation {
    Reset,
    Receive,
    Transmit,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum BluetoothDtmResult {
    Complete { received_packets: Option<u16> },
    HciRejected,
    Timeout,
    LeaseExpired,
}

/// Longest HCI command parameter block a request carries. It covers every
/// legacy command and LE Extended Create Connection for all three PHYs;
/// longer advertising data travels in the fragments HCI defines for it, and
/// the bound keeps a decoded command within the embedded queue budget.
pub const BLUETOOTH_HCI_PARAMETER_BYTES: usize = 64;
/// Longest HCI event packet: event code, length and parameters.
pub const BLUETOOTH_HCI_EVENT_BYTES: usize = 2 + 255;

/// One raw HCI exchange with the image's Controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum BluetoothHciRequest {
    /// Send one command and wait for its Command Complete or Command Status;
    /// other events that arrive meanwhile stay queued for
    /// [`Self::NextEvent`].
    Command {
        opcode: u16,
        parameters: heapless::Vec<u8, BLUETOOTH_HCI_PARAMETER_BYTES>,
    },
    /// Return the oldest queued Controller event, waiting up to `wait_ms`.
    NextEvent { wait_ms: u16 },
}

/// The image's answer to one [`BluetoothHciRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum BluetoothHciResponse {
    /// The command's Command Complete or Command Status event.
    Completed(heapless::Vec<u8, BLUETOOTH_HCI_EVENT_BYTES>),
    /// One Controller event.
    Event {
        packet: heapless::Vec<u8, BLUETOOTH_HCI_EVENT_BYTES>,
        /// Events dropped because the queue was full, since the last report.
        dropped: u16,
    },
    /// No event arrived in time.
    NoEvent,
    /// The command did not complete in time.
    Timeout,
    /// The Host transport failed.
    TransportFailed,
}

use crate::base::ResetReason;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct BluetoothDtmEvidence {
    pub reset_reason: ResetReason,
    pub operation: BluetoothDtmOperation,
    pub result: BluetoothDtmResult,
    pub rx_diagnostics: BluetoothDtmRxDiagnostics,
}

/// Cumulative production RX recycle observations since this firmware boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct BluetoothDtmRxDiagnostics {
    pub sequence_checks: u32,
    pub sequence_deadline_rejections: u32,
    pub last_sequence_lead_ticks: i32,
    pub successful_events: u32,
    pub failed_events: u32,
    pub last_failure_status: u32,
    pub empty_events: u32,
    pub counted_packets: u32,
    pub rejected_packets: u32,
}

impl BluetoothDtmEvidence {
    pub fn completed(self, requested: BluetoothDtmOperation) -> bool {
        self.operation == requested
            && matches!(self.result,
            BluetoothDtmResult::Complete { received_packets }
                if received_packets.is_some() == (requested == BluetoothDtmOperation::End))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removed_power_off_variant_is_not_decodable() {
        assert!(postcard::from_bytes::<BluetoothPeripheralTermination>(&[4]).is_err());
        for mode in [
            BluetoothPeripheralTermination::PeerReset,
            BluetoothPeripheralTermination::PeerRfkill,
            BluetoothPeripheralTermination::TargetDisconnect,
            BluetoothPeripheralTermination::TargetReset,
        ] {
            let mut storage = [0; 8];
            let bytes = postcard::to_slice(&mode, &mut storage).unwrap();
            assert_eq!(
                postcard::from_bytes::<BluetoothPeripheralTermination>(bytes).unwrap(),
                mode
            );
        }
    }
    #[test]
    fn peripheral_acl_probe_fills_the_legacy_fragmentation_envelope() {
        let payload = bluetooth_peripheral_acl_payload();
        let second = bluetooth_peripheral_acl_payload_for_sequence(1);
        assert_eq!(payload.len(), BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES);
        assert_eq!(u16::from_le_bytes([payload[0], payload[1]]), 247);
        assert_eq!(&payload[2..4], &[0xff, 0xff]);
        assert_eq!(
            payload.len().div_ceil(27),
            BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS as usize
        );
        assert!(
            payload[4..]
                .windows(27)
                .all(|window| window[0] != window[26])
        );
        assert_eq!(&payload[..4], &second[..4]);
        assert!(payload[4..].iter().zip(&second[4..]).all(|(a, b)| a != b));
    }

    #[test]
    fn rx_deadline_diagnostics_survive_framed_transport() {
        let expected = crate::Envelope::new(
            3,
            4,
            0,
            5,
            crate::bluetooth::DtmResult(BluetoothDtmEvidence {
                reset_reason: ResetReason::Other,
                operation: BluetoothDtmOperation::End,
                result: BluetoothDtmResult::Complete {
                    received_packets: Some(0),
                },
                rx_diagnostics: BluetoothDtmRxDiagnostics {
                    sequence_checks: u32::MAX,
                    sequence_deadline_rejections: 1371,
                    last_sequence_lead_ticks: -1,
                    successful_events: 2,
                    ..Default::default()
                },
            }),
        );
        let mut encoder = crate::FrameEncoder::new();
        let mut decoder = crate::FrameDecoder::new();
        let mut observed = None;
        decoder.feed(
            crate::WireKind::Event,
            encoder.encode(&expected).unwrap(),
            |frame| {
                observed = frame
                    .unwrap()
                    .decode::<crate::bluetooth::DtmResult>()
                    .map(Result::unwrap)
            },
        );
        assert_eq!(observed, Some(expected));
    }
    #[test]
    fn only_correlated_complete_results_with_correct_count_scope_pass() {
        let evidence = BluetoothDtmEvidence {
            reset_reason: ResetReason::Other,
            rx_diagnostics: BluetoothDtmRxDiagnostics::default(),
            operation: BluetoothDtmOperation::End,
            result: BluetoothDtmResult::Complete {
                received_packets: Some(0),
            },
        };
        assert!(evidence.completed(BluetoothDtmOperation::End));
        assert!(!evidence.completed(BluetoothDtmOperation::Receive));
        for result in [
            BluetoothDtmResult::HciRejected,
            BluetoothDtmResult::Timeout,
            BluetoothDtmResult::LeaseExpired,
            BluetoothDtmResult::Complete {
                received_packets: None,
            },
        ] {
            assert!(
                !BluetoothDtmEvidence { result, ..evidence }.completed(BluetoothDtmOperation::End)
            );
        }
    }
}
