//! What a lower-MAC backend supports and what it performs autonomously.

use oer_protocols_ieee80211_wire::qos::Tid;
use oer_protocols_ieee80211_wire::{
    channel::{Band, Channel, ChannelWidth},
    he::HeChannelWidthSet,
    phy::{FecCoding, HeMcs, HtMcs, PhyRate, PpduBandwidth},
    qos::WmmAccessCategory,
};

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

/// A channel width without its geometry: the side of a 40 MHz channel's
/// secondary is the access point's.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Width {
    /// 20 MHz.
    Mhz20,
    /// 40 MHz.
    Mhz40,
}

impl Width {
    /// The width of `width`, its geometry left out.
    pub const fn of(width: ChannelWidth) -> Self {
        match width {
            ChannelWidth::Mhz20 => Self::Mhz20,
            ChannelWidth::Mhz40Above | ChannelWidth::Mhz40Below => Self::Mhz40,
        }
    }

    /// The width in MHz.
    pub const fn mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40 => 40,
        }
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

    /// The set of `width` alone.
    pub const fn of(width: Width) -> Self {
        match width {
            Width::Mhz20 => Self::MHZ20,
            Width::Mhz40 => Self::MHZ40,
        }
    }

    /// Whether this set includes `width`.
    pub const fn contains_width(self, width: Width) -> bool {
        self.0 & Self::of(width).0 != 0
    }

    /// Whether this set includes a PPDU of `bandwidth`: 20 and 40 MHz are
    /// the widths a set names.
    pub const fn contains_bandwidth(self, bandwidth: PpduBandwidth) -> bool {
        match bandwidth {
            PpduBandwidth::Mhz20 => self.0 & Self::MHZ20.0 != 0,
            PpduBandwidth::Mhz40 => self.0 & Self::MHZ40.0 != 0,
            PpduBandwidth::Mhz80 | PpduBandwidth::Mhz160 => false,
        }
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

/// A PPDU format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PhyFormat {
    /// Non-HT: DSSS, CCK and OFDM.
    NonHt,
    /// High throughput.
    Ht,
    /// High efficiency.
    He,
}

impl PhyFormat {
    /// The format of `rate`.
    pub const fn of_rate(rate: PhyRate) -> Self {
        match rate {
            PhyRate::Legacy(_) => Self::NonHt,
            PhyRate::Ht(_) => Self::Ht,
            PhyRate::He(_) => Self::He,
        }
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

    /// The set of `format` alone.
    pub const fn of(format: PhyFormat) -> Self {
        match format {
            PhyFormat::NonHt => Self::NON_HT,
            PhyFormat::Ht => Self::HT,
            PhyFormat::He => Self::HE,
        }
    }

    /// Whether `format` is in the set.
    pub const fn contains(self, format: PhyFormat) -> bool {
        self.0 & Self::of(format).0 != 0
    }

    /// Whether the format of `rate` is in the set.
    pub const fn contains_rate(self, rate: PhyRate) -> bool {
        self.contains(PhyFormat::of_rate(rate))
    }
}

/// The channel widths a backend sends and receives each PPDU format at: the
/// pairs of a format and a width it offers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FormatWidths {
    /// Non-HT.
    pub non_ht: WidthSet,
    /// HT.
    pub ht: WidthSet,
    /// HE.
    pub he: WidthSet,
}

impl FormatWidths {
    /// The widths `format` is offered at.
    pub const fn of(self, format: PhyFormat) -> WidthSet {
        match format {
            PhyFormat::NonHt => self.non_ht,
            PhyFormat::Ht => self.ht,
            PhyFormat::He => self.he,
        }
    }

    /// Every width some format is offered at: the channels the backend
    /// tunes to.
    pub const fn channels(self) -> WidthSet {
        self.non_ht.union(self.ht).union(self.he)
    }

    /// The Supported Channel Width Set an HE Capabilities element carries
    /// for HE's widths in `bands`, or `None` where a band's rule cannot say
    /// them: in the 2.4 GHz band, B0 is 40 MHz; in the 5 GHz band, B1 is 40
    /// and 80 MHz together, so 40 MHz alone has no encoding, and a set
    /// naming no band has no element.
    ///
    /// SOURCE: IEEE Std 802.11ax-2021, 9.4.2.248.3; see
    /// [`HeChannelWidthSet`].
    pub const fn he_channel_width_set(self, bands: BandSet) -> Option<HeChannelWidthSet> {
        let wide = self.he.contains_bandwidth(PpduBandwidth::Mhz40);
        if bands.0 == 0 {
            return None;
        }
        let mut field = HeChannelWidthSet::MHZ20;
        if bands.contains(Band::Ghz2_4) && wide {
            field = field.union(HeChannelWidthSet::GHZ2_4_MHZ40);
        }
        if bands.contains(Band::Ghz5) && wide {
            return None;
        }
        Some(field)
    }
}

/// Whether a backend serves one capability: one fact of the backend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Support {
    /// It does not.
    Unsupported,
    /// It does.
    Supported,
}

impl Support {
    /// Whether it does.
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

/// The rates a backend sends.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RateSupport {
    /// DSSS and CCK rates, in the 2.4 GHz band only.
    pub dsss_cck: Support,
    /// OFDM rates 6 through 54 Mb/s.
    pub ofdm: Support,
    /// The highest HT MCS; `None` without HT.
    pub ht_max_mcs: Option<HtMcs>,
    /// The highest HE SU MCS; `None` without HE.
    pub he_max_mcs: Option<HeMcs>,
    /// HE dual carrier modulation.
    pub he_dcm: Support,
    /// LDPC coding of HE PPDUs.
    pub he_ldpc: Support,
    /// Spatial streams of HT and HE rates.
    pub spatial_streams: u8,
}

impl RateSupport {
    /// Whether the backend sends `rate`; its bandwidth against the format's
    /// widths and the channel is checked by
    /// [`LowerMacCapabilities::supports_rate`].
    pub const fn supports(self, rate: PhyRate) -> bool {
        match rate {
            PhyRate::Legacy(rate) => {
                if rate.is_ofdm() {
                    self.ofdm.is_supported()
                } else {
                    self.dsss_cck.is_supported()
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
                        && (!rate.dcm() || self.he_dcm.is_supported())
                        && (matches!(rate.fec(), FecCoding::Bcc) || self.he_ldpc.is_supported())
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
/// Constructors of checked values refuse inputs outside these limits. An
/// optional feature states its own limits in its own capabilities, as a
/// port's arenas state what they aggregate
/// ([`PortArena::ampdu_capabilities`](crate::PortArena::ampdu_capabilities)).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LowerMacCapabilities {
    /// Radio bands the backend can tune to.
    pub bands: BandSet,
    /// The widths each PPDU format is offered at.
    pub formats: FormatWidths,
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
    pub tx_power_ceiling_min_dbm: Option<oer_protocols_ieee80211_wire::power::Dbm>,
    /// The PPDU formats that send an individually addressed frame with
    /// [`TxResponse::None`](crate::TxResponse::None).
    pub individual_no_ack: PhyFormatSet,
    /// The receive rules a station interface may request.
    pub station_receive_filters: ReceiveFilter,
    /// The receive rules an access-point interface may request.
    pub access_point_receive_filters: ReceiveFilter,
    /// Highest TID of a receive Block Ack agreement.
    pub rx_block_ack_max_tid: Tid,
    /// Largest receive Block Ack window.
    pub rx_block_ack_max_window: u16,
    /// Whether the backend delivers a received MPDU carrying an A-MSDU
    /// inside an A-MPDU of a receive Block Ack agreement. Only then does
    /// the recipient grant an originator's request for A-MSDUs in the
    /// agreement (IEEE Std 802.11-2020 9.4.1.14, A-MSDU Supported).
    pub rx_amsdu_in_ampdu: bool,
    /// Largest AIFSN an EDCA record may carry.
    pub edca_max_aifsn: u8,
    /// Largest contention-window exponent (`ECWmin`, `ECWmax`) an EDCA
    /// record may carry.
    pub edca_max_cw_exponent: u8,
    /// Largest TXOP limit retained by the backend.
    /// An access sends a single frame.
    pub edca_max_txop_limit: oer_protocols_ieee80211_wire::extensions::wmm::TxopLimit,
}

impl LowerMacCapabilities {
    /// Whether the backend tunes to `channel`.
    pub const fn supports_channel(self, channel: Channel) -> bool {
        self.bands.contains(channel.band()) && self.formats.channels().contains(channel.width())
    }

    /// Whether the backend sends `rate` on `channel`.
    pub const fn supports_rate(self, rate: PhyRate, channel: Channel) -> bool {
        let dsss_in_5ghz = matches!(channel.band(), Band::Ghz5)
            && matches!(rate, PhyRate::Legacy(legacy) if !legacy.is_ofdm());
        self.rates.supports(rate)
            && !dsss_in_5ghz
            && self
                .formats
                .of(PhyFormat::of_rate(rate))
                .contains_bandwidth(rate.bandwidth())
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

#[cfg(test)]
mod tests {
    use super::*;
    use oer_protocols_ieee80211_wire::{
        channel::Ghz2_4Channel,
        phy::{Dcm, HeGiLtf, HeRate, HtGuardInterval, HtRate, SpatialStreams},
    };

    fn capabilities(ht: WidthSet) -> LowerMacCapabilities {
        LowerMacCapabilities {
            bands: BandSet::GHZ2_4,
            formats: FormatWidths {
                non_ht: WidthSet::MHZ20,
                ht,
                he: WidthSet::MHZ20,
            },
            rates: RateSupport {
                dsss_cck: Support::Supported,
                ofdm: Support::Supported,
                ht_max_mcs: HtMcs::new(7),
                he_max_mcs: HeMcs::new(9),
                he_dcm: Support::Supported,
                he_ldpc: Support::Supported,
                spatial_streams: 1,
            },
            services: HardwareServices::FCS,
            vifs: 2,
            tx_queues: 4,
            max_mpdu_length: 2304,
            tx_power_ceiling_min_dbm: Some(oer_protocols_ieee80211_wire::power::Dbm::new(0)),
            individual_no_ack: PhyFormatSet::NON_HT,
            station_receive_filters: ReceiveFilter::BSS_MEMBER,
            access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
            rx_block_ack_max_tid: oer_protocols_ieee80211_wire::qos::Tid::constant::<7>(),
            rx_block_ack_max_window: 64,
            rx_amsdu_in_ampdu: true,
            edca_max_aifsn: 15,
            edca_max_cw_exponent: 10,
            edca_max_txop_limit: oer_protocols_ieee80211_wire::extensions::wmm::TxopLimit::new(
                u16::MAX,
            ),
        }
    }

    /// A rate is sent at a bandwidth its format is offered at: on a 40 MHz
    /// channel, with HT at 20 and 40 MHz and HE at 20 MHz, an HT rate at
    /// 40 MHz is sent and an HE rate at 40 MHz is not; with HT at 20 MHz
    /// only, neither is.
    #[test]
    fn a_rate_is_sent_at_the_widths_of_its_format() {
        let channel = Channel::from(Ghz2_4Channel::new(6, ChannelWidth::Mhz40Above).unwrap());
        let ht = PhyRate::Ht(
            HtRate::new(
                HtMcs::new(7).unwrap(),
                PpduBandwidth::Mhz40,
                HtGuardInterval::Normal,
            )
            .unwrap(),
        );
        let he = PhyRate::He(
            HeRate::new(
                HeMcs::new(7).unwrap(),
                SpatialStreams::ONE,
                PpduBandwidth::Mhz40,
                HeGiLtf::Ltf2xGi800Ns,
                FecCoding::Bcc,
                Dcm::NotUsed,
            )
            .unwrap(),
        );
        let wide = capabilities(WidthSet::MHZ20.union(WidthSet::MHZ40));
        assert!(wide.supports_channel(channel));
        assert!(wide.supports_rate(ht, channel));
        assert!(!wide.supports_rate(he, channel));
        let narrow = capabilities(WidthSet::MHZ20);
        assert!(!narrow.supports_rate(ht, channel));
        assert!(!narrow.supports_rate(he, channel));
    }
}
