//! Per-PPDU selection of the protection exchange.
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
//! [`ProtectionPolicy::select`] is total: every PPDU receives exactly one
//! [`TxProtection`]. The ERP and HT rows follow IEEE Std 802.11-2020 and the
//! Linux mac80211, mt76x02 and rt2800 implementations. How many bytes of an
//! HE PPDU fit below a TXOP duration threshold depends on the PPDU duration
//! estimate a transmitter uses, so it is an [`HeTxopRtsBudget`] the caller
//! supplies.

use core::num::NonZeroU16;

use oer_ieee80211_lower_mac::Protection;
use oer_ieee80211_mac::{
    phy::{DsssPreamble, HeRate, LegacyRate, PhyRate, PpduBandwidth},
    protection::{ErpProtection, HtProtectionMode},
};

/// Finite HE TXOP Duration RTS Threshold in the element's native 32-µs
/// units.
///
/// Encodings zero and 1023 disable the rule and do not construct this type.
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

/// The peer's nominal packet padding (HE Capabilities).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HePacketPadding {
    #[default]
    None,
    Us8,
    Us16,
}

impl HePacketPadding {
    pub const fn micros(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Us8 => 8,
            Self::Us16 => 16,
        }
    }
}

/// The largest HE SU APEP length whose TXOP stays below a TXOP duration
/// threshold: a transmitter's own PPDU duration estimate.
pub trait HeTxopRtsBudget {
    fn max_unprotected_apep_bytes(
        &self,
        threshold: HeTxopDurationRtsThreshold,
        padding: HePacketPadding,
        rate: HeRate,
    ) -> u16;
}

/// No duration estimate: every HE PPDU to an individual receiver is
/// protected while the BSS advertises a threshold.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtectEveryHeTxop;

impl HeTxopRtsBudget for ProtectEveryHeTxop {
    fn max_unprotected_apep_bytes(
        &self,
        _threshold: HeTxopDurationRtsThreshold,
        _padding: HePacketPadding,
        _rate: HeRate,
    ) -> u16 {
        0
    }
}

impl<B: HeTxopRtsBudget + ?Sized> HeTxopRtsBudget for &B {
    fn max_unprotected_apep_bytes(
        &self,
        threshold: HeTxopDurationRtsThreshold,
        padding: HePacketPadding,
        rate: HeRate,
    ) -> u16 {
        (**self).max_unprotected_apep_bytes(threshold, padding, rate)
    }
}

/// Non-HT rates in one BSSBasicRateSet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BasicRates(u16);

impl BasicRates {
    /// The rates in 500-kb/s units with their long-preamble form; bit `i`
    /// of the set is entry `i`.
    pub(crate) const ORDER: [(u8, LegacyRate); 12] = [
        (2, LegacyRate::Dsss1M),
        (4, LegacyRate::Dsss2M(DsssPreamble::Long)),
        (11, LegacyRate::Cck5M5(DsssPreamble::Long)),
        (22, LegacyRate::Cck11M(DsssPreamble::Long)),
        (12, LegacyRate::Ofdm6M),
        (18, LegacyRate::Ofdm9M),
        (24, LegacyRate::Ofdm12M),
        (36, LegacyRate::Ofdm18M),
        (48, LegacyRate::Ofdm24M),
        (72, LegacyRate::Ofdm36M),
        (96, LegacyRate::Ofdm48M),
        (108, LegacyRate::Ofdm54M),
    ];

    /// No rate.
    pub const EMPTY: Self = Self(0);

    /// Rates that every ERP station supports when a BSS names no usable
    /// rate: 1, 2, 5.5, 11, 6, 12 and 24 Mb/s.
    pub const ERP_MANDATORY: Self = Self(0b0000_0001_0101_1111);

    const ERP_MANDATORY_HEAD: LegacyRate = {
        let [(_, first), ..] = Self::ORDER;
        first
    };
}

// The mandatory set's first entry is present and eligible both with and
// without ERP protection, so its iterator has this head before skip(1).
const _: [(); 1] = [(); (BasicRates::ERP_MANDATORY.0 & 1 != 0
    && matches!(BasicRates::ERP_MANDATORY_HEAD, LegacyRate::Dsss1M))
    as usize];

impl BasicRates {
    /// Collect rates marked basic (bit seven) in Supported Rates and
    /// Extended Supported Rates element bodies, in 500-kb/s units.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8]) -> Self {
        let mut mask = 0;
        for encoded in supported.iter().chain(extended) {
            if encoded & 0x80 == 0 {
                continue;
            }
            for (index, (units, _)) in Self::ORDER.iter().enumerate() {
                if encoded & 0x7f == *units {
                    mask |= 1 << index;
                }
            }
        }
        Self(mask)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The rates of the set, each with its long preamble.
    pub fn rates(self) -> impl Iterator<Item = LegacyRate> {
        Self::ORDER
            .into_iter()
            .enumerate()
            .filter(move |(index, _)| self.0 & (1 << index) != 0)
            .map(|(_, (_, rate))| rate)
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
        basic_rates: BasicRates::EMPTY,
        short_preamble: false,
    };
}

/// Local dot11RTSThreshold in PSDU bytes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RtsLengthThreshold(u16);

impl RtsLengthThreshold {
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
    pub rate: PhyRate,
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

/// The control exchange that precedes one PPDU and its control-frame rate.
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

    /// The exchange a lower-MAC attempt requests; the backend chooses the
    /// control frame's rate itself.
    pub const fn port_protection(self) -> Protection {
        match self {
            Self::None => Protection::None,
            Self::CtsToSelf { .. } => Protection::CtsToSelf,
            Self::RtsCts { .. } => Protection::RtsCts,
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

/// BSS requirements plus the local length threshold of one transmitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectionPolicy {
    bss: BssProtection,
    rts_length_threshold: Option<RtsLengthThreshold>,
}

impl ProtectionPolicy {
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

    /// Select the exchange and its control-frame rate for one PPDU.
    pub fn select(
        &self,
        ppdu: ProtectedPpdu,
        he_budget: &impl HeTxopRtsBudget,
    ) -> TxProtectionDecision {
        let individual = matches!(ppdu.receiver, TxReceiver::Individual);
        let (non_dsss, ht_protected) = match ppdu.rate {
            PhyRate::Legacy(rate) => (rate.is_ofdm(), false),
            PhyRate::Ht(rate) => (
                true,
                ht_protects(
                    self.bss.ht,
                    matches!(rate.bandwidth(), PpduBandwidth::Mhz40),
                ),
            ),
            PhyRate::He(_) => (true, ht_protects(self.bss.ht, false)),
        };
        let he_txop = match (self.bss.he_txop_rts_threshold, ppdu.rate) {
            (Some(threshold), PhyRate::He(rate)) => {
                ppdu.psdu.length()
                    > u32::from(he_budget.max_unprotected_apep_bytes(
                        threshold,
                        self.bss.he_packet_padding,
                        rate,
                    ))
            }
            _ => false,
        };
        // The length threshold applies to one MPDU; an aggregate is protected
        // only by the BSS rules above.
        let length = match ppdu.psdu {
            TxPsdu::Mpdu { length } => self
                .rts_length_threshold
                .is_some_and(|threshold| length > u32::from(threshold.0)),
            TxPsdu::Ampdu { .. } => false,
        };
        let erp = self.bss.erp.use_protection() && non_dsss;
        let reasons = TxProtectionReasons::default()
            .with(TxProtectionReasons::ERP, erp)
            .with(TxProtectionReasons::HT, ht_protected)
            .with(TxProtectionReasons::HE_TXOP_DURATION, individual && he_txop)
            .with(TxProtectionReasons::LENGTH, individual && length);

        let rts = individual && (ht_protected || he_txop || length);
        let protection = if rts {
            TxProtection::RtsCts {
                rate: self.control_rate(ppdu.rate),
            }
        } else if erp || ht_protected {
            TxProtection::CtsToSelf {
                rate: self.control_rate(ppdu.rate),
            }
        } else {
            TxProtection::None
        };
        TxProtectionDecision {
            protection,
            reasons,
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
    /// Barker long preambles; 1 Mb/s always uses the long preamble.
    pub fn control_rate(&self, data: PhyRate) -> LegacyRate {
        let dsss_only = self.bss.erp.use_protection();
        let eligible = |rate: &LegacyRate| !dsss_only || !rate.is_ofdm();
        let mut advertised = self.bss.basic_rates.rates().filter(eligible);
        let data_kbps = data.nominal_kbps();
        let rate = match advertised.next() {
            Some(first) => select_control_rate(first, advertised, data_kbps),
            None => select_control_rate(
                BasicRates::ERP_MANDATORY_HEAD,
                BasicRates::ERP_MANDATORY.rates().filter(eligible).skip(1),
                data_kbps,
            ),
        };
        let short = self.bss.short_preamble && !self.bss.erp.long_preamble_required();
        match (rate, short) {
            (LegacyRate::Dsss2M(_), true) => LegacyRate::Dsss2M(DsssPreamble::Short),
            (LegacyRate::Cck5M5(_), true) => LegacyRate::Cck5M5(DsssPreamble::Short),
            (LegacyRate::Cck11M(_), true) => LegacyRate::Cck11M(DsssPreamble::Short),
            (rate, _) => rate,
        }
    }
}

/// Select from a nonempty candidate sequence. A rate at or below the data
/// rate outranks every faster rate; within either class choose the fastest
/// admissible rate or the slowest rate above the data rate, respectively.
fn select_control_rate(
    first: LegacyRate,
    rest: impl Iterator<Item = LegacyRate>,
    data_kbps: u32,
) -> LegacyRate {
    rest.fold(first, |selected, candidate| {
        let selected_kbps = selected.kbps();
        let candidate_kbps = candidate.kbps();
        let prefer_candidate = match (candidate_kbps <= data_kbps, selected_kbps <= data_kbps) {
            (true, true) => candidate_kbps > selected_kbps,
            (false, false) => candidate_kbps < selected_kbps,
            (true, false) => true,
            (false, true) => false,
        };
        if prefer_candidate {
            candidate
        } else {
            selected
        }
    })
}

/// Whether an HT Protection mode protects one HT or HE PPDU of this width.
///
/// Nonmember and non-HT mixed modes protect every HT PPDU; 20-MHz mode
/// protects only 40-MHz PPDUs. HE PPDUs follow the HT rules, as in mt76x02
/// and rt2800, because non-HE stations cannot decode them either.
const fn ht_protects(mode: HtProtectionMode, forty_mhz: bool) -> bool {
    match mode {
        HtProtectionMode::None => false,
        HtProtectionMode::TwentyMhz => forty_mhz,
        HtProtectionMode::Nonmember | HtProtectionMode::NonHtMixed => true,
    }
}

#[cfg(test)]
mod tests;
