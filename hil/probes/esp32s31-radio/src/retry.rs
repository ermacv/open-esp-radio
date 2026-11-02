//! Retry-rate admission at the comparison probe's descriptor boundary.

use oer_esp32s31_ieee80211_driver::mac::{
    portable::UnsupportedTxRate,
    tx::{HtChannelWidth, TxPhyRate},
};
use oer_ieee80211_contracts_port::{PhyRate, backend::RetryCounters};
use oer_ieee80211_core_upper_mac::retry::{RateLadder, select_ordinary_retry_rate};

/// Return word of the rate-selection probe; the vendor comparison observes
/// descriptor writes and does not compare this additional refusal report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum RetryProbeStatus {
    Selected = 0,
    InvalidInitialRate = 1,
    RateUnavailable = 2,
    UnsupportedHtMcs = 3,
    UnsupportedHtBandwidth = 4,
    UnsupportedHeMcs = 5,
    UnsupportedHeSpatialStreams = 6,
    UnsupportedHeBandwidth = 7,
    UnsupportedHeDcm = 8,
}

/// Refused input or selected stage, retaining the reason and its source value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetryRateRefusal {
    InvalidInitialRate {
        code: u32,
    },
    RateUnavailable {
        initial: TxPhyRate,
        counters: RetryCounters,
    },
    UnsupportedStage {
        rate: PhyRate,
        reason: UnsupportedTxRate,
    },
}

impl RetryRateRefusal {
    pub(crate) const fn status(self) -> RetryProbeStatus {
        match self {
            Self::InvalidInitialRate { .. } => RetryProbeStatus::InvalidInitialRate,
            Self::RateUnavailable { .. } => RetryProbeStatus::RateUnavailable,
            Self::UnsupportedStage { reason, .. } => match reason {
                UnsupportedTxRate::HtMcs => RetryProbeStatus::UnsupportedHtMcs,
                UnsupportedTxRate::HtBandwidth => RetryProbeStatus::UnsupportedHtBandwidth,
                UnsupportedTxRate::HeMcs => RetryProbeStatus::UnsupportedHeMcs,
                UnsupportedTxRate::HeSpatialStreams => {
                    RetryProbeStatus::UnsupportedHeSpatialStreams
                }
                UnsupportedTxRate::HeBandwidth => RetryProbeStatus::UnsupportedHeBandwidth,
                UnsupportedTxRate::HeDcm => RetryProbeStatus::UnsupportedHeDcm,
            },
        }
    }
}

/// Admit the selected stage exactly. An unrepresentable stage is refused;
/// it neither restores the initial rate nor writes the descriptor.
pub(crate) fn select(
    initial_code: u32,
    counters: RetryCounters,
    ladder: &impl RateLadder,
) -> Result<TxPhyRate, RetryRateRefusal> {
    let initial = u8::try_from(initial_code)
        .ok()
        .and_then(|code| TxPhyRate::from_code(code, HtChannelWidth::Mhz20))
        .ok_or(RetryRateRefusal::InvalidInitialRate { code: initial_code })?;
    let rate = select_ordinary_retry_rate(initial.phy_rate(), counters, ladder)
        .ok_or(RetryRateRefusal::RateUnavailable { initial, counters })?;
    TxPhyRate::try_from(rate).map_err(|reason| RetryRateRefusal::UnsupportedStage { rate, reason })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_hal::types::MacLegacyRate;
    use oer_espressif_ieee80211_policy::retry_ladder::EspressifRetryLadder;
    use oer_ieee80211_wire::phy::{
        FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, PpduBandwidth, SpatialStreams,
    };

    struct Stage(Option<PhyRate>);
    impl RateLadder for Stage {
        fn rate(&self, _: PhyRate, _: u8) -> Option<PhyRate> {
            self.0
        }
    }

    #[test]
    fn a_representable_vendor_stage_is_admitted_exactly() {
        assert_eq!(
            select(
                u32::from(MacLegacyRate::Ofdm54M.code()),
                RetryCounters {
                    mpdu: 4,
                    short: 2,
                    long: 255
                },
                &EspressifRetryLadder,
            ),
            Ok(TxPhyRate::Legacy(MacLegacyRate::Ofdm6M))
        );
        assert_eq!(RetryProbeStatus::Selected as u32, 0);
    }

    #[test]
    fn unsupported_ht_and_he_stages_return_the_exact_rate_and_reason() {
        // Even an HT initial rate must not be substituted for an
        // unrepresentable selected stage.
        let initial = u32::from(
            TxPhyRate::try_from(PhyRate::Ht(
                HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
            ))
            .unwrap()
            .code(),
        );
        let ht =
            PhyRate::Ht(HtRate::new(HtMcs::new(8).unwrap(), PpduBandwidth::Mhz20, false).unwrap());
        let he = PhyRate::He(
            HeRate::new(
                HeMcs::new(7).unwrap(),
                SpatialStreams::new(2).unwrap(),
                PpduBandwidth::Mhz20,
                HeGiLtf::Ltf2xGi800Ns,
                FecCoding::Ldpc,
                false,
            )
            .unwrap(),
        );
        for (rate, reason, status) in [
            (
                ht,
                UnsupportedTxRate::HtMcs,
                RetryProbeStatus::UnsupportedHtMcs,
            ),
            (
                he,
                UnsupportedTxRate::HeSpatialStreams,
                RetryProbeStatus::UnsupportedHeSpatialStreams,
            ),
        ] {
            let refusal =
                select(initial, RetryCounters::default(), &Stage(Some(rate))).unwrap_err();
            assert_eq!(refusal, RetryRateRefusal::UnsupportedStage { rate, reason });
            assert_eq!(refusal.status(), status);
            assert_ne!(status, RetryProbeStatus::Selected);
        }
    }

    #[test]
    fn invalid_initial_codes_and_exhausted_legacy_series_have_distinct_refusals() {
        for code in [u32::MAX, 256, 255] {
            let refusal = select(code, RetryCounters::default(), &Stage(None)).unwrap_err();
            assert_eq!(refusal, RetryRateRefusal::InvalidInitialRate { code });
            assert_eq!(refusal.status(), RetryProbeStatus::InvalidInitialRate);
        }
        let initial = TxPhyRate::Legacy(MacLegacyRate::Ofdm54M);
        let counters = RetryCounters {
            mpdu: 32,
            short: 0,
            long: 0,
        };
        let refusal =
            select(u32::from(initial.code()), counters, &EspressifRetryLadder).unwrap_err();
        assert_eq!(
            refusal,
            RetryRateRefusal::RateUnavailable { initial, counters }
        );
        assert_eq!(refusal.status(), RetryProbeStatus::RateUnavailable);
    }
}
