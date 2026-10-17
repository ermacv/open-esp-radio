//! Per-PPDU selection of the IEEE 802.11 protection exchange.
//!
//! Four independent sources can require a control frame before a PPDU:
//!
//! | Source | Owner | Mechanism |
//! | --- | --- | --- |
//! | ERP Use_Protection | BSS ERP element | CTS-to-self at a DSSS/HR rate, any receiver |
//! | HT Protection | BSS HT Operation element | RTS/CTS to an individual receiver, CTS-to-self to a group |
//! | HE TXOP Duration RTS Threshold | BSS HE Operation element | RTS/CTS to an individual receiver |
//! | dot11RTSThreshold | Local configuration | RTS/CTS before one long individual MPDU, never an A-MPDU |
//!
//! An RTS/CTS exchange also sets the NAV of every station that hears the CTS,
//! so it supersedes CTS-to-self when both are required. Group-addressed PPDUs
//! never carry an RTS: they have no receiver that could answer it.
//!
//! [`WifiTxProtectionPolicy::select`] is total. Every PPDU the ordinary and
//! aggregate owners publish receives exactly one [`TxProtection`]; there is no
//! admission frontier for a protection requirement. The ESP32-S31 MAC generates
//! the RTS or CTS frame, its Duration and the SIFS sequence from the queue
//! request flag and the programmed PPDU.
//!
//! The selection itself is portable: `oer_ieee80211_upper_mac::protection`
//! decides over the portable rates, and the HE TXOP byte budget is the
//! Espressif estimate of `oer_espressif_ieee80211_policy::he_txop`. The types
//! here are the chip's vocabulary over [`TxPhyRate`] and the S31
//! [`LegacyRate`] codes; the policy converts them to the portable values and
//! back.
//!
//! The pinned vendor PP does not implement ERP or HT protection: it stores the
//! HT Protection field without reading it and never requests CTS-to-self.
//! Its only protection sources are the length threshold in `lmacTxFrame` and
//! the HE byte threshold in `ppCheckTxRTS`. The ERP and HT rows above follow
//! IEEE 802.11 and the Linux mac80211, mt76x02 and rt2800 implementations.

use core::num::NonZeroU16;

use oer_espressif_ieee80211_policy::he_txop::{
    EspressifHeTxopRtsBudget, maximum_unprotected_apep_bytes,
};
use oer_ieee80211_mac::{
    phy::PhyRate,
    protection::{ErpProtection, HtProtectionMode},
};
use oer_ieee80211_upper_mac::protection as portable;

#[cfg(test)]
use crate::tx::HtChannelWidth;
use crate::tx::{HeRate, LegacyRate, TxPhyRate};

/// Finite HE TXOP Duration RTS Threshold in the element's native 32-us units.
///
/// Encodings zero and 1023 disable the rule, as in the complete vendor
/// `ieee80211_parse_heopr`, and do not construct this type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HeTxopDurationRtsThreshold(NonZeroU16);

impl HeTxopDurationRtsThreshold {
    pub const fn new(units_32_us: u16) -> Option<Self> {
        if units_32_us >= 0x03ff {
            return None;
        }
        match NonZeroU16::new(units_32_us) {
            Some(units) => Some(Self(units)),
            None => None,
        }
    }

    pub const fn units_32_us(self) -> u16 {
        self.0.get()
    }
}

/// Peer nominal packet padding used by the vendor HE RTS byte table.
///
/// SOURCE: complete `libnet80211.a[ieee80211_he.o]::ieee80211_parse_hecap`
/// stores this code at node offset `0x354`; complete
/// `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab` subtracts 8 us
/// for code one, 16 us for code two and nothing for any other code.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HePacketPadding {
    #[default]
    None,
    Us8,
    Us16,
}

impl HePacketPadding {
    pub const fn from_vendor_code(code: u8) -> Self {
        match code {
            1 => Self::Us8,
            2 => Self::Us16,
            _ => Self::None,
        }
    }

    const fn portable(self) -> portable::HePacketPadding {
        match self {
            Self::None => portable::HePacketPadding::None,
            Self::Us8 => portable::HePacketPadding::Us8,
            Self::Us16 => portable::HePacketPadding::Us16,
        }
    }
}

/// HE TXOP duration rule applied by one associated non-AP HE station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeTxopRtsRule {
    threshold: HeTxopDurationRtsThreshold,
    padding: HePacketPadding,
}

impl HeTxopRtsRule {
    pub const fn new(threshold: HeTxopDurationRtsThreshold, padding: HePacketPadding) -> Self {
        Self { threshold, padding }
    }

    pub const fn threshold(self) -> HeTxopDurationRtsThreshold {
        self.threshold
    }

    pub const fn padding(self) -> HePacketPadding {
        self.padding
    }

    /// Largest HE SU APEP length whose TXOP stays below the threshold: the
    /// Espressif estimate of
    /// `oer_espressif_ieee80211_policy::he_txop::maximum_unprotected_apep_bytes`,
    /// recovered from complete
    /// `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab`.
    pub fn maximum_unprotected_apep_bytes(self, rate: HeRate) -> u16 {
        let PhyRate::He(rate) = TxPhyRate::He(rate).phy_rate() else {
            unreachable!("an S31 HE rate is a portable HE rate");
        };
        maximum_unprotected_apep_bytes(self.threshold.portable(), self.padding.portable(), rate)
    }
}

impl HeTxopDurationRtsThreshold {
    const fn portable(self) -> portable::HeTxopDurationRtsThreshold {
        match portable::HeTxopDurationRtsThreshold::new(self.0.get()) {
            Some(threshold) => threshold,
            None => panic!("both thresholds admit the same encodings"),
        }
    }
}

/// Non-HT rates in one BSSBasicRateSet: the portable set.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BasicRates(portable::BasicRates);

impl BasicRates {
    /// Rates that every ERP station supports when a BSS names no usable rate.
    pub const ERP_MANDATORY: Self = Self(portable::BasicRates::ERP_MANDATORY);

    /// Collect rates marked basic (bit seven) in Supported Rates and Extended
    /// Supported Rates element bodies, in 500-kbit/s units.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8]) -> Self {
        Self(portable::BasicRates::from_rate_elements(
            supported, extended,
        ))
    }

    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

/// Protection facts advertised by the BSS that one PPDU belongs to.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BssProtection {
    pub erp: ErpProtection,
    pub ht: HtProtectionMode,
    /// HE Operation TXOP Duration RTS Threshold of the BSS.
    pub he_txop_rts_threshold: Option<HeTxopDurationRtsThreshold>,
    /// Peer nominal packet padding, fixed at association.
    pub he_packet_padding: HePacketPadding,
    pub basic_rates: BasicRates,
    /// Capability Information Short Preamble.
    pub short_preamble: bool,
}

impl BssProtection {
    /// A BSS without protection requirements whose basic rates are unknown.
    pub const UNPROTECTED: Self = Self {
        erp: ErpProtection::NONE,
        ht: HtProtectionMode::None,
        he_txop_rts_threshold: None,
        he_packet_padding: HePacketPadding::None,
        basic_rates: BasicRates(portable::BasicRates::EMPTY),
        short_preamble: false,
    };

    const fn portable(self) -> portable::BssProtection {
        portable::BssProtection {
            erp: self.erp,
            ht: self.ht,
            he_txop_rts_threshold: match self.he_txop_rts_threshold {
                Some(threshold) => Some(threshold.portable()),
                None => None,
            },
            he_packet_padding: self.he_packet_padding.portable(),
            basic_rates: self.basic_rates.0,
            short_preamble: self.short_preamble,
        }
    }
}

/// Local dot11RTSThreshold in PSDU bytes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RtsLengthThreshold(u16);

impl RtsLengthThreshold {
    /// Complete `lmacInit` stores 0x092a at `lmacConfMib+0x16`; complete
    /// `lmacIsLongFrame` requests RTS for a longer individual PSDU.
    pub const VENDOR_DEFAULT: Self = Self(0x092a);

    pub const fn new(bytes: u16) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> u16 {
        self.0
    }
}

/// Receiver class taken from the MPDU Address 1 field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxReceiver {
    Individual,
    Group,
}

impl TxReceiver {
    /// Classify the receiver address (RA). The Ethernet destination of a
    /// To-DS frame is Address 3 and does not select the exchange.
    pub const fn from_address1(address1: &[u8; 6]) -> Self {
        if address1[0] & 1 != 0 {
            Self::Group
        } else {
            Self::Individual
        }
    }
}

/// The PSDU carried by one PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxPsdu {
    /// One MPDU; `length` includes MIC and FCS.
    Mpdu { length: u32 },
    /// One A-MPDU; `length` is the complete aggregate, the HE APEP length.
    Ampdu { length: u32 },
}

impl TxPsdu {
    pub const fn length(self) -> u32 {
        match self {
            Self::Mpdu { length } | Self::Ampdu { length } => length,
        }
    }
}

/// One PPDU presented for protection selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedPpdu {
    pub rate: TxPhyRate,
    pub receiver: TxReceiver,
    pub psdu: TxPsdu,
}

/// Why one PPDU is protected; several sources can apply at once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxProtectionReasons(u8);

impl TxProtectionReasons {
    pub const ERP: Self = Self(1 << 0);
    pub const HT: Self = Self(1 << 1);
    pub const HE_TXOP_DURATION: Self = Self(1 << 2);
    pub const LENGTH: Self = Self(1 << 3);

    const fn from_portable(reasons: portable::TxProtectionReasons) -> Self {
        Self::default_const()
            .with(
                Self::ERP,
                reasons.contains(portable::TxProtectionReasons::ERP),
            )
            .with(
                Self::HT,
                reasons.contains(portable::TxProtectionReasons::HT),
            )
            .with(
                Self::HE_TXOP_DURATION,
                reasons.contains(portable::TxProtectionReasons::HE_TXOP_DURATION),
            )
            .with(
                Self::LENGTH,
                reasons.contains(portable::TxProtectionReasons::LENGTH),
            )
    }

    const fn default_const() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    const fn with(self, other: Self, present: bool) -> Self {
        if present {
            Self(self.0 | other.0)
        } else {
            self
        }
    }
}

/// Control exchange that precedes one PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtection {
    None,
    CtsToSelf { rate: LegacyRate },
    RtsCts { rate: LegacyRate },
}

impl TxProtection {
    /// Transmit rate of the protection frame, if any.
    pub const fn control_rate(self) -> Option<LegacyRate> {
        match self {
            Self::None => None,
            Self::CtsToSelf { rate } | Self::RtsCts { rate } => Some(rate),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxProtectionDecision {
    pub protection: TxProtection,
    pub reasons: TxProtectionReasons,
}

impl TxProtectionDecision {
    pub const UNPROTECTED: Self = Self {
        protection: TxProtection::None,
        reasons: TxProtectionReasons(0),
    };
}

/// BSS requirements plus the local length threshold for one transmitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxProtectionPolicy {
    bss: BssProtection,
    rts_length_threshold: Option<RtsLengthThreshold>,
}

impl Default for WifiTxProtectionPolicy {
    fn default() -> Self {
        Self::new(Some(RtsLengthThreshold::VENDOR_DEFAULT))
    }
}

impl WifiTxProtectionPolicy {
    /// A transmitter outside any BSS, with its local length threshold.
    pub const fn new(rts_length_threshold: Option<RtsLengthThreshold>) -> Self {
        Self {
            bss: BssProtection::UNPROTECTED,
            rts_length_threshold,
        }
    }

    pub const fn bss(self) -> BssProtection {
        self.bss
    }

    pub const fn rts_length_threshold(self) -> Option<RtsLengthThreshold> {
        self.rts_length_threshold
    }

    /// Replace the BSS facts, keeping the local length threshold.
    pub fn install_bss(&mut self, bss: BssProtection) {
        self.bss = bss;
    }

    /// Replace the local dot11RTSThreshold; `None` disables the length rule.
    pub fn set_rts_length_threshold(&mut self, threshold: Option<RtsLengthThreshold>) {
        self.rts_length_threshold = threshold;
    }

    /// Leave the BSS: no BSS requirement remains.
    pub fn clear_bss(&mut self) {
        self.bss = BssProtection::UNPROTECTED;
    }

    /// The portable policy of these facts.
    fn portable(&self) -> portable::ProtectionPolicy {
        let mut policy = portable::ProtectionPolicy::new(
            self.rts_length_threshold
                .map(|threshold| portable::RtsLengthThreshold::new(threshold.0)),
        );
        policy.install_bss(self.bss.portable());
        policy
    }

    /// Select the exchange and its control-frame rate for one PPDU.
    ///
    /// The single-MPDU length rule follows complete `lmacIsLongFrame`,
    /// reached only from `lmacTxFrame`: the vendor A-MPDU path never consults
    /// the length threshold, so an aggregate is protected only by the BSS
    /// rules.
    pub fn select(&self, ppdu: ProtectedPpdu) -> TxProtectionDecision {
        let decision = self.portable().select(
            portable::ProtectedPpdu {
                rate: ppdu.rate.phy_rate(),
                receiver: match ppdu.receiver {
                    TxReceiver::Individual => portable::TxReceiver::Individual,
                    TxReceiver::Group => portable::TxReceiver::Group,
                },
                psdu: match ppdu.psdu {
                    TxPsdu::Mpdu { length } => portable::TxPsdu::Mpdu { length },
                    TxPsdu::Ampdu { length } => portable::TxPsdu::Ampdu { length },
                },
            },
            &EspressifHeTxopRtsBudget,
        );
        TxProtectionDecision {
            protection: match decision.protection {
                portable::TxProtection::None => TxProtection::None,
                portable::TxProtection::CtsToSelf { rate } => TxProtection::CtsToSelf {
                    rate: LegacyRate::from_phy_rate(rate),
                },
                portable::TxProtection::RtsCts { rate } => TxProtection::RtsCts {
                    rate: LegacyRate::from_phy_rate(rate),
                },
            },
            reasons: TxProtectionReasons::from_portable(decision.reasons),
        }
    }

    /// Control-frame rate for one data rate.
    ///
    /// The fastest basic rate not faster than the data rate, or the slowest
    /// basic rate when all are faster, as in mac80211 `rate_control_*`. Under
    /// ERP protection only DSSS/HR rates are eligible so that non-ERP
    /// stations decode the NAV. A BSS whose basic set names no eligible rate
    /// falls back to the ERP mandatory set. DSSS/HR control frames use the
    /// short preamble only when the BSS allows it and does not require
    /// Barker long preambles; 1 Mbit/s always uses the long preamble.
    ///
    /// A caller that chooses the protection exchange itself, such as the
    /// lower-MAC port's single-attempt path, takes its control rate from here.
    pub fn control_rate(&self, data: TxPhyRate) -> LegacyRate {
        LegacyRate::from_phy_rate(self.portable().control_rate(data.phy_rate()))
    }
}

#[cfg(test)]
mod tests;
