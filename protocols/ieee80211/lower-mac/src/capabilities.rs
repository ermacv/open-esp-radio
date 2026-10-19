//! What a lower-MAC backend supports and what it performs autonomously.

use oer_ieee80211_mac::{
    channel::{Band, Channel, ChannelWidth},
    phy::{FecCoding, HeMcs, HtMcs, PhyRate},
    qos::WmmAccessCategory,
};
use oer_radio_coex::CoexPriority;

use crate::control::{ReceiveFilter, VifRole};

/// A set of bands.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BandSet(u8);

impl BandSet {
    /// No radio bands.
    pub const NONE: Self = Self(0);
    /// The 2.4 GHz band.
    pub const GHZ2_4: Self = Self(1 << 0);
    /// The 5 GHz band.
    pub const GHZ5: Self = Self(1 << 1);

    /// All bands present in either set.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether this set includes `band`.
    pub const fn contains(self, band: Band) -> bool {
        let bit = match band {
            Band::Ghz2_4 => Self::GHZ2_4.0,
            Band::Ghz5 => Self::GHZ5.0,
        };
        self.0 & bit != 0
    }
}

/// A set of channel widths.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct WidthSet(u8);

impl WidthSet {
    /// No channel widths.
    pub const NONE: Self = Self(0);
    /// Twenty-megahertz channels.
    pub const MHZ20: Self = Self(1 << 0);
    /// Both forty-megahertz geometries.
    pub const MHZ40: Self = Self(1 << 1);

    /// All widths present in either set.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether this set includes the channel geometry `width`.
    pub const fn contains(self, width: ChannelWidth) -> bool {
        let bit = match width {
            ChannelWidth::Mhz20 => Self::MHZ20.0,
            ChannelWidth::Mhz40Above | ChannelWidth::Mhz40Below => Self::MHZ40.0,
        };
        self.0 & bit != 0
    }
}

/// A set of PPDU formats.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PhyFormatSet(u8);

impl PhyFormatSet {
    /// No PPDU formats.
    pub const NONE: Self = Self(0);
    /// Non-HT PPDUs: DSSS, CCK and OFDM.
    pub const NON_HT: Self = Self(1 << 0);
    /// High-throughput PPDUs.
    pub const HT: Self = Self(1 << 1);
    /// High-efficiency PPDUs.
    pub const HE: Self = Self(1 << 2);

    /// All PPDU formats present in either set.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether the format of `rate` is in the set.
    pub const fn contains_rate(self, rate: PhyRate) -> bool {
        let bit = match rate {
            PhyRate::Legacy(_) => Self::NON_HT.0,
            PhyRate::Ht(_) => Self::HT.0,
            PhyRate::He(_) => Self::HE.0,
        };
        self.0 & bit != 0
    }
}

/// A set of coexistence priority levels.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CoexPrioritySet(u8);

impl CoexPrioritySet {
    /// No coexistence priority levels.
    pub const NONE: Self = Self(0);

    /// The set of one level.
    pub const fn only(priority: CoexPriority) -> Self {
        Self(1 << priority as u8)
    }

    /// All priority levels present in either set.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether this set includes `priority`.
    pub const fn contains(self, priority: CoexPriority) -> bool {
        self.0 & (1 << priority as u8) != 0
    }
}

/// The rates a backend sends.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RateSupport {
    /// DSSS and CCK rates, in the 2.4 GHz band only.
    pub dsss_cck: bool,
    /// OFDM rates 6 through 54 Mb/s.
    pub ofdm: bool,
    /// The highest HT MCS; `None` without HT.
    pub ht_max_mcs: Option<HtMcs>,
    /// The highest HE SU MCS; `None` without HE.
    pub he_max_mcs: Option<HeMcs>,
    /// The widest HE PPDU in MHz.
    pub he_max_bandwidth_mhz: u16,
    /// HE dual carrier modulation.
    pub he_dcm: bool,
    /// LDPC coding of HE PPDUs.
    pub he_ldpc: bool,
    /// Spatial streams of HT and HE rates.
    pub spatial_streams: u8,
}

impl RateSupport {
    /// Whether the backend sends `rate`; bandwidth against the channel is
    /// checked by [`LowerMacCapabilities::supports_rate`].
    pub const fn supports(self, rate: PhyRate) -> bool {
        match rate {
            PhyRate::Legacy(rate) => {
                if rate.is_ofdm() {
                    self.ofdm
                } else {
                    self.dsss_cck
                }
            }
            PhyRate::Ht(rate) => match self.ht_max_mcs {
                Some(max) => {
                    rate.mcs().index() <= max.index()
                        && rate.mcs().spatial_streams() <= self.spatial_streams
                }
                None => false,
            },
            PhyRate::He(rate) => match self.he_max_mcs {
                Some(max) => {
                    rate.mcs().index() <= max.index()
                        && rate.spatial_streams().count() <= self.spatial_streams
                        && rate.bandwidth().mhz() <= self.he_max_bandwidth_mhz
                        && (!rate.dcm() || self.he_dcm)
                        && (matches!(rate.fec(), FecCoding::Bcc) || self.he_ldpc)
                }
                None => false,
            },
        }
    }
}

/// Work a backend performs without software on the air timeline.
///
/// The first six are below the port on every backend that has them; the
/// others are work portable code performs above the port unless the
/// backend reports it, in which case the portable owner delegates instead.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct HardwareServices(u16);

impl HardwareServices {
    /// No autonomous hardware services.
    pub const NONE: Self = Self(0);
    /// Append the FCS to every transmitted MPDU and check it on reception.
    pub const FCS: Self = Self(1 << 0);
    /// Send the ACK a received individually addressed frame solicits.
    pub const IMMEDIATE_ACK: Self = Self(1 << 1);
    /// Count down a chosen CSMA/CA backoff and arbitrate the medium.
    pub const BACKOFF_COUNTDOWN: Self = Self(1 << 2);
    /// Encrypt and authenticate protected frames with an installed key.
    pub const CIPHER_TRANSFORM: Self = Self(1 << 3);
    /// Match received A-MPDUs against receive Block Ack agreements and
    /// answer them with a BlockAck.
    pub const RX_BLOCK_ACK_MATCHING: Self = Self(1 << 4);
    /// Capture the BlockAck answering a transmitted A-MPDU.
    pub const TX_BLOCK_ACK_CAPTURE: Self = Self(1 << 5);
    /// Assign sequence numbers.
    pub const SEQUENCE_NUMBERS: Self = Self(1 << 8);
    /// Select the key of an outgoing frame.
    pub const KEY_SELECTION: Self = Self(1 << 9);
    /// Reorder received MPDUs of a Block Ack agreement.
    pub const RX_REORDER: Self = Self(1 << 11);
    /// Select the MPDUs a following A-MPDU attempt retransmits.
    pub const AMPDU_RETRY_SELECTION: Self = Self(1 << 12);

    /// All services present in either set.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every service in `other` is present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The parametric limits of a backend: which values of the base port's
/// operations it accepts. It does not change while the port exists.
///
/// Constructors of checked values refuse inputs outside these limits.
/// Optional operations are expressed by extension traits such as
/// [`AmpduPhase`](crate::AmpduPhase),
/// [`BeaconTimingPhase`](crate::BeaconTimingPhase),
/// [`MonitorPhase`](crate::MonitorPhase) and
/// [`AirReservationPhase`](crate::AirReservationPhase). Each extension's
/// capabilities state its own limits.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LowerMacCapabilities {
    /// Radio bands the backend can tune to.
    pub bands: BandSet,
    /// Supported channel widths and forty-megahertz geometries.
    pub widths: WidthSet,
    /// Transmit rates and their PHY constraints.
    pub rates: RateSupport,
    /// MAC work the backend performs autonomously.
    pub services: HardwareServices,
    /// Virtual interfaces the backend configures at once.
    pub vifs: u8,
    /// Transmit queues, each holding at most one attempt in flight: one or
    /// four. With four, queue `n` serves the access category whose ACI is
    /// `n`; with one, every access category shares it. See
    /// [`Self::tx_queue`].
    pub tx_queues: u8,
    /// The longest MPDU a transmit buffer holds, without FCS
    /// and MIC.
    pub max_mpdu_length: u16,
    /// The lowest [`TxPower::MaxDbm`](crate::TxPower::MaxDbm) ceiling the
    /// backend applies; `None` when it applies none.
    pub tx_power_ceiling_min_dbm: Option<i8>,
    /// The coexistence levels an attempt may carry.
    pub coex_priorities: CoexPrioritySet,
    /// The PPDU formats that send an individually addressed frame with
    /// [`TxResponse::None`](crate::TxResponse::None).
    pub individual_no_ack: PhyFormatSet,
    /// The receive rules a station interface may request.
    pub station_receive_filters: ReceiveFilter,
    /// The receive rules an access-point interface may request.
    pub access_point_receive_filters: ReceiveFilter,
    /// Highest TID of a receive Block Ack agreement.
    pub rx_block_ack_max_tid: u8,
    /// Largest receive Block Ack window.
    pub rx_block_ack_max_window: u16,
    /// Largest AIFSN an EDCA record may carry.
    pub edca_max_aifsn: u8,
    /// Largest contention-window exponent (`ECWmin`, `ECWmax`) an EDCA
    /// record may carry.
    pub edca_max_cw_exponent: u8,
}

impl LowerMacCapabilities {
    /// Whether the backend tunes to `channel`.
    pub const fn supports_channel(self, channel: Channel) -> bool {
        self.bands.contains(channel.band()) && self.widths.contains(channel.width())
    }

    /// Whether the backend sends `rate` on `channel`.
    pub const fn supports_rate(self, rate: PhyRate, channel: Channel) -> bool {
        let dsss_in_5ghz = matches!(channel.band(), Band::Ghz5)
            && matches!(rate, PhyRate::Legacy(legacy) if !legacy.is_ofdm());
        self.rates.supports(rate)
            && !dsss_in_5ghz
            && rate.bandwidth().mhz() <= channel.bandwidth_mhz()
    }

    /// The queue an attempt of `access_category` occupies.
    pub const fn tx_queue(self, access_category: WmmAccessCategory) -> u8 {
        if self.tx_queues >= 4 {
            access_category as u8
        } else {
            0
        }
    }

    /// The receive rules an interface of `role` may request.
    pub const fn receive_filters(self, role: VifRole) -> ReceiveFilter {
        match role {
            VifRole::Station => self.station_receive_filters,
            VifRole::AccessPoint => self.access_point_receive_filters,
        }
    }
}
