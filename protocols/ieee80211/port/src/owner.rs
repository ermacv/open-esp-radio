//! The lower-MAC port on the owner shape of `oer-radio-port`
//! ([`Owner`](oer_radio_port::Owner), [`PortBody`](oer_radio_port::PortBody)):
//! its validated values and its disabled-only settings. The running phase
//! and its tokens are [`phase`](crate::phase)'s.
//!
//! Every refusal of the old lower-MAC port trait that a caller's own state
//! explains is a type here or in the phase (`docs/architecture.md`,
//! "Ownership"); what the world controls (the air, time, the hardware) stays a typed
//! outcome. A backend implements [`LowerMacBackend`].
//!
//! | Refusal of the old port trait | Form here |
//! | --- | --- |
//! | lifecycle refusals, "disabled", "port enabled" | the owner's state: the install gives the port its channel and addresses, [`LowerMacSetup`] changes the channel only while disabled, a backend whose addresses change implements [`AddressSetup`], a phase exists only while enabled |
//! | an unsupported channel | [`SupportedChannel`] |
//! | an unknown interface or role | the phase's interface of each role ([`StationPhase`](crate::StationPhase), [`AccessPointPhase`](crate::AccessPointPhase)) with only that role's operations; an attempt names no interface |
//! | the receive rules a role may not request, a BSSID the role cannot take | [`SupportedFilter`] of the role, and the station's [`StationReceive`]; an access point's BSSID is its address |
//! | busy (configuring an interface, removing a key an attempt in flight uses, closing the gate) | the operation returns at once and its end arrives as an event; never refused |
//! | an unsupported rate | [`PhaseRate`], checked against the phase's channel; a BlockAckReq takes a [`LegacyPhaseRate`]; a retry plan's stages take checked rates |
//! | an unsupported power | [`SupportedPower`] |
//! | a response the frame or rate cannot carry | the buffer's [`TxFrame`] kind: an [`IndividualAddress`] or [`GroupAddress`] receiver, and for no ACK a [`NoAckHeader`] at a [`SupportedNoAck`] rate; the backend writes the QoS Ack Policy from the kind ([`QosControl`]) |
//! | a busy queue, no free buffer | a transmit ticket and buffers of a class ([`TxBufferClass`](crate::TxBufferClass)), tokens the caller holds; the backend keeps an attempt its queue cannot take yet |
//! | a duplicate attempt identifier | the ticket, which comes back with the terminal |
//! | an invalid length | [`MpduLength`] and [`ClassFrame`](crate::ClassFrame) |
//! | no key slot, an unknown key | a key slot ([`Slot`](oer_radio_port::Slot) of the interface's own kind) and the installed key token an attempt names |
//! | an invalid key | [`Ccmp128Key`](oer_ieee80211_wire::ccmp::Ccmp128Key), [`GroupKeyId`](oer_ieee80211_wire::ccmp::GroupKeyId), an [`IndividualAddress`] peer |
//! | no Block Ack slot, an invalid agreement | a Block Ack slot, [`SupportedBlockAck`] and the agreement token that holds the slot |
//! | an unsupported EDCA record or BSS color | [`SupportedEdca`], [`BssColor`] |
//!
//! These stay the world's outcomes: [`Poisoned`](oer_radio_port::Poisoned),
//! [`EventsLost`](oer_radio_port::EventsLost) (only unsolicited reception),
//! a clock that could not be read, a TSF that failed its timing check, and
//! hardware that rejects a key or misreads a programmed agreement (a
//! fault).
//!
//! # Capabilities
//!
//! What the hardware supports is the same for every instance of a backend
//! type, so it is an associated constant of a capabilities marker
//! ([`Ieee80211Capabilities`]); a validated value is branded by that
//! marker, minted only by its checks, and a portable core is generic over
//! the marker alone. A channel is first permitted by a regulatory domain, then
//! checked against the marker:
//!
//! ```
//! # use oer_ieee80211_port::{Channel, ChannelWidth, Ieee80211Capabilities, SupportedChannel};
//! # use oer_radio_regulatory::{RegulatoryDomain, Refused};
//! fn tune<K: Ieee80211Capabilities>(
//!     domain: &RegulatoryDomain,
//!     channel: Channel,
//! ) -> Option<SupportedChannel<K>> {
//!     SupportedChannel::new(domain.permit(channel).ok()?)
//! }
//! ```
//!
//! # Compile-fail tests
//!
//! A backend that tunes to 2.4 GHz only does not tune to a 5 GHz channel:
//!
//! ```
//! # use oer_ieee80211_port::*;
//! # use oer_ieee80211_wire::phy::{HeMcs, HtMcs};
//! # struct TwoFourOnly;
//! # impl Ieee80211Capabilities for TwoFourOnly {
//! #     type SppScope = oer_ieee80211_wire::security::spp::Association;
//! #     const CAPS: LowerMacCapabilities = LowerMacCapabilities {
//! #         bands: BandSet::GHZ2_4,
//! #         widths: WidthSet::MHZ20,
//! #         rates: RateSupport {
//! #             dsss_cck: true,
//! #             ofdm: true,
//! #             ht_max_mcs: HtMcs::new(7),
//! #             he_max_mcs: HeMcs::new(9),
//! #             he_max_bandwidth_mhz: 20,
//! #             he_dcm: false,
//! #             he_ldpc: false,
//! #             spatial_streams: 1,
//! #         },
//! #         services: HardwareServices::FCS,
//! #         vifs: 2,
//! #         tx_queues: 4,
//! #         max_mpdu_length: 2304,
//! #         tx_power_ceiling_min_dbm: Some(0),
//! #         individual_no_ack: PhyFormatSet::NON_HT,
//! #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         rx_block_ack_max_tid: 7,
//! #         rx_block_ack_max_window: 64,
//! #         edca_max_aifsn: 15,
//! #         edca_max_cw_exponent: 10,
//! #         edca_max_txop_limit_units_32_us: u16::MAX,
//! #     };
//! # }
//! let permitted = oer_radio_regulatory::TEST_DOMAIN
//!     .permit(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap())
//!     .unwrap();
//! assert!(SupportedChannel::<TwoFourOnly>::new(permitted).is_none());
//! ```
//!
//! Its compiling twin names a channel the capabilities state:
//!
//! ```
//! # use oer_ieee80211_port::*;
//! # use oer_ieee80211_wire::phy::{HeMcs, HtMcs};
//! # struct TwoFourOnly;
//! # impl Ieee80211Capabilities for TwoFourOnly {
//! #     type SppScope = oer_ieee80211_wire::security::spp::Association;
//! #     const CAPS: LowerMacCapabilities = LowerMacCapabilities {
//! #         bands: BandSet::GHZ2_4,
//! #         widths: WidthSet::MHZ20,
//! #         rates: RateSupport {
//! #             dsss_cck: true,
//! #             ofdm: true,
//! #             ht_max_mcs: HtMcs::new(7),
//! #             he_max_mcs: HeMcs::new(9),
//! #             he_max_bandwidth_mhz: 20,
//! #             he_dcm: false,
//! #             he_ldpc: false,
//! #             spatial_streams: 1,
//! #         },
//! #         services: HardwareServices::FCS,
//! #         vifs: 2,
//! #         tx_queues: 4,
//! #         max_mpdu_length: 2304,
//! #         tx_power_ceiling_min_dbm: Some(0),
//! #         individual_no_ack: PhyFormatSet::NON_HT,
//! #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
//! #         rx_block_ack_max_tid: 7,
//! #         rx_block_ack_max_window: 64,
//! #         edca_max_aifsn: 15,
//! #         edca_max_cw_exponent: 10,
//! #         edca_max_txop_limit_units_32_us: u16::MAX,
//! #     };
//! # }
//! let permitted = oer_radio_regulatory::TEST_DOMAIN
//!     .permit(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap())
//!     .unwrap();
//! assert!(SupportedChannel::<TwoFourOnly>::new(permitted).is_some());
//! ```
//!
//! No caller or backend mints a rate past the phase's check:
//!
//! ```compile_fail,E0624
//! # use oer_ieee80211_port::{PhaseRate, PhyRate};
//! # use oer_ieee80211_wire::phy::LegacyRate;
//! let _ = PhaseRate::checked(PhyRate::Legacy(LegacyRate::Ofdm6M));
//! ```
//!
//! ```
//! # use oer_ieee80211_port::{LowerMacPhase, PhaseRate, PhyRate};
//! # use oer_ieee80211_wire::phy::LegacyRate;
//! fn rate<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &P) -> Option<PhaseRate<'p, 'id, P::Caps>> {
//!     phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_port::BssColor;
//! let _ = BssColor { color: 64 };
//! ```
//!
//! ```
//! # use oer_ieee80211_port::BssColor;
//! assert!(BssColor::new(64).is_none());
//! assert!(BssColor::new(63).is_some());
//! ```

use core::marker::PhantomData;
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::frame::FrameControl;

use crate::EdcaParameterSet;
use oer_ieee80211_wire::sequence::SequenceNumber;
use oer_radio_port::PortBackend;
use oer_radio_regulatory::Permitted;

use crate::{Channel, LowerMacCapabilities, PhyRate, ReceiveFilter, TxPower, VifRole};

/// What a lower-MAC backend type supports, the same for every instance: the
/// brand of its validated values.
pub trait Ieee80211Capabilities {
    /// Smallest domain whose association SPP mode can be programmed independently.
    type SppScope: oer_ieee80211_wire::security::spp::SppScope;
    /// The backend's capabilities.
    const CAPS: LowerMacCapabilities;
}

/// A channel the regulatory domain permits and the capabilities `K` tune to.
pub struct SupportedChannel<K> {
    permitted: Permitted<Channel>,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedChannel<K> {
    /// `permitted`, when `K` tunes to its channel.
    pub const fn new(permitted: Permitted<Channel>) -> Option<Self> {
        if K::CAPS.supports_channel(permitted.channel()) {
            Some(Self {
                permitted,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The channel.
    pub const fn channel(&self) -> Channel {
        self.permitted.channel()
    }

    /// The channel with what the domain allows on it.
    pub const fn permitted(&self) -> Permitted<Channel> {
        self.permitted
    }
}

impl<K> Clone for SupportedChannel<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for SupportedChannel<K> {}

impl<K> PartialEq for SupportedChannel<K> {
    fn eq(&self, other: &Self) -> bool {
        self.permitted == other.permitted
    }
}

impl<K> Eq for SupportedChannel<K> {}

impl<K> core::fmt::Debug for SupportedChannel<K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("SupportedChannel")
            .field(&self.permitted)
            .finish()
    }
}

/// An interface role, as a type.
pub trait InterfaceRole: 'static {
    /// The role.
    const ROLE: VifRole;
}

/// The station role.
pub enum Station {}

/// The access-point role.
pub enum AccessPoint {}

impl InterfaceRole for Station {
    const ROLE: VifRole = VifRole::Station;
}

impl InterfaceRole for AccessPoint {
    const ROLE: VifRole = VifRole::AccessPoint;
}

/// Receive rules an interface of role `R` may request of `K`.
pub struct SupportedFilter<K, R> {
    filter: ReceiveFilter,
    brand: PhantomData<fn() -> (K, R)>,
}

impl<K: Ieee80211Capabilities, R: InterfaceRole> SupportedFilter<K, R> {
    /// `filter`, when `K` admits every rule of it for role `R`.
    pub const fn new(filter: ReceiveFilter) -> Option<Self> {
        if K::CAPS.receive_filters(R::ROLE).contains(filter) {
            Some(Self {
                filter,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The rules.
    pub const fn get(&self) -> ReceiveFilter {
        self.filter
    }
}

impl<K, R> Clone for SupportedFilter<K, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, R> Copy for SupportedFilter<K, R> {}

impl<K, R> core::fmt::Debug for SupportedFilter<K, R> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("SupportedFilter")
            .field(&self.filter)
            .finish()
    }
}

/// What a station receives: nothing, other BSSs' management only (outside
/// a BSS), or a BSS with its rules.
pub enum StationReceive<K> {
    /// Deliver no station frames.
    Off,
    /// Deliver beacons and Probe Responses from every BSS for scanning.
    OtherBssManagement,
    /// Apply the station's receive rules within the named BSS.
    Bss {
        /// The BSS whose group traffic and beacons the station recognizes.
        bssid: IndividualAddress,
        /// Receive rules supported for this station interface.
        filter: SupportedFilter<K, Station>,
    },
}

impl<K> Clone for StationReceive<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for StationReceive<K> {}

/// A transmit power `K` applies.
#[derive(Debug)]
pub struct SupportedPower<K> {
    power: TxPower,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedPower<K> {
    /// `power`, when `K` applies it: its calibrated power, or a ceiling no
    /// lower than its lowest.
    pub const fn new(power: TxPower) -> Option<Self> {
        let supported = match (power, K::CAPS.tx_power_ceiling_min_dbm) {
            (TxPower::Calibrated, _) => true,
            (TxPower::MaxDbm(dbm), Some(lowest)) => dbm >= lowest,
            (TxPower::MaxDbm(_), None) => false,
        };
        if supported {
            Some(Self {
                power,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The power.
    pub const fn get(&self) -> TxPower {
        self.power
    }
}

/// The length of one MPDU without FCS and MIC: at least a header, at most
/// what `K`'s buffers hold.
#[derive(Debug)]
pub struct MpduLength<K> {
    len: u16,
    brand: PhantomData<fn() -> K>,
}

impl<K> Copy for MpduLength<K> {}

impl<K> Clone for MpduLength<K> {
    fn clone(&self) -> Self {
        *self
    }
}

/// The shortest MPDU: an ACK frame's header.
const MIN_MPDU: u16 = 10;

impl<K: Ieee80211Capabilities> MpduLength<K> {
    /// `len`, when it is an MPDU's length `K` buffers.
    pub const fn new(len: usize) -> Option<Self> {
        if len >= MIN_MPDU as usize && len <= K::CAPS.max_mpdu_length as usize {
            Some(Self {
                len: len as u16,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The length.
    pub const fn get(&self) -> usize {
        self.len as usize
    }
}

/// A receive Block Ack agreement within `K`'s limits.
#[derive(Debug)]
pub struct SupportedBlockAck<K> {
    peer: IndividualAddress,
    tid: Tid,
    start_sequence: SequenceNumber,
    window: u16,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee80211Capabilities> SupportedBlockAck<K> {
    /// An agreement with `peer` on `tid`, when `K` acknowledges its TID and
    /// window.
    pub const fn new(
        peer: IndividualAddress,
        tid: Tid,
        start_sequence: SequenceNumber,
        window: u16,
    ) -> Option<Self> {
        if tid.get() <= K::CAPS.rx_block_ack_max_tid
            && window >= 1
            && window <= K::CAPS.rx_block_ack_max_window
        {
            Some(Self {
                peer,
                tid,
                start_sequence,
                window,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The originator, the TID, the starting sequence and the window.
    pub const fn get(&self) -> (IndividualAddress, Tid, SequenceNumber, u16) {
        (self.peer, self.tid, self.start_sequence, self.window)
    }
}

/// A VIF's EDCA table whose AIFSN, contention exponents and TXOP limits
/// fit `K`, with `ECWmin <= ECWmax` and each MSDU lifetime in 0..=500 TU.
/// Validation refuses the complete table without changing any record.
pub struct SupportedEdca<K> {
    parameters: EdcaParameterSet,
    brand: PhantomData<fn() -> K>,
}

impl<K> core::fmt::Debug for SupportedEdca<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("SupportedEdca")
            .field(&self.parameters)
            .finish()
    }
}

impl<K> Clone for SupportedEdca<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for SupportedEdca<K> {}

impl<K> PartialEq for SupportedEdca<K> {
    fn eq(&self, other: &Self) -> bool {
        self.parameters == other.parameters
    }
}

impl<K> Eq for SupportedEdca<K> {}

impl<K: Ieee80211Capabilities> SupportedEdca<K> {
    /// `parameters`, when `K` applies every record of it.
    pub const fn new(parameters: EdcaParameterSet) -> Option<Self> {
        use oer_ieee80211_wire::qos::WmmAccessCategory;
        const fn valid<K: Ieee80211Capabilities>(
            parameters: EdcaParameterSet,
            category: WmmAccessCategory,
        ) -> bool {
            let record = parameters.access_category(category);
            record.wmm.aifsn <= K::CAPS.edca_max_aifsn
                && record.wmm.aifsn <= 15
                && record.wmm.ecw_max <= K::CAPS.edca_max_cw_exponent
                && record.wmm.ecw_max <= crate::edca::MAX_CW_EXPONENT
                && record.wmm.txop_limit_units_32_us <= K::CAPS.edca_max_txop_limit_units_32_us
                && record.msdu_lifetime_tu <= EdcaParameterSet::DEFAULT_MSDU_LIFETIME_TU
                && record.wmm.ecw_min <= record.wmm.ecw_max
        }
        if !(valid::<K>(parameters, WmmAccessCategory::BestEffort)
            && valid::<K>(parameters, WmmAccessCategory::Background)
            && valid::<K>(parameters, WmmAccessCategory::Video)
            && valid::<K>(parameters, WmmAccessCategory::Voice))
        {
            return None;
        }
        Some(Self {
            parameters,
            brand: PhantomData,
        })
    }

    /// The parameter set.
    pub const fn get(&self) -> EdcaParameterSet {
        self.parameters
    }
}

/// The color of an HE BSS, 0-63.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssColor {
    color: u8,
}

impl BssColor {
    /// `color`, when it is a BSS color.
    pub const fn new(color: u8) -> Option<Self> {
        if color <= 63 {
            Some(Self { color })
        } else {
            None
        }
    }

    /// The color.
    pub const fn get(self) -> u8 {
        self.color
    }
}

/// A rate the phase's channel carries: [`LowerMacPhase::rate`](crate::LowerMacPhase::rate) checks it
/// against the capabilities and the channel the phase runs on.
///
/// The invariant installation brand, capabilities and phase loan remain in
/// every derived rate, retry plan and frame. A second installation with the
/// same capabilities cannot consume it; keeping it also keeps the enabled
/// owner borrowed, so disable and retune cannot invalidate its channel check.
pub struct PhaseRate<'p, 'id, K> {
    rate: PhyRate,
    loan: PhantomData<fn(&'p ()) -> &'p ()>,
    install: PhantomData<fn(&'id ()) -> &'id ()>,
    capabilities: PhantomData<fn(K) -> K>,
}

impl<K> Copy for PhaseRate<'_, '_, K> {}
impl<K> Clone for PhaseRate<'_, '_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> core::fmt::Debug for PhaseRate<'_, '_, K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("PhaseRate").field(&self.rate).finish()
    }
}

impl<'p, 'id, K> PhaseRate<'p, 'id, K> {
    /// A rate [`LowerMacPhase::rate`](crate::LowerMacPhase::rate) checked; nothing else mints one.
    pub(crate) const fn checked(rate: PhyRate) -> Self {
        Self {
            rate,
            loan: PhantomData,
            install: PhantomData,
            capabilities: PhantomData,
        }
    }

    /// The rate.
    pub const fn get(&self) -> PhyRate {
        self.rate
    }

    /// The rate, when it is non-HT, as a BlockAckReq needs.
    ///
    /// # Errors
    ///
    /// The rate, when it is not.
    pub fn legacy(self) -> Result<LegacyPhaseRate<'p, 'id, K>, Self> {
        if matches!(self.rate, PhyRate::Legacy(_)) {
            Ok(LegacyPhaseRate(self))
        } else {
            Err(self)
        }
    }
}

/// The receiver of a group frame: its address has the group bit set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupAddress([u8; 6]);

impl GroupAddress {
    /// `address`, when its group bit is set.
    pub const fn new(address: [u8; 6]) -> Option<Self> {
        if address[0] & 1 == 1 {
            Some(Self(address))
        } else {
            None
        }
    }

    /// The address.
    pub const fn get(self) -> [u8; 6] {
        self.0
    }
}

/// A traffic identifier, 0 to 15.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tid(u8);

impl Tid {
    /// `tid`, when it is at most 15.
    pub const fn new(tid: u8) -> Option<Self> {
        if tid <= 15 { Some(Self(tid)) } else { None }
    }

    /// The identifier.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The on-wire acknowledgement policy chosen by a checked response header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QosAckPolicy {
    /// The addressed receiver acknowledges this frame.
    Normal,
    /// No acknowledgement is solicited.
    NoAck,
}

/// The QoS Control field of a QoS data frame, but its Ack Policy: the
/// backend writes that from the frame's response
/// (its [`TxFrame`] kind), the one source of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QosControl {
    /// The traffic identifier of this QoS data frame.
    pub tid: Tid,
    /// Whether this frame ends the current service period.
    pub end_of_service_period: bool,
    /// Whether the body carries an A-MSDU.
    pub amsdu_present: bool,
    /// The field's high octet (TXOP limit, queue size or mesh control).
    pub high: u8,
}

impl QosControl {
    /// The field, with the policy derived from its checked response header.
    pub const fn encode(self, policy: QosAckPolicy) -> u16 {
        self.tid.get() as u16
            | (self.end_of_service_period as u16) << 4
            | match policy {
                QosAckPolicy::Normal => 0,
                QosAckPolicy::NoAck => 1 << 5,
            }
            | (self.amsdu_present as u16) << 7
            | (self.high as u16) << 8
    }
}

/// The MAC header of a data or management frame, addressed to `A`
/// ([`IndividualAddress`] or [`GroupAddress`]). Its fields agree with its
/// Frame Control: a fourth address exactly when To DS and From DS are both
/// set, a QoS Control exactly in a QoS data frame, an HT Control exactly
/// when the Order bit is set in a QoS data or management frame.
///
/// Only preparation derives Protected from the operation's protection:
///
/// ```compile_fail,E0624
/// use oer_ieee80211_port::HeaderSpec;
/// use oer_ieee80211_wire::address::IndividualAddress;
/// fn flag(header: HeaderSpec<IndividualAddress>) {
///     let _header = header.with_protection(false);
/// }
/// ```
///
/// A caller passes its checked response header to the encoder. Preparation
/// supplies Protected from `TxProtection` when constructing a fresh MPDU.
///
/// ```
/// use oer_ieee80211_port::{AckHeader, HeaderLayout, LayoutError};
/// fn encode(header: AckHeader, out: &mut [u8]) -> Result<(), LayoutError> {
///     HeaderLayout::new(header, out)?.encode();
///     Ok(())
/// }
/// ```
///
/// A checked MAC header always contains its mandatory fields:
///
/// ```compile_fail,E0599
/// use oer_ieee80211_port::{HeaderSpec, ReceiverAddress};
/// fn empty<A: ReceiverAddress>(header: HeaderSpec<A>) { let _empty = header.is_empty(); }
/// ```
///
/// ```
/// use oer_ieee80211_port::{HeaderSpec, ReceiverAddress};
/// fn length<A: ReceiverAddress>(header: HeaderSpec<A>) -> usize { header.header_len() }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderSpec<A> {
    frame_control: FrameControl,
    duration: u16,
    receiver: A,
    transmitter: [u8; 6],
    address3: [u8; 6],
    sequence_control: u16,
    address4: Option<[u8; 6]>,
    qos: Option<QosControl>,
    ht_control: Option<[u8; 4]>,
}

/// A receiver address of a header: [`IndividualAddress`] or
/// [`GroupAddress`].
pub trait ReceiverAddress: Copy {
    /// The address.
    fn address(self) -> [u8; 6];
}

impl ReceiverAddress for IndividualAddress {
    fn address(self) -> [u8; 6] {
        self.get()
    }
}

impl ReceiverAddress for GroupAddress {
    fn address(self) -> [u8; 6] {
        self.0
    }
}

impl<A: ReceiverAddress> HeaderSpec<A> {
    /// QoS fields, when this is a QoS data header.
    pub const fn qos(&self) -> Option<QosControl> {
        self.qos
    }
    /// Receiver of this checked header.
    pub const fn receiver(&self) -> A {
        self.receiver
    }
    /// Derive the Protected bit from the preparation's sole protection value.
    pub(crate) fn with_protection(mut self, protection: crate::MpduProtection) -> Self {
        self.frame_control = self
            .frame_control
            .with_protected(matches!(protection, crate::MpduProtection::Ccmp));
        self
    }
    /// Encode a fresh aggregate subframe with its builder's receiver and TID.
    /// `ClassSubframe` retains the proven QoS value through this write.
    pub(crate) fn in_aggregate(mut self, receiver: A, qos: QosControl) -> Self {
        self.receiver = receiver;
        self.qos = Some(qos);
        self
    }
    /// The header, when its fields agree with its Frame Control.
    #[allow(clippy::too_many_arguments, reason = "the fields of a MAC header")]
    pub fn new(
        frame_control: FrameControl,
        duration: u16,
        receiver: A,
        transmitter: [u8; 6],
        address3: [u8; 6],
        sequence_control: u16,
        address4: Option<[u8; 6]>,
        qos: Option<QosControl>,
        ht_control: Option<[u8; 4]>,
    ) -> Option<Self> {
        (address4.is_some() == frame_control.is_four_address()
            && qos.is_some() == frame_control.is_qos_data()
            && ht_control.is_some() == frame_control.has_ht_control())
        .then_some(Self {
            frame_control,
            duration,
            receiver,
            transmitter,
            address3,
            sequence_control,
            address4,
            qos,
            ht_control,
        })
    }

    /// Decode a data or management header, checking its receiver's kind.
    pub fn decode(frame: &[u8], receiver: impl FnOnce([u8; 6]) -> Option<A>) -> Option<Self> {
        let header = oer_ieee80211_wire::frame::DecodedHeader::decode(frame)?;
        let qos = header.qos().map(|field| QosControl {
            tid: Tid((field & 15) as u8),
            end_of_service_period: field & 0x10 != 0,
            amsdu_present: field & 0x80 != 0,
            high: (field >> 8) as u8,
        });
        Self::new(
            header.control(),
            header.duration(),
            receiver(header.receiver())?,
            header.transmitter(),
            header.address3(),
            header.sequence_control(),
            header.address4(),
            qos,
            header.ht_control(),
        )
    }

    /// The header's length in octets.
    pub fn header_len(&self) -> usize {
        match (
            self.address4.is_some(),
            self.qos.is_some(),
            self.ht_control.is_some(),
        ) {
            (false, false, false) => 24,
            (false, true, false) => 26,
            (false, false, true) => 28,
            (false, true, true) | (true, false, false) => 30,
            (true, true, false) => 32,
            (true, false, true) => 34,
            (true, true, true) => 36,
        }
    }

    pub(crate) fn encoder(self, policy: QosAckPolicy) -> HeaderLength<A> {
        let [f0, f1] = self.frame_control.get().to_le_bytes();
        let [d0, d1] = self.duration.to_le_bytes();
        let [r0, r1, r2, r3, r4, r5] = self.receiver.address();
        let [t0, t1, t2, t3, t4, t5] = self.transmitter;
        let [a0, a1, a2, a3, a4, a5] = self.address3;
        let [s0, s1] = self.sequence_control.to_le_bytes();
        // Every arm constructs its exact wire array. No caller can select N
        // independently of these fields, and encoding copies the entire array.
        match (
            self.address4,
            self.qos.map(|qos| qos.encode(policy).to_le_bytes()),
            self.ht_control,
        ) {
            (None, None, None) => HeaderLength::L24(HeaderEncoder::new([
                f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3, a4,
                a5, s0, s1,
            ])),
            (None, Some([q0, q1]), None) => HeaderLength::L26(HeaderEncoder::new([
                f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3, a4,
                a5, s0, s1, q0, q1,
            ])),
            (None, None, Some([h0, h1, h2, h3])) => HeaderLength::L28(HeaderEncoder::new([
                f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3, a4,
                a5, s0, s1, h0, h1, h2, h3,
            ])),
            (None, Some([q0, q1]), Some([h0, h1, h2, h3])) => {
                HeaderLength::L30(HeaderEncoder::new([
                    f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3,
                    a4, a5, s0, s1, q0, q1, h0, h1, h2, h3,
                ]))
            }
            (Some([b0, b1, b2, b3, b4, b5]), None, None) => {
                HeaderLength::L30(HeaderEncoder::new([
                    f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3,
                    a4, a5, s0, s1, b0, b1, b2, b3, b4, b5,
                ]))
            }
            (Some([b0, b1, b2, b3, b4, b5]), Some([q0, q1]), None) => {
                HeaderLength::L32(HeaderEncoder::new([
                    f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3,
                    a4, a5, s0, s1, b0, b1, b2, b3, b4, b5, q0, q1,
                ]))
            }
            (Some([b0, b1, b2, b3, b4, b5]), None, Some([h0, h1, h2, h3])) => {
                HeaderLength::L34(HeaderEncoder::new([
                    f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3,
                    a4, a5, s0, s1, b0, b1, b2, b3, b4, b5, h0, h1, h2, h3,
                ]))
            }
            (Some([b0, b1, b2, b3, b4, b5]), Some([q0, q1]), Some([h0, h1, h2, h3])) => {
                HeaderLength::L36(HeaderEncoder::new([
                    f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, a0, a1, a2, a3,
                    a4, a5, s0, s1, b0, b1, b2, b3, b4, b5, q0, q1, h0, h1, h2, h3,
                ]))
            }
        }
    }
}

/// An encoder for one header shape, selected by the portable layout
/// constructor. Its private construction proves that its fields fill N bytes.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_wire::address::IndividualAddress;
/// use oer_ieee80211_port::{owner::HeaderEncoder};
/// fn short(encoder: HeaderEncoder<IndividualAddress, 24>) {
///     encoder.encode(&mut [0; 23]);
/// }
/// ```
///
/// ```
/// use oer_ieee80211_wire::address::IndividualAddress;
/// use oer_ieee80211_port::{owner::HeaderEncoder};
/// fn sized(encoder: HeaderEncoder<IndividualAddress, 24>) {
///     encoder.encode(&mut [0; 24]);
/// }
/// ```
pub struct HeaderEncoder<A, const N: usize> {
    bytes: [u8; N],
    receiver: PhantomData<A>,
}
impl<A: ReceiverAddress, const N: usize> HeaderEncoder<A, N> {
    fn new(bytes: [u8; N]) -> Self {
        Self {
            bytes,
            receiver: PhantomData,
        }
    }

    /// Write this header into exactly its storage. This operation cannot fail.
    pub fn encode(self, out: &mut [u8; N]) {
        *out = self.bytes;
    }
}

pub(crate) enum HeaderLength<A> {
    L24(HeaderEncoder<A, 24>),
    L26(HeaderEncoder<A, 26>),
    L28(HeaderEncoder<A, 28>),
    L30(HeaderEncoder<A, 30>),
    L32(HeaderEncoder<A, 32>),
    L34(HeaderEncoder<A, 34>),
    L36(HeaderEncoder<A, 36>),
}

/// A rate of the phase that carries an individual frame without an ACK,
/// as the capabilities' individual no-ACK formats state:
/// [`LowerMacPhase::no_ack`](crate::LowerMacPhase::no_ack) checks it.
#[derive(Debug)]
pub struct SupportedNoAck<'p, 'id, K>(pub(crate) PhaseRate<'p, 'id, K>);

impl<'p, 'id, K> SupportedNoAck<'p, 'id, K> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p, 'id, K> {
        &self.0
    }
}

/// A non-HT rate of the phase, the only rates a BlockAckReq goes at:
/// [`PhaseRate::legacy`] checks it.
#[derive(Debug)]
pub struct LegacyPhaseRate<'p, 'id, K>(PhaseRate<'p, 'id, K>);

impl<'p, 'id, K> LegacyPhaseRate<'p, 'id, K> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p, 'id, K> {
        &self.0
    }
}

/// The header of an individual frame that solicits an ACK: any data or
/// management frame but Action No Ack, which its receiver never
/// acknowledges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckHeader(HeaderSpec<IndividualAddress>);

impl AckHeader {
    /// `header`, when it is not an Action No Ack frame.
    pub fn new(header: HeaderSpec<IndividualAddress>) -> Option<Self> {
        (!header.frame_control.is_action_no_ack()).then_some(Self(header))
    }

    /// The header.
    pub const fn header(&self) -> &HeaderSpec<IndividualAddress> {
        &self.0
    }

    pub(crate) fn with_protection(self, protection: crate::MpduProtection) -> Self {
        Self(self.0.with_protection(protection))
    }
}

/// The header of an individual frame that solicits no ACK: one that says
/// so to its receiver, a QoS data frame (whose Ack Policy the backend
/// writes as No Ack) or an Action No Ack frame. Any other individual frame
/// is acknowledged by its receiver whatever its sender expects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoAckHeader(HeaderSpec<IndividualAddress>);

impl NoAckHeader {
    /// `header`, when it is a QoS data or an Action No Ack frame.
    pub fn new(header: HeaderSpec<IndividualAddress>) -> Option<Self> {
        let frame_control = header.frame_control;
        (frame_control.is_qos_data() || frame_control.is_action_no_ack()).then_some(Self(header))
    }

    /// The header.
    pub const fn header(&self) -> &HeaderSpec<IndividualAddress> {
        &self.0
    }

    pub(crate) fn with_protection(self, protection: crate::MpduProtection) -> Self {
        Self(self.0.with_protection(protection))
    }
}

mod response_header {
    pub trait Sealed {}
}

/// A header whose response policy is fixed by its type. Only `AckHeader`,
/// `NoAckHeader` and a group-addressed `HeaderSpec` implement this sealed
/// contract; a raw individually addressed header has no response proof.
pub trait ResponseHeader: response_header::Sealed {
    /// Its receiver's checked address type.
    type Address: ReceiverAddress;
    /// QoS Ack Policy selected by this header kind.
    const ACK_POLICY: QosAckPolicy;
    /// Consume the response proof into its fields for encoding.
    fn into_header(self) -> HeaderSpec<Self::Address>;
}

impl response_header::Sealed for AckHeader {}
impl ResponseHeader for AckHeader {
    type Address = IndividualAddress;
    const ACK_POLICY: QosAckPolicy = QosAckPolicy::Normal;
    fn into_header(self) -> HeaderSpec<Self::Address> {
        self.0
    }
}
impl response_header::Sealed for NoAckHeader {}
impl ResponseHeader for NoAckHeader {
    type Address = IndividualAddress;
    const ACK_POLICY: QosAckPolicy = QosAckPolicy::NoAck;
    fn into_header(self) -> HeaderSpec<Self::Address> {
        self.0
    }
}
impl response_header::Sealed for HeaderSpec<GroupAddress> {}
impl ResponseHeader for HeaderSpec<GroupAddress> {
    type Address = GroupAddress;
    const ACK_POLICY: QosAckPolicy = QosAckPolicy::NoAck;
    fn into_header(self) -> HeaderSpec<Self::Address> {
        self
    }
}

/// What a buffer request describes: the frame's kind, its header and its
/// rate, from which its response follows. Only the kinds the hardware
/// sends exist, so a frame and its response never disagree.
#[derive(Debug)]
pub enum TxFrame<'p, 'id, K> {
    /// An individual data or management frame, which solicits an ACK.
    Individual {
        /// Checked acknowledged header and complete MPDU length.
        frame: SizedFrame<AckHeader, K>,
        /// The phase-bound rates and retry budget of this attempt.
        plan: crate::RetryPlan<PhaseRate<'p, 'id, K>>,
    },
    /// An individual frame that solicits no ACK and says so, at a rate that
    /// carries one without it.
    IndividualNoAck {
        /// Checked unacknowledged header and complete MPDU length.
        frame: SizedFrame<NoAckHeader, K>,
        /// A phase-bound rate that supports individual No Ack frames.
        rate: SupportedNoAck<'p, 'id, K>,
    },
    /// A group data or management frame, which solicits no response.
    Group {
        /// Checked group receiver, header and complete MPDU length.
        frame: SizedFrame<HeaderSpec<GroupAddress>, K>,
        /// The phase-bound rate of this group transmission.
        rate: PhaseRate<'p, 'id, K>,
    },
    /// A BlockAckReq, which solicits a BlockAck, at a non-HT rate.
    BlockAckRequest {
        /// The peer asked to return its Block Ack state.
        receiver: IndividualAddress,
        /// The address of the requesting transmitter.
        transmitter: [u8; 6],
        /// The traffic identifier of the Block Ack agreement.
        tid: Tid,
        /// The first sequence number requested in the Block Ack window.
        starting_sequence: SequenceNumber,
        /// The non-HT rates and retry budget of the request.
        plan: crate::RetryPlan<LegacyPhaseRate<'p, 'id, K>>,
    },
}

impl<K> TxFrame<'_, '_, K> {
    /// The MPDU's length without its FCS: the length its buffer holds.
    pub const fn mpdu_len(&self) -> usize {
        match self {
            Self::Individual { frame, .. } => frame.len.len as usize,
            Self::IndividualNoAck { frame, .. } => frame.len.len as usize,
            Self::Group { frame, .. } => frame.len.len as usize,
            Self::BlockAckRequest { .. } => oer_ieee80211_wire::block_ack::BLOCK_ACK_REQUEST_LEN,
        }
    }
}

/// A header that knows its length: [`HeaderSpec`], [`AckHeader`] and
/// [`NoAckHeader`].
pub trait FrameHeader {
    /// The header's length in octets.
    fn header_len(&self) -> usize;
}

impl<A: ReceiverAddress> FrameHeader for HeaderSpec<A> {
    fn header_len(&self) -> usize {
        HeaderSpec::header_len(self)
    }
}

impl FrameHeader for AckHeader {
    fn header_len(&self) -> usize {
        self.0.header_len()
    }
}

impl FrameHeader for NoAckHeader {
    fn header_len(&self) -> usize {
        self.0.header_len()
    }
}

/// A frame's header with the length of its whole MPDU, which holds the
/// header: the buffer a request lends is exactly that long.
#[derive(Debug)]
pub struct SizedFrame<H, K> {
    header: H,
    len: MpduLength<K>,
}

// A checked description owns no buffer or admission token. Copying it
// preserves the length/header proof without requiring a capability marker
// (or the operation context that holds it) to implement Clone.
impl<H: Copy, K> Copy for SizedFrame<H, K> {}

impl<H: Copy, K> Clone for SizedFrame<H, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H: FrameHeader, K: Ieee80211Capabilities> SizedFrame<H, K> {
    /// `header` in an MPDU of `len` octets, when they hold the header.
    pub fn new(header: H, len: MpduLength<K>) -> Option<Self> {
        (len.get() >= header.header_len()).then_some(Self { header, len })
    }

    /// The header.
    pub const fn header(&self) -> &H {
        &self.header
    }

    /// The MPDU's length.
    pub const fn len(&self) -> &MpduLength<K> {
        &self.len
    }
}

/// A lower-MAC backend on the owner shape. Its disabled-only settings are a
/// [`LowerMacSetup`] and its running phase a
/// [`LowerMacPhase`](crate::LowerMacPhase); the body's resources give its
/// transmit tickets, buffers, key and Block Ack slots and its gate once.
pub trait LowerMacBackend: PortBackend {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;

    /// The depth of the driver's software queue of each access category, a
    /// composition parameter. A service keeps at most this many data attempts
    /// of a category at the port, across all its data sources, so a station
    /// frame submitted at once waits for at most this depth plus the one on
    /// the air.
    const TX_QUEUE_DEPTH: core::num::NonZeroU8;

    /// Longest current publication: PPDU plus response wait or hardware
    /// timeout. Gate/key/disable barriers never wait for the remaining plan.
    /// Composition passes this constant to the channel arbiter.
    const PUBLICATION_BOUND: oer_time::Duration;

    /// Maximum admission-to-terminal time of a GTK mutation, including
    /// queued publications and the finite DMA/ISR/software RX cut. The driver
    /// owns a completion wake and a deadline wake. Expiry poisons with all
    /// resources retained; it never returns a usable mutation permission.
    /// Disable must finish or poison within this same bound under continuous RX.
    const GROUP_COMMAND_BOUND: oer_time::Duration;
}

/// The disabled-only settings of a lower-MAC port.
pub trait LowerMacSetup {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;

    /// Tune to `channel`, which every interface shares, in place of the
    /// channel the install or an earlier call set: a port always has one,
    /// so no phase runs untuned. The next `Enable` retunes. Nothing refuses
    /// it: the port is disabled and the channel is supported.
    fn set_channel(&mut self, channel: SupportedChannel<Self::Caps>);
}

/// The disabled-only setting of a backend whose interface addresses change
/// after its install; one whose hardware fixes them (a station
/// address, set by its cold start) does not implement it.
pub trait AddressSetup: LowerMacSetup {
    /// Set the address of the interface of the role `R`, in place of the
    /// one the install or an earlier call gave it.
    fn set_address<R: InterfaceRole>(&mut self, address: IndividualAddress);
}

mod extensions;

pub use extensions::{
    AdmittedAggregate, AggregateAttempt, AggregatePushResult, AggregateRefused, AggregateState,
    AirReservationPhase, AmpduPhase, AmpduRate, BeaconTimingPhase, BeaconWindow,
    CheckedPoolAggregate, ClassSubframe, EmptyAggregate, InterfaceAggregate,
    InterfaceAggregateAttempt, InterfaceAggregateSubmission, InterfaceProtection,
    MAX_RESERVATION_MICROS, MonitorPhase, MpduStartSpacing, NonEmptyAggregate, NonEmptyState,
    PhaseAggregate, PoolSubframe, SubframeSource, TbttPlan,
};

#[cfg(test)]
mod tests;
