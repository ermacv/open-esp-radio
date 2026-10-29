//! On-device UDP workloads using the pinned stack's socket API.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};
pub const MAX_DATAGRAM_PAYLOAD: u16 = 1400;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum DatagramMode {
    Echo,
    Sink,
    Source,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartDatagrams {
    pub mode: DatagramMode,
    pub local_port: u16,
    pub peer: Option<super::Ipv4Endpoint>,
    pub payload_bytes: u16,
    /// Source packets per second; None sends as quickly as socket capacity permits.
    pub packets_per_second: Option<u32>,
}
impl StartDatagrams {
    pub fn valid(&self) -> bool {
        self.local_port != 0
            && self.payload_bytes >= DatagramHeader::LENGTH as u16
            && self.payload_bytes <= MAX_DATAGRAM_PAYLOAD
            && match self.mode {
                DatagramMode::Source => {
                    matches!(self.peer, Some(peer) if peer.port != 0 && peer.address != [0; 4])
                }
                _ => self.peer.is_none() && self.packets_per_second.is_none(),
            }
            && match self.packets_per_second {
                Some(rate) => (1..=1_000_000).contains(&rate),
                None => true,
            }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopDatagrams;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum DatagramEnd {
    Stopped,
    SocketCapacity,
    Bind,
    Receive,
    Send,
    Deadline,
    SequenceExhausted,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DatagramCounters {
    pub received_packets: u64,
    pub received_bytes: u64,
    pub sent_packets: u64,
    pub sent_bytes: u64,
    /// Unfilled sequence gaps, starting at sequence zero; late arrivals within
    /// the 64-packet window correct these provisional gaps.
    pub missing: u64,
    pub reordered: u64,
    pub duplicates: u64,
    pub malformed: u64,
    pub too_old: u64,
    pub saturated: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DatagramsFinished {
    pub start_request: u32,
    pub started_us: u64,
    pub ended_us: u64,
    pub end: DatagramEnd,
    pub counters: DatagramCounters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatagramHeader {
    pub sequence: u64,
    pub sent_us: u64,
}
impl DatagramHeader {
    pub const LENGTH: usize = 16;
    pub fn read(payload: &[u8]) -> Option<Self> {
        let (sequence, rest) = payload.split_first_chunk::<8>()?;
        let sent = rest.first_chunk::<8>()?;
        Some(Self {
            sequence: u64::from_be_bytes(*sequence),
            sent_us: u64::from_be_bytes(*sent),
        })
    }
    pub fn write(self, payload: &mut [u8]) -> bool {
        let Some((sequence, rest)) = payload.split_first_chunk_mut::<8>() else {
            return false;
        };
        let Some(sent) = rest.first_chunk_mut::<8>() else {
            return false;
        };
        *sequence = self.sequence.to_be_bytes();
        *sent = self.sent_us.to_be_bytes();
        true
    }
}
#[cfg(oer_trace)]
pub struct DatagramTracker {
    counters: DatagramCounters,
    highest: Option<u64>,
    seen: u64,
}
#[cfg(oer_trace)]
impl Default for DatagramTracker {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(oer_trace)]
impl DatagramTracker {
    pub const fn new() -> Self {
        Self {
            counters: DatagramCounters {
                received_packets: 0,
                received_bytes: 0,
                sent_packets: 0,
                sent_bytes: 0,
                missing: 0,
                reordered: 0,
                duplicates: 0,
                malformed: 0,
                too_old: 0,
                saturated: false,
            },
            highest: None,
            seen: 0,
        }
    }
    pub fn counters(&self) -> DatagramCounters {
        self.counters
    }
    fn add(value: &mut u64, delta: u64, saturated: &mut bool) {
        match value.checked_add(delta) {
            Some(next) => *value = next,
            None => {
                *value = u64::MAX;
                *saturated = true;
            }
        }
    }
    pub fn sent(&mut self, bytes: u16) {
        Self::add(
            &mut self.counters.sent_packets,
            1,
            &mut self.counters.saturated,
        );
        Self::add(
            &mut self.counters.sent_bytes,
            u64::from(bytes),
            &mut self.counters.saturated,
        );
    }
    pub fn received(&mut self, payload: &[u8]) {
        Self::add(
            &mut self.counters.received_packets,
            1,
            &mut self.counters.saturated,
        );
        Self::add(
            &mut self.counters.received_bytes,
            payload.len() as u64,
            &mut self.counters.saturated,
        );
        let Some(header) = DatagramHeader::read(payload) else {
            Self::add(
                &mut self.counters.malformed,
                1,
                &mut self.counters.saturated,
            );
            return;
        };
        match self.highest {
            None => {
                self.highest = Some(header.sequence);
                self.seen = 1;
                Self::add(
                    &mut self.counters.missing,
                    header.sequence,
                    &mut self.counters.saturated,
                );
            }
            Some(highest) if header.sequence > highest => {
                let advance = header.sequence.saturating_sub(highest);
                Self::add(
                    &mut self.counters.missing,
                    advance.saturating_sub(1),
                    &mut self.counters.saturated,
                );
                self.seen = if advance >= 64 {
                    1
                } else {
                    self.seen.wrapping_shl(advance as u32) | 1
                };
                self.highest = Some(header.sequence);
            }
            Some(highest) => {
                let behind = highest.saturating_sub(header.sequence);
                if behind >= 64 {
                    Self::add(&mut self.counters.too_old, 1, &mut self.counters.saturated);
                    return;
                }
                let bit = 1_u64.wrapping_shl(behind as u32);
                if self.seen & bit != 0 {
                    Self::add(
                        &mut self.counters.duplicates,
                        1,
                        &mut self.counters.saturated,
                    );
                } else {
                    self.seen |= bit;
                    self.counters.missing = self.counters.missing.saturating_sub(1);
                    Self::add(
                        &mut self.counters.reordered,
                        1,
                        &mut self.counters.saturated,
                    );
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_is_big_endian_and_short_payloads_are_refused() {
        let value = DatagramHeader {
            sequence: u64::MAX,
            sent_us: 17,
        };
        let mut bytes = [0; 16];
        assert!(value.write(&mut bytes));
        assert_eq!(DatagramHeader::read(&bytes), Some(value));
        assert_eq!(bytes[15], 17);
        assert!(!value.write(&mut [0; 15]));
        assert!(DatagramHeader::read(&[0; 15]).is_none());
    }
    #[cfg(oer_trace)]
    #[test]
    fn late_packets_correct_gaps_duplicates_do_not_and_window_exhaustion_is_explicit() {
        let mut tracker = DatagramTracker::new();
        let mut bytes = [0; 16];
        for sequence in [0, 3, 2, 2, 1, 68, 0] {
            assert!(
                DatagramHeader {
                    sequence,
                    sent_us: 0
                }
                .write(&mut bytes)
            );
            tracker.received(&bytes);
        }
        let counters = tracker.counters();
        assert_eq!(counters.received_packets, 7);
        assert_eq!(counters.received_bytes, 112);
        assert_eq!(counters.missing, 64);
        assert_eq!(counters.reordered, 2);
        assert_eq!(counters.duplicates, 1);
        assert_eq!(counters.too_old, 1);
        tracker.received(&[0; 15]);
        assert_eq!(tracker.counters().malformed, 1);
        tracker.counters.sent_packets = u64::MAX;
        tracker.sent(16);
        assert!(tracker.counters().saturated);
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn source_refuses_invalid_bounds_rate_and_peer_before_socket_construction() {
        let source = StartDatagrams {
            mode: DatagramMode::Source,
            local_port: 9000,
            peer: Some(super::super::Ipv4Endpoint {
                address: [10, 0, 0, 1],
                port: 9001,
            }),
            payload_bytes: MAX_DATAGRAM_PAYLOAD,
            packets_per_second: Some(1),
        };
        assert!(source.valid());
        assert!(
            !StartDatagrams {
                payload_bytes: 15,
                ..source
            }
            .valid()
        );
        assert!(
            !StartDatagrams {
                payload_bytes: MAX_DATAGRAM_PAYLOAD.saturating_add(1),
                ..source
            }
            .valid()
        );
        assert!(
            !StartDatagrams {
                packets_per_second: Some(0),
                ..source
            }
            .valid()
        );
        assert!(
            !StartDatagrams {
                packets_per_second: Some(1_000_001),
                ..source
            }
            .valid()
        );
        assert!(
            !StartDatagrams {
                peer: None,
                ..source
            }
            .valid()
        );
        assert!(
            !StartDatagrams {
                local_port: 0,
                ..source
            }
            .valid()
        );
        assert!(
            StartDatagrams {
                packets_per_second: None,
                ..source
            }
            .valid()
        );
    }
}
