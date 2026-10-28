//! Split-MAC offload boundaries, service capabilities, receive metadata and
//! the terminal status of one logical MPDU exchange.

use crate::{HardwareServices, RxCryptoStatus, RxEvidence, TxStatus};
use oer_ieee80211_wire::{channel::Channel, phy::PhyRate};


/// Owner that must perform one indivisible MAC operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOperationOwner {
    /// The MAC service delegates the operation to radio hardware.
    Hardware,
    /// Source-owned software performs the operation.
    Software,
    /// The complete MAC service does not currently provide the operation.
    Unsupported,
}

/// Independently stated offload boundaries for a split-MAC implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacOperationOwnership {
    /// Append the transmit FCS to an encoded MPDU.
    pub tx_fcs_generation: MacOperationOwner,
    /// Send the immediate ACK response required by the receive exchange.
    pub immediate_ack_response: MacOperationOwner,
    /// Count down an already selected CSMA/CA backoff and arbitrate the medium.
    pub csma_ca_backoff_countdown: MacOperationOwner,
    /// Update contention state, choose retry rates and decide whether to retry.
    pub unicast_retry_policy: MacOperationOwner,
    /// Allocate per-interface/per-TID 802.11 sequence numbers.
    pub tx_sequence_assignment: MacOperationOwner,
    /// Select the logical key used by an outgoing protected MPDU.
    pub ccmp_key_selection: MacOperationOwner,
    /// Allocate and encode the outgoing CCMP packet number.
    pub ccmp_packet_number: MacOperationOwner,
    /// Perform the CCMP payload transform and append/verify its MIC.
    pub ccmp_transform: MacOperationOwner,
    /// Match incoming A-MPDU sequence spaces against an active BA agreement.
    pub rx_block_ack_matching: MacOperationOwner,
    /// Reorder received MPDUs and decide when gaps may be released.
    pub rx_reorder: MacOperationOwner,
    /// Capture the transmit BlockAck starting sequence and bitmap.
    pub tx_block_ack_capture: MacOperationOwner,
    /// Select and retain missing MPDUs for a subsequent A-MPDU attempt.
    pub tx_ampdu_retry_selection: MacOperationOwner,
}

/// End-to-end resource limits visible to portable HMAC policy.
///
/// These are service limits, after hardware and source-owned storage limits
/// have both been applied.  They are not a claim about the total resources
/// physically present in a chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacResourceLimits {
    /// Simultaneously active radio channel contexts.
    pub channel_contexts: u8,
    /// Ordinary WMM/EDCA transmit queues exposed by the service.
    pub ordinary_tx_queues: u8,
    /// Receive BlockAck agreement banks exposed by the service.
    pub rx_block_ack_entries: u8,
    /// Highest receive BlockAck TID accepted by the service.
    pub rx_block_ack_max_tid: u8,
    /// Maximum receive reorder window retained by the complete service.
    pub rx_block_ack_max_window: u16,
    /// Maximum transmit BlockAck window retained by the complete service.
    pub tx_block_ack_max_window: u16,
    /// Maximum MPDUs retained in one transmit aggregate.
    pub tx_ampdu_max_subframes: u16,
    /// Pairwise CCMP slots exposed for one station interface.
    pub station_pairwise_ccmp_slots: u8,
    /// Group CCMP slots exposed for one station interface.
    pub station_group_ccmp_slots: u8,
    /// Pairwise CCMP slots exposed for one access-point interface.
    pub access_point_pairwise_ccmp_slots: u8,
    /// Group CCMP slots exposed for one access-point interface.
    pub access_point_group_ccmp_slots: u8,
    /// Simultaneously associated peers owned by the AP protocol service.
    pub access_point_association_entries: u8,
    /// Associated peers which may own independent protected data ports.
    pub access_point_encrypted_clients: u8,
}

/// Virtual-interface roles implemented by the complete driver today.
///
/// This advertises source-owned service, not latent hardware ability. For
/// example, a chip with several address filters still reports zero AP VIFs
/// until the AP owner graph and lifecycle exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacInterfaceCapabilities {
    pub station_interfaces: u8,
    pub access_point_interfaces: u8,
    pub simultaneous_station_access_point: bool,
    /// A monitor tap can own the radio without a protocol VIF.
    pub standalone_monitor: bool,
    /// A monitor tap can remain active while a protocol VIF runs.
    pub monitor_with_interfaces: bool,
    pub raw_monitor_tap: bool,
    pub normalized_monitor_tap: bool,
    pub protocol_validated_monitor_tap: bool,
}

impl MacInterfaceCapabilities {
    pub const fn supports_role(self, role: crate::VifRole) -> bool {
        match role {
            crate::VifRole::Station => self.station_interfaces != 0,
            crate::VifRole::AccessPoint => self.access_point_interfaces != 0,
        }
    }

    pub const fn supports_monitor_tap(self, point: crate::MonitorTapPoint) -> bool {
        match point {
            crate::MonitorTapPoint::Raw => self.raw_monitor_tap,
            crate::MonitorTapPoint::Normalized => self.normalized_monitor_tap,
            crate::MonitorTapPoint::ProtocolValidated => self.protocol_validated_monitor_tap,
        }
    }
}

impl MacOperationOwnership {
    /// The operations delegated to hardware, as the lower-MAC port's
    /// capability set reports them.
    pub const fn hardware_services(self) -> HardwareServices {
        let mut services = HardwareServices::NONE;
        let owned = [
            (self.tx_fcs_generation, HardwareServices::FCS),
            (self.immediate_ack_response, HardwareServices::IMMEDIATE_ACK),
            (
                self.csma_ca_backoff_countdown,
                HardwareServices::BACKOFF_COUNTDOWN,
            ),
            (
                self.tx_sequence_assignment,
                HardwareServices::SEQUENCE_NUMBERS,
            ),
            (self.ccmp_key_selection, HardwareServices::KEY_SELECTION),
            (self.ccmp_transform, HardwareServices::CIPHER_TRANSFORM),
            (
                self.rx_block_ack_matching,
                HardwareServices::RX_BLOCK_ACK_MATCHING,
            ),
            (self.rx_reorder, HardwareServices::RX_REORDER),
            (
                self.tx_block_ack_capture,
                HardwareServices::TX_BLOCK_ACK_CAPTURE,
            ),
            (
                self.tx_ampdu_retry_selection,
                HardwareServices::AMPDU_RETRY_SELECTION,
            ),
        ];
        let mut remaining = owned.as_slice();
        while let Some(((owner, service), rest)) = remaining.split_first() {
            if matches!(owner, MacOperationOwner::Hardware) {
                services = services.union(*service);
            }
            remaining = rest;
        }
        services
    }
}

/// Complete portable description of one MAC service implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacServiceCapabilities {
    pub interfaces: MacInterfaceCapabilities,
    pub operations: MacOperationOwnership,
    pub resources: MacResourceLimits,
}

/// Portable metadata carried with one received MPDU.
///
/// `Rate` is the portable [`PhyRate`] unless a backend keeps its own typed
/// PHY record for chip-side consumers, following the same rule as
/// [`MacTxStatus`]; [`Self::map_rate`] converts it. A backend may publish the
/// physical fields immediately and leave semantic fields unavailable until
/// protocol validation has run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxMetadata<Rate = PhyRate> {
    /// The channel the frame was received on, when a backend observed it;
    /// a backend's configured channel is not per-frame evidence.
    pub channel: RxEvidence<Channel>,
    pub rate: RxEvidence<Rate>,
    pub rssi_dbm: RxEvidence<i8>,
    pub crypto: RxEvidence<RxCryptoStatus>,
    /// Whether the hardware marked this as an IEEE VHT/HE S-MPDU (an MPDU
    /// carried alone in an A-MPDU subframe with the delimiter EOF bit set).
    /// This is not a synonym for an ordinary non-aggregated MPDU.
    pub s_mpdu: RxEvidence<bool>,
    /// Whether this MPDU was carried in an A-MPDU container.
    ///
    /// This does not state how many MPDUs the container carried. In
    /// particular, an S-MPDU is the sole MPDU in a VHT/HE A-MPDU and
    /// therefore has both `s_mpdu=true` and `ampdu=true`. Provenance remains
    /// explicit because HT supplies an RXVECTOR Aggregation bit, while the
    /// VHT/HE PPDU format establishes A-MPDU containment by protocol rule.
    pub ampdu: RxEvidence<bool>,
    /// Whether this MPDU carries an A-MSDU payload.
    ///
    /// A-MPDU and A-MSDU are independent dimensions and deliberately retain
    /// independent evidence. Protocol parsing can prove the latter without
    /// proving the former.
    pub amsdu: RxEvidence<bool>,
}

impl<Rate> MacRxMetadata<Rate> {
    /// Metadata for a synthetic event or a boundary that has not observed any
    /// receive status. This is not a successful/zero-valued hardware sample.
    pub const fn unavailable() -> Self {
        Self {
            channel: RxEvidence::Unavailable,
            rate: RxEvidence::Unavailable,
            rssi_dbm: RxEvidence::Unavailable,
            crypto: RxEvidence::Unavailable,
            s_mpdu: RxEvidence::Unavailable,
            ampdu: RxEvidence::Unavailable,
            amsdu: RxEvidence::Unavailable,
        }
    }

    /// The same metadata with its rate converted, keeping the rate's
    /// provenance; a rate `convert` finds no value in becomes unavailable.
    /// A backend's chip-typed rate becomes the portable [`PhyRate`] this way.
    pub fn map_rate<U>(self, convert: impl FnOnce(Rate) -> Option<U>) -> MacRxMetadata<U> {
        MacRxMetadata {
            channel: self.channel,
            rate: self.rate.and_then(convert),
            rssi_dbm: self.rssi_dbm,
            crypto: self.crypto,
            s_mpdu: self.s_mpdu,
            ampdu: self.ampdu,
            amsdu: self.amsdu,
        }
    }
}

/// Terminal result of one logical MPDU exchange.
///
/// A logical exchange may contain several hardware publications.  This is
/// intentionally different from a raw completion status for one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxResult {
    /// Hardware completed the exchange successfully.
    Transmitted,
    /// The terminal publication ended with this status, which the retry
    /// policy does not retry; never [`TxStatus::Success`]. A backend keeps
    /// its raw completion code as its own diagnostic.
    HardwareFailure(TxStatus),
    /// Every permitted publication ended at the hardware ACK/CTS timeout edge.
    HardwareTimeout,
    /// Every permitted publication lost contention before an on-air attempt.
    CollisionLimit,
}

/// Normalized terminal status returned from a backend to portable MAC policy.
///
/// `Rate` is the portable [`PhyRate`] unless a backend keeps its own typed
/// rate. Either way the final rate is never reduced to an untyped integer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxStatus<Rate = PhyRate> {
    pub result: MacTxResult,
    /// Total hardware publications, including the initial attempt.
    pub attempts: u8,
    /// Rate used for the terminal hardware publication.
    pub final_rate: Rate,
    /// `Some(true/false)` only when the receiver class has ACK semantics.
    pub acknowledged: Option<bool>,
    /// Signed ACK SNR sample when the hardware reports a valid one.
    pub ack_snr_db: Option<i8>,
    /// Measured on-air duration when the backend can report it.
    pub airtime_micros: Option<u32>,
}

impl MacServiceCapabilities {
    /// Whether a requested receive BA window fits the complete service.
    pub const fn supports_rx_block_ack_window(self, window: u16) -> bool {
        window != 0 && window <= self.resources.rx_block_ack_max_window
    }

    /// Whether a requested transmit BA window fits the complete service.
    pub const fn supports_tx_block_ack_window(self, window: u16) -> bool {
        window != 0 && window <= self.resources.tx_block_ack_max_window
    }
}

#[cfg(test)]
mod tests;
