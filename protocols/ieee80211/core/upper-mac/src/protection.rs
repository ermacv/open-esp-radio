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

use oer_ieee80211_wire::{
    phy::{DsssPreamble, HeRate, LegacyRate, PhyRate, PpduBandwidth},
    protection::HtProtectionMode,
};

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

mod bss;
pub use bss::{
    BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold, ProtectedPpdu,
    RtsLengthThreshold, TxProtectionDecision, TxProtectionReasons, TxPsdu, TxReceiver,
};
use oer_ieee80211_contracts_port::Protection;

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
                .is_some_and(|threshold| length > u32::from(threshold.bytes())),
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
            Protection::RtsCts {
                rate: self.control_rate(ppdu.rate),
            }
        } else if erp || ht_protected {
            Protection::CtsToSelf {
                rate: self.control_rate(ppdu.rate),
            }
        } else {
            Protection::None
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
