//! Network traffic session configuration.

use super::SESSION_FLOW_CAPACITY;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Logical network endpoint backed by the shared physical Wi-Fi owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiNetworkInterface {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Transport {
    Udp,
    Tcp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Direction {
    Rx,
    Tx,
    Bidirectional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Completion {
    DurationMillis(u32),
    TransferBytes(u64),
    HostStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ipv4Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FlowConfig {
    pub payload_bytes: u16,
    pub offered_rate_bps: Option<u64>,
    /// Number of datagrams admitted at one offered-rate deadline.
    ///
    /// `None` selects the target's bounded throughput-oriented default. A
    /// small explicit value lets fairness HIL describe sparse packet bursts
    /// without turning a low average bitrate into one queue-sized burst.
    pub pacing_group_datagrams: Option<u8>,
}

/// One independently identifiable flow inside a network-interface session.
///
/// `peer` is the target's UDP transmit destination when `target_tx` is present.
/// For receive-only sessions it may be absent when there is exactly one flow;
/// multi-flow receive sessions require a peer so the target can classify the
/// source without combining independent sequence spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionFlowConfig {
    pub flow_id: u8,
    pub peer: Option<Ipv4Endpoint>,
    pub target_rx: Option<FlowConfig>,
    pub target_tx: Option<FlowConfig>,
    /// Optional exact identity in both application-payload directions of a
    /// bounded UDP session. Old packets from an earlier lifecycle stage must
    /// not be counted as recovery of the newly configured station session.
    pub payload_identity: Option<UdpSessionPayloadIdentity>,
}

/// Eight-byte stage identity following the four-byte benchmark UDP sequence.
/// Identified payloads retain the existing `0x5a` benchmark fill after that
/// header so either consumer can reject altered application content.
/// It is an application-payload contract, not a radio generation or a source
/// of independent HIL traffic evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UdpSessionPayloadIdentity(u64);

impl UdpSessionPayloadIdentity {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub fn write_to(self, payload: &mut [u8]) -> bool {
        let Some(bytes) = payload.get_mut(4..12) else {
            return false;
        };
        bytes.copy_from_slice(&self.0.to_be_bytes());
        true
    }

    pub fn matches(self, payload: &[u8]) -> bool {
        Self::from_payload(payload) == Some(self) && Self::fill_matches(payload)
    }

    /// Accept only the next measured data packet in an identified RX flow.
    /// Terminal negative markers are handled separately by the receiver.
    pub fn accepts_next_data(
        self,
        observed: Option<Self>,
        fill_matches: bool,
        sequence: Option<i32>,
        accepted_units: u64,
    ) -> bool {
        observed == Some(self) && fill_matches && sequence == i32::try_from(accepted_units).ok()
    }

    pub fn fill_matches(payload: &[u8]) -> bool {
        payload
            .get(12..)
            .is_some_and(|tail| tail.iter().all(|byte| *byte == 0x5a))
    }

    pub fn fill_after_header(payload: &mut [u8]) -> bool {
        let Some(tail) = payload.get_mut(12..) else {
            return false;
        };
        tail.fill(0x5a);
        true
    }

    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        let bytes: [u8; 8] = payload.get(4..12)?.try_into().ok()?;
        Some(Self(u64::from_be_bytes(bytes)))
    }
}

/// Link properties that must be true before a measured transport session may
/// advertise readiness.
///
/// Correctness cells deliberately use [`Self::NONE`]: a standards-compliant
/// peer may reject aggregation and the data plane must still work. Throughput
/// cells can require one negotiated TX BlockAck TID so an absent AddBA
/// response is reported as unavailable test precondition instead of being
/// misclassified as slow S-MPDU performance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionLinkRequirements {
    pub tx_block_ack_tid: Option<u8>,
}

impl SessionLinkRequirements {
    pub const NONE: Self = Self {
        tx_block_ack_tid: None,
    };

    pub const fn tx_block_ack(tid: u8) -> Self {
        Self {
            tx_block_ack_tid: Some(tid),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionConfig {
    /// Exact network endpoint that owns this transport session. The physical
    /// radio may expose both endpoints during one same-channel STA+AP epoch;
    /// transport ownership must therefore never be inferred from the Wi-Fi
    /// role or from whichever stack became ready first.
    pub network_interface: WifiNetworkInterface,
    pub transport: Transport,
    pub direction: Direction,
    pub completion: Completion,
    /// Contiguous flow table. Slot zero is always occupied. A second occupied
    /// slot represents another UDP peer on the same network interface and is
    /// accepted only when the image advertises [`super::UdpMultiFlow`].
    pub flows: [Option<SessionFlowConfig>; SESSION_FLOW_CAPACITY],
    pub link_requirements: SessionLinkRequirements,
}

impl SessionConfig {
    pub const fn primary_flow(self) -> Option<SessionFlowConfig> {
        self.flows[0]
    }

    pub fn active_flow_count(self) -> usize {
        self.flows.iter().flatten().count()
    }

    /// Validate the target-neutral bounded session shape.
    ///
    /// Runtime feature availability remains the target's responsibility. This
    /// method owns the wire invariants so host/model tests exercise the same
    /// rules as embedded admission.
    pub fn structurally_valid(self, maximum_payload_bytes: u16, udp_multi_flow: bool) -> bool {
        let flow_count = self.active_flow_count();
        if flow_count == 0 || self.flows[0].is_none() {
            return false;
        }
        let mut saw_empty = false;
        for flow in self.flows {
            match flow {
                Some(_) if saw_empty => return false,
                Some(_) => {}
                None => saw_empty = true,
            }
        }
        if flow_count > 1 && (self.transport != Transport::Udp || !udp_multi_flow) {
            return false;
        }

        let valid_flow = |flow: FlowConfig| {
            flow.payload_bytes >= 64
                && flow.payload_bytes <= maximum_payload_bytes
                && flow
                    .offered_rate_bps
                    .is_none_or(|rate| (1_000..=1_000_000_000).contains(&rate))
                && flow
                    .pacing_group_datagrams
                    .is_none_or(|datagrams| datagrams != 0 && flow.offered_rate_bps.is_some())
        };
        let identities_valid = self
            .flows
            .iter()
            .flatten()
            .enumerate()
            .all(|(index, flow)| {
                self.flows[..index]
                    .iter()
                    .flatten()
                    .all(|earlier| earlier.flow_id != flow.flow_id)
                    && flow.peer.is_none_or(|peer| {
                        peer.port != 0
                            && self.flows[..index]
                                .iter()
                                .flatten()
                                .filter_map(|earlier| earlier.peer)
                                .all(|earlier| earlier != peer)
                    })
            });
        let pacing_valid = self.flows.iter().flatten().all(|flow| {
            flow.target_rx
                .is_none_or(|rx| rx.pacing_group_datagrams.is_none())
                && flow.target_tx.is_none_or(|tx| {
                    tx.pacing_group_datagrams.is_none() || self.transport == Transport::Udp
                })
        });
        let payload_identities_valid = self
            .flows
            .iter()
            .flatten()
            .all(|flow| flow.payload_identity.is_none() || self.transport == Transport::Udp);
        let peers_valid = match (self.transport, self.direction) {
            (Transport::Tcp, _) => {
                flow_count == 1 && self.flows.iter().flatten().all(|flow| flow.peer.is_none())
            }
            (Transport::Udp, Direction::Rx) if flow_count == 1 => {
                self.flows.iter().flatten().all(|flow| flow.peer.is_none())
            }
            (Transport::Udp, Direction::Rx) => {
                self.flows.iter().flatten().all(|flow| flow.peer.is_some())
            }
            (Transport::Udp, Direction::Tx | Direction::Bidirectional) => {
                self.flows.iter().flatten().all(|flow| flow.peer.is_some())
            }
        };
        let direction_valid = match self.direction {
            Direction::Rx => self
                .flows
                .iter()
                .flatten()
                .all(|flow| flow.target_rx.is_some_and(valid_flow) && flow.target_tx.is_none()),
            Direction::Tx => self
                .flows
                .iter()
                .flatten()
                .all(|flow| flow.target_rx.is_none() && flow.target_tx.is_some_and(valid_flow)),
            Direction::Bidirectional => self.flows.iter().flatten().all(|flow| {
                flow.target_rx.is_some_and(valid_flow) && flow.target_tx.is_some_and(valid_flow)
            }),
        };
        let link_requirements_valid = match self.link_requirements.tx_block_ack_tid {
            None => true,
            Some(tid) => {
                tid < 8
                    && matches!(self.direction, Direction::Tx | Direction::Bidirectional)
                    && self
                        .flows
                        .iter()
                        .flatten()
                        .all(|flow| flow.target_tx.is_some())
            }
        };

        identities_valid
            && pacing_valid
            && payload_identities_valid
            && peers_valid
            && direction_valid
            && link_requirements_valid
            && matches!(self.completion, Completion::DurationMillis(duration) if (1..=300_000).contains(&duration))
    }
}

/// Data-plane readiness together with the exact link requirement proved by
/// the target before accepting measured traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionReady {
    pub direction: Direction,
    pub tx_block_ack_tid: Option<u8>,
}
