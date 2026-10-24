//! W4 observations and bounded ICMP control. None of these records carries
//! SSIDs, passphrases, PMKs or traffic keys. KeyID is a public CCMP header field.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProbeStation {
    /// Only 100 (main window) or 10 (reconnect window) is admitted.
    pub count: u16,
    pub identifier: u16,
}
impl ProbeStation {
    pub const fn valid(self) -> bool {
        matches!(self.count, 10 | 100)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationProbeReady {
    pub epoch: u32,
    pub identifier: u16,
    pub count: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationProbeFinished {
    pub epoch: u32,
    pub identifier: u16,
    pub sent: u16,
    pub received: u16,
    pub peer_requests: u16,
    pub peer_replies: u16,
    pub malformed: u32,
    pub queue_drops: u32,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationCounters {
    pub epoch: u32,
    pub tx_timeouts: u32,
    pub queue_collisions: u32,
    pub rx_refused: u32,
    pub deauth_transmissions: u32,
    pub group_generation: u32,
    pub group_key_id: u8,
    /// Decoded broadcast ARP MSDUs received since this group key was installed.
    pub group_arp_received: u32,
    pub irq_free: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStationCounters;
/// Public correlation fields for an ARP observed before TX encryption or
/// after validated RX decryption. No MAC address or key material is exported.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationArpFrame {
    pub epoch: u32,
    pub generation: u32,
    pub transmitted: bool,
    pub group_addressed: bool,
    pub operation: u16,
    pub sender: [u8; 4],
    pub target: [u8; 4],
    pub sequence: u16,
    pub packet_number: u64,
    pub key_id: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationGroupKey {
    pub epoch: u32,
    pub generation: u32,
    pub key_id: u8,
    pub replaced: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopStation;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationStopped(pub StationCounters);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_probe_counts_are_bounded_before_starting_work() {
        for count in 0..=u16::MAX {
            assert_eq!(
                ProbeStation {
                    count,
                    identifier: 7
                }
                .valid(),
                matches!(count, 10 | 100)
            );
        }
    }
}
