//! Split-MAC offload boundaries, service capabilities and normalized statuses.

use oer_ieee80211_lower_mac::{HardwareServices, TxStatus};
use oer_ieee80211_mac::{channel::Channel, phy::PhyRate, qos::WmmAccessCategory};

use crate::interface;

/// Provenance of one normalized receive value: the lower-MAC port's
/// [`RxEvidence`](oer_ieee80211_lower_mac::RxEvidence).
pub use oer_ieee80211_lower_mac::RxEvidence as MacRxEvidence;

/// Crypto result visible for an accepted receive MPDU: the lower-MAC port's
/// [`RxCryptoStatus`](oer_ieee80211_lower_mac::RxCryptoStatus).
pub use oer_ieee80211_lower_mac::RxCryptoStatus as MacRxCryptoStatus;

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
    pub const fn supports_role(self, role: interface::VifRole) -> bool {
        match role {
            interface::VifRole::Station => self.station_interfaces != 0,
            interface::VifRole::AccessPoint => self.access_point_interfaces != 0,
        }
    }

    pub const fn supports_monitor_tap(self, point: interface::MonitorTapPoint) -> bool {
        match point {
            interface::MonitorTapPoint::Raw => self.raw_monitor_tap,
            interface::MonitorTapPoint::Normalized => self.normalized_monitor_tap,
            interface::MonitorTapPoint::ProtocolValidated => self.protocol_validated_monitor_tap,
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
        let mut index = 0;
        while index < owned.len() {
            if matches!(owned[index].0, MacOperationOwner::Hardware) {
                services = services.union(owned[index].1);
            }
            index += 1;
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

/// Portable policy for one logical ordinary-MPDU exchange.
///
/// The encoded frame and its ownership lease are deliberately not embedded in
/// this copyable value. A concrete MAC backend combines this policy with an
/// owned TX lease and translates the access category and typed PHY rate into
/// its private queue/register representation. `Rate` is the portable
/// [`PhyRate`] unless the backend's retry ladder needs its own rate type. Descriptor capacity, hardware
/// key indices, coexistence priorities and DMA metadata are therefore not
/// part of the HMAC contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPlan<Rate = PhyRate> {
    /// Standard WMM/EDCA category selected by protocol policy.
    pub access_category: WmmAccessCategory,
    /// Initial typed PHY policy selected for this logical exchange.
    pub initial_rate: Rate,
    /// Maximum hardware publications, including the first publication.
    pub publication_limit: u8,
    /// Executor watchdog applied independently to each publication.
    ///
    /// This is not the on-air MPDU lifetime and not a blocking delay.
    pub publication_timeout: oer_time::Duration,
}

/// Observable state of a MAC transmit queue at the SoftMAC/backend boundary.
///
/// `Backpressured` is a normal ownership condition: an earlier lease or
/// hardware transaction is still live and the caller must wait for progress.
/// `ResetRequired` is terminal for the current radio epoch and must never be
/// treated as ordinary queue pressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxQueueState {
    Ready,
    Backpressured,
    ResetRequired,
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
    pub channel: MacRxEvidence<Channel>,
    pub rate: MacRxEvidence<Rate>,
    pub rssi_dbm: MacRxEvidence<i8>,
    pub crypto: MacRxEvidence<MacRxCryptoStatus>,
    /// Whether the hardware marked this as an IEEE VHT/HE S-MPDU (an MPDU
    /// carried alone in an A-MPDU subframe with the delimiter EOF bit set).
    /// This is not a synonym for an ordinary non-aggregated MPDU.
    pub s_mpdu: MacRxEvidence<bool>,
    /// Whether this MPDU was carried in an A-MPDU container.
    ///
    /// This does not state how many MPDUs the container carried. In
    /// particular, an S-MPDU is the sole MPDU in a VHT/HE A-MPDU and
    /// therefore has both `s_mpdu=true` and `ampdu=true`. Provenance remains
    /// explicit because HT supplies an RXVECTOR Aggregation bit, while the
    /// VHT/HE PPDU format establishes A-MPDU containment by protocol rule.
    pub ampdu: MacRxEvidence<bool>,
    /// Whether this MPDU carries an A-MSDU payload.
    ///
    /// A-MPDU and A-MSDU are independent dimensions and deliberately retain
    /// independent evidence. Protocol parsing can prove the latter without
    /// proving the former.
    pub amsdu: MacRxEvidence<bool>,
}

impl<Rate> MacRxMetadata<Rate> {
    /// Metadata for a synthetic event or a boundary that has not observed any
    /// receive status. This is not a successful/zero-valued hardware sample.
    pub const fn unavailable() -> Self {
        Self {
            channel: MacRxEvidence::Unavailable,
            rate: MacRxEvidence::Unavailable,
            rssi_dbm: MacRxEvidence::Unavailable,
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::Unavailable,
            ampdu: MacRxEvidence::Unavailable,
            amsdu: MacRxEvidence::Unavailable,
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

/// Terminal result of one logical A-MPDU exchange.
///
/// A successful aggregate may include several aggregate publications followed
/// by individual retries of MPDUs taken out of it.  `Delivered` therefore describes
/// the complete HMAC-visible exchange, not just receipt of one BlockAck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacAmpduTxResult {
    /// Every original MPDU was acknowledged, by BlockAck or by an individual
    /// retry.
    Delivered,
    /// The retry policy ended while at least one original MPDU was unacknowledged.
    Incomplete,
    /// The aggregate publication reached its terminal hardware timeout edge.
    HardwareTimeout,
    /// The aggregate publication reached its terminal collision limit.
    CollisionLimit,
}

/// Normalized terminal status for one logical A-MPDU exchange.
///
/// The aggregate rate is kept separately from the individual retries' final
/// rate.  Collapsing those into one `final_rate` would lose which part of the
/// exchange used which PHY policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacAmpduTxStatus<Rate = PhyRate> {
    pub result: MacAmpduTxResult,
    pub original_subframes: u16,
    /// Number of A-MPDU hardware publications, including the first one.
    pub aggregate_attempts: u8,
    pub aggregate_rate: Rate,
    /// Original MPDUs acknowledged by one or more BlockAck responses.
    pub block_acknowledged_subframes: u16,
    /// Signed ACK SNR sample of the last BlockAck, when the hardware
    /// reports a valid one.
    pub ack_snr_db: Option<i8>,
    /// MPDUs taken out of the aggregate after its last publication and
    /// sent as individual frames.
    pub individual_retries: MacIndividualRetries<Rate>,
}

/// Individual transmissions of MPDUs that left an A-MPDU exchange: a single
/// missing HT MPDU, or every live missing MPDU once the BlockAck agreement
/// ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacIndividualRetries<Rate = PhyRate> {
    /// MPDUs whose individual transmission was acknowledged.
    pub transmitted: u16,
    /// MPDUs whose individual transmission ended unacknowledged.
    pub failed: u16,
    /// Hardware publications across every individual transmission.
    pub attempts: u16,
    /// Rate of the last individual transmission, if any was made.
    pub final_rate: Option<Rate>,
}

impl<Rate> MacIndividualRetries<Rate> {
    pub const NONE: Self = Self {
        transmitted: 0,
        failed: 0,
        attempts: 0,
        final_rate: None,
    };

    /// Account one finished individual transmission.
    pub fn record(&mut self, status: MacTxStatus<Rate>) {
        if matches!(status.result, MacTxResult::Transmitted) {
            self.transmitted = self.transmitted.saturating_add(1);
        } else {
            self.failed = self.failed.saturating_add(1);
        }
        self.attempts = self.attempts.saturating_add(u16::from(status.attempts));
        self.final_rate = Some(status.final_rate);
    }
}

impl<Rate: Copy> MacAmpduTxStatus<Rate> {
    /// Original MPDUs proved delivered by BlockAck or by an acknowledged
    /// individual retry.
    pub const fn delivered_subframes(&self) -> u16 {
        self.block_acknowledged_subframes
            .saturating_add(self.individual_retries.transmitted)
    }

    pub const fn fully_delivered(&self) -> bool {
        matches!(self.result, MacAmpduTxResult::Delivered)
            && self.delivered_subframes() == self.original_subframes
    }

    /// Aggregate and ordinary hardware publications made for the complete
    /// exchange.
    pub const fn total_publication_attempts(&self) -> u16 {
        (self.aggregate_attempts as u16).saturating_add(self.individual_retries.attempts)
    }
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
