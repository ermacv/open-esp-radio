//! Conversions between the ESP32-S31 MAC's typed rates, receive prefix and
//! completions and the portable values of the lower-MAC port.
//!
//! The chip types keep what only this chip needs (rate-control codes, raw
//! HE-SIG words, completion status and detail bytes); the portable values
//! carry what every backend reports. A conversion that has no portable
//! meaning, such as an HE MU reception or a rate the S31 formatter cannot
//! send, fails closed instead of approximating.

use oer_ieee80211_lower_mac::RxMeta;
use oer_ieee80211_mac::{
    channel::Channel,
    phy::{self, DsssPreamble, FecCoding, HeGiLtf, PhyRate, PpduBandwidth, SpatialStreams},
};
use oer_ieee80211_softmac::MacRxMetadata;

use crate::{
    rx::{HeBandwidth, HeGuardIntervalAndLtf, RxBasebandFormat, RxPhyInfo},
    tx::{
        HeBccDcmMcs, HeFecCoding, HeLdpcDcmMcs, HeMcs, HeRate, HtChannelWidth, HtGuardInterval,
        HtMcs, HtRate, LegacyRate, TxPhyRate,
    },
};

impl LegacyRate {
    /// The portable non-HT rate.
    pub const fn phy_rate(self) -> phy::LegacyRate {
        match self {
            Self::Dsss1MLong => phy::LegacyRate::Dsss1M,
            Self::Dsss2MLong => phy::LegacyRate::Dsss2M(DsssPreamble::Long),
            Self::Cck5M5Long => phy::LegacyRate::Cck5M5(DsssPreamble::Long),
            Self::Cck11MLong => phy::LegacyRate::Cck11M(DsssPreamble::Long),
            Self::Dsss2MShort => phy::LegacyRate::Dsss2M(DsssPreamble::Short),
            Self::Cck5M5Short => phy::LegacyRate::Cck5M5(DsssPreamble::Short),
            Self::Cck11MShort => phy::LegacyRate::Cck11M(DsssPreamble::Short),
            Self::Ofdm6M => phy::LegacyRate::Ofdm6M,
            Self::Ofdm9M => phy::LegacyRate::Ofdm9M,
            Self::Ofdm12M => phy::LegacyRate::Ofdm12M,
            Self::Ofdm18M => phy::LegacyRate::Ofdm18M,
            Self::Ofdm24M => phy::LegacyRate::Ofdm24M,
            Self::Ofdm36M => phy::LegacyRate::Ofdm36M,
            Self::Ofdm48M => phy::LegacyRate::Ofdm48M,
            Self::Ofdm54M => phy::LegacyRate::Ofdm54M,
        }
    }

    /// The S31 rate of a portable non-HT rate; every one has an encoding.
    pub const fn from_phy_rate(rate: phy::LegacyRate) -> Self {
        match rate {
            phy::LegacyRate::Dsss1M => Self::Dsss1MLong,
            phy::LegacyRate::Dsss2M(DsssPreamble::Long) => Self::Dsss2MLong,
            phy::LegacyRate::Cck5M5(DsssPreamble::Long) => Self::Cck5M5Long,
            phy::LegacyRate::Cck11M(DsssPreamble::Long) => Self::Cck11MLong,
            phy::LegacyRate::Dsss2M(DsssPreamble::Short) => Self::Dsss2MShort,
            phy::LegacyRate::Cck5M5(DsssPreamble::Short) => Self::Cck5M5Short,
            phy::LegacyRate::Cck11M(DsssPreamble::Short) => Self::Cck11MShort,
            phy::LegacyRate::Ofdm6M => Self::Ofdm6M,
            phy::LegacyRate::Ofdm9M => Self::Ofdm9M,
            phy::LegacyRate::Ofdm12M => Self::Ofdm12M,
            phy::LegacyRate::Ofdm18M => Self::Ofdm18M,
            phy::LegacyRate::Ofdm24M => Self::Ofdm24M,
            phy::LegacyRate::Ofdm36M => Self::Ofdm36M,
            phy::LegacyRate::Ofdm48M => Self::Ofdm48M,
            phy::LegacyRate::Ofdm54M => Self::Ofdm54M,
        }
    }
}

impl HeGuardIntervalAndLtf {
    pub const fn phy_gi_ltf(self) -> HeGiLtf {
        match self {
            Self::OneLtf800Ns => HeGiLtf::Ltf1xGi800Ns,
            Self::TwoLtf800Ns => HeGiLtf::Ltf2xGi800Ns,
            Self::TwoLtf1600Ns => HeGiLtf::Ltf2xGi1600Ns,
            Self::FourLtf3200Ns => HeGiLtf::Ltf4xGi3200Ns,
        }
    }

    pub const fn from_phy_gi_ltf(gi_ltf: HeGiLtf) -> Self {
        match gi_ltf {
            HeGiLtf::Ltf1xGi800Ns => Self::OneLtf800Ns,
            HeGiLtf::Ltf2xGi800Ns => Self::TwoLtf800Ns,
            HeGiLtf::Ltf2xGi1600Ns => Self::TwoLtf1600Ns,
            HeGiLtf::Ltf4xGi3200Ns => Self::FourLtf3200Ns,
        }
    }
}

impl TxPhyRate {
    /// The portable rate. Every S31 transmit rate has one: HT is one
    /// spatial stream at the queue's width, HE is HE20 SU with one spatial
    /// stream.
    pub fn phy_rate(self) -> PhyRate {
        match self {
            Self::Legacy(rate) => PhyRate::Legacy(rate.phy_rate()),
            Self::Ht(rate) => PhyRate::Ht(
                phy::HtRate::new(
                    phy::HtMcs::new(rate.mcs.index()).expect("S31 HT MCS is 0-7"),
                    match rate.channel_width {
                        HtChannelWidth::Mhz20 => PpduBandwidth::Mhz20,
                        HtChannelWidth::Mhz40 => PpduBandwidth::Mhz40,
                    },
                    matches!(rate.guard_interval, HtGuardInterval::Short400Ns),
                )
                .expect("HT at 20 or 40 MHz"),
            ),
            Self::He(rate) => PhyRate::He(
                phy::HeRate::new(
                    phy::HeMcs::new(rate.mcs().index()).expect("S31 HE MCS is 0-9"),
                    SpatialStreams::ONE,
                    PpduBandwidth::Mhz20,
                    rate.guard_interval_and_ltf().phy_gi_ltf(),
                    match rate.fec_coding() {
                        HeFecCoding::Bcc => FecCoding::Bcc,
                        HeFecCoding::Ldpc => FecCoding::Ldpc,
                    },
                    rate.is_dcm(),
                )
                .expect("S31 DCM rates use MCS 0, 1, 3 or 4"),
            ),
        }
    }
}

/// A portable rate the S31 transmit formatter cannot send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedTxRate {
    /// HT above MCS 7: more than the S31's one spatial stream.
    HtMcs,
    /// HT at 80 or 160 MHz, which HT does not have.
    HtBandwidth,
    /// HE above MCS 9, the bounded HE SU formatter's limit.
    HeMcs,
    /// HE with more than one spatial stream.
    HeSpatialStreams,
    /// HE wider than 20 MHz; the S31 formatter sends HE20 SU only.
    HeBandwidth,
    /// HE DCM at an MCS the coding's DCM table does not hold.
    HeDcm,
}

impl TryFrom<PhyRate> for TxPhyRate {
    type Error = UnsupportedTxRate;

    fn try_from(rate: PhyRate) -> Result<Self, UnsupportedTxRate> {
        match rate {
            PhyRate::Legacy(rate) => Ok(Self::Legacy(LegacyRate::from_phy_rate(rate))),
            PhyRate::Ht(rate) => {
                let mcs = HtMcs::from_index(rate.mcs().index()).ok_or(UnsupportedTxRate::HtMcs)?;
                let width = match rate.bandwidth() {
                    PpduBandwidth::Mhz20 => HtChannelWidth::Mhz20,
                    PpduBandwidth::Mhz40 => HtChannelWidth::Mhz40,
                    PpduBandwidth::Mhz80 | PpduBandwidth::Mhz160 => {
                        return Err(UnsupportedTxRate::HtBandwidth);
                    }
                };
                let guard_interval = if rate.short_gi() {
                    HtGuardInterval::Short400Ns
                } else {
                    HtGuardInterval::Long800Ns
                };
                Ok(Self::Ht(HtRate::new(mcs, guard_interval, width)))
            }
            PhyRate::He(rate) => {
                if rate.spatial_streams() != SpatialStreams::ONE {
                    return Err(UnsupportedTxRate::HeSpatialStreams);
                }
                if rate.bandwidth() != PpduBandwidth::Mhz20 {
                    return Err(UnsupportedTxRate::HeBandwidth);
                }
                let gi_ltf = HeGuardIntervalAndLtf::from_phy_gi_ltf(rate.gi_ltf());
                let index = rate.mcs().index();
                let he = match (rate.fec(), rate.dcm()) {
                    (FecCoding::Bcc, false) => HeRate::new(
                        HeMcs::from_index(index).ok_or(UnsupportedTxRate::HeMcs)?,
                        gi_ltf,
                    ),
                    (FecCoding::Ldpc, false) => HeRate::ldpc(
                        HeMcs::from_index(index).ok_or(UnsupportedTxRate::HeMcs)?,
                        gi_ltf,
                    ),
                    (FecCoding::Bcc, true) => HeRate::bcc_dcm(
                        match index {
                            0 => HeBccDcmMcs::Mcs0,
                            1 => HeBccDcmMcs::Mcs1,
                            3 => HeBccDcmMcs::Mcs3,
                            _ => return Err(UnsupportedTxRate::HeDcm),
                        },
                        gi_ltf,
                    ),
                    (FecCoding::Ldpc, true) => HeRate::ldpc_dcm(
                        match index {
                            0 => HeLdpcDcmMcs::Mcs0,
                            1 => HeLdpcDcmMcs::Mcs1,
                            3 => HeLdpcDcmMcs::Mcs3,
                            4 => HeLdpcDcmMcs::Mcs4,
                            _ => return Err(UnsupportedTxRate::HeDcm),
                        },
                        gi_ltf,
                    ),
                };
                Ok(Self::He(he))
            }
        }
    }
}

impl RxPhyInfo {
    /// The portable rate of the received PPDU, or `None` when the prefix
    /// holds none.
    ///
    /// Non-HT formats decode the five-bit `rate` field, which the public
    /// `esp_wifi_rxctrl_t` defines with the `wifi_phy_rate_t` codes of
    /// [`LegacyRate`]; the code must agree with the decoded format (DSSS or
    /// CCK for 802.11b, OFDM otherwise). HT decodes HT-SIG and HE SU decodes
    /// HE-SIG-A; HE with a Doppler midamble has no spatial-stream count.
    /// HE extended-range SU, HE MU, HE trigger-based, VHT and unknown
    /// formats have no portable rate here: the MU and TB rates are per-user
    /// fields this prefix does not carry, and the extended-range bandwidth
    /// field selects a resource unit rather than a PPDU bandwidth.
    pub fn phy_rate(self) -> Option<PhyRate> {
        match self.baseband_format() {
            RxBasebandFormat::Dot11b | RxBasebandFormat::Ofdm => {
                let rate = LegacyRate::from_code(self.rate)?.phy_rate();
                let ofdm = matches!(self.baseband_format(), RxBasebandFormat::Ofdm);
                (rate.is_ofdm() == ofdm).then_some(PhyRate::Legacy(rate))
            }
            RxBasebandFormat::Ht => {
                let signal = self.ht_signal()?;
                let bandwidth = if signal.channel_width_mhz == 40 {
                    PpduBandwidth::Mhz40
                } else {
                    PpduBandwidth::Mhz20
                };
                phy::HtRate::new(
                    phy::HtMcs::new(signal.mcs)?,
                    bandwidth,
                    signal.short_guard_interval,
                )
                .map(PhyRate::Ht)
            }
            RxBasebandFormat::HeSu => {
                let signal = self.he_su_signal()?;
                phy::HeRate::new(
                    phy::HeMcs::new(signal.mcs)?,
                    SpatialStreams::new(signal.spatial_stream_count()?)?,
                    match signal.bandwidth {
                        HeBandwidth::Mhz20 => PpduBandwidth::Mhz20,
                        HeBandwidth::Mhz40 => PpduBandwidth::Mhz40,
                        HeBandwidth::Mhz80 => PpduBandwidth::Mhz80,
                        HeBandwidth::Mhz160Or80Plus80 => PpduBandwidth::Mhz160,
                    },
                    signal.guard_interval_and_ltf.phy_gi_ltf(),
                    if signal.ldpc {
                        FecCoding::Ldpc
                    } else {
                        FecCoding::Bcc
                    },
                    signal.dcm,
                )
                .map(PhyRate::He)
            }
            RxBasebandFormat::Vht
            | RxBasebandFormat::HeMu
            | RxBasebandFormat::HeExtendedRangeSu
            | RxBasebandFormat::HeTriggerBased
            | RxBasebandFormat::VhtMu
            | RxBasebandFormat::Unknown(_) => None,
        }
    }
}

/// The portable receive metadata of an S31 prefix decoded by
/// [`crate::rx::decode_normalized_rx_metadata`], on the configured
/// `channel`.
///
/// The S31 prefix fields this driver has proven carry no noise floor or
/// timestamp, so those stay unavailable here (the lower-MAC core adds the
/// PHY's noise-floor estimate and the port its timestamp); a rate without
/// a portable meaning ([`RxPhyInfo::phy_rate`]) becomes unavailable with
/// it.
pub fn rx_meta(metadata: MacRxMetadata<RxPhyInfo>, channel: Channel) -> RxMeta {
    let metadata = metadata.map_rate(RxPhyInfo::phy_rate);
    RxMeta {
        channel,
        rate: metadata.rate,
        rssi_dbm: metadata.rssi_dbm,
        crypto: metadata.crypto,
        s_mpdu: metadata.s_mpdu,
        ampdu: metadata.ampdu,
        amsdu: metadata.amsdu,
        ..RxMeta::unavailable(channel)
    }
}

#[cfg(test)]
mod tests;
