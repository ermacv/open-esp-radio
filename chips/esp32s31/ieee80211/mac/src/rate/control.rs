//! The ESP32-S31 side of the Espressif rate control.
//!
//! The controller itself, its schedule selection and adaptation, is the
//! family's policy (`oer-espressif-ieee80211-policy::rate_control`). This
//! module turns its schedules into S31 rates ([`StaTxRatePolicy`], with the
//! certification overrides) and programs its beamforming report rate into
//! the MAC.

use crate::{
    rate::schedule::RateScheduleRef,
    rx::HeGuardIntervalAndLtf,
    tx::{
        HeDcmRate, HeMcs, HeRate, HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate,
        TxCompletion, TxPhyRate,
    },
};

use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacHal,
    owner::RadioAccess,
    types::{
        MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeErSuAckRateProfile,
    },
};

use {oer_ieee80211_wire::he::HeDcmConstellation, oer_ieee80211_wire::station::association::PhyMode};

use oer_espressif_ieee80211_policy::rate_control::{
    BeamformingReportRate, StaRateControlAssociation,
};

/// Ordered PAC leaves used by the association rate-control transition.
pub trait BeamformingReportHardware {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile);
    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile);
}

impl BeamformingReportHardware for WifiMacHal<'_> {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        WifiMacHal::set_he_beamforming_report_profile(self, profile);
    }

    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        WifiMacHal::set_he_ersu_ack_rate_profile(self, profile);
    }
}

impl BeamformingReportHardware for RadioAccess {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        BeamformingReportHardware::set_he_beamforming_report_profile(
            &mut self.wifi_mac_hal(),
            profile,
        );
    }

    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        BeamformingReportHardware::set_he_ersu_ack_rate_profile(&mut self.wifi_mac_hal(), profile);
    }
}

/// Publish both hardware leaves of complete `trc_set_bf_report_rate`.
///
/// The mutable borrow is the Rust ownership boundary for the radio
/// registers: while this call runs, no second safe owner can interleave
/// writes between the report profile and its matching ACK profile.
///
/// SOURCE: complete pinned `libpp.a[trc.o]`
/// `libpp[trc.o]::trc_set_bf_report_rate`, size `0x52`, and its complete
/// `libpp.a[hal_mac_ctl.o]` children
/// `libpp[hal_mac_ctl.o]::hal_he_set_bf_report_rate` and `libpp[hal_mac_ctl.o]::hal_he_set_ersu_ack_rate`.
pub fn program_beamforming_report<H: BeamformingReportHardware>(
    rate: BeamformingReportRate,
    hardware: &mut H,
) -> Result<(), MacHeBeamformingReportProfileError> {
    let profile = MacHeBeamformingReportProfile::from_hal_arguments(
        rate.signal_mode(),
        rate.rate_code(),
        rate.dcm(),
        rate.extended_range_single_user(),
    )?;
    let ack_profile = if rate.extended_range_ack() {
        MacHeErSuAckRateProfile::ExtendedRange
    } else {
        MacHeErSuAckRateProfile::Ordinary
    };

    // Preserve the complete blob's transaction order: three report-rate
    // RMWs first, then the four ER-SU ACK-rate RMWs.
    hardware.set_he_beamforming_report_profile(profile);
    hardware.set_he_ersu_ack_rate_profile(ack_profile);
    Ok(())
}

/// Runtime-independent inputs that turn one recovered rate schedule into the
/// exact PHY format published by the S31 TX owner.
///
/// The rate-control arena owns the current rate byte, while association state
/// owns channel width, the peer-qualified HE LTF choice and LDPC capability.
/// Keeping the join here prevents applications from interpreting the
/// overlapping Dot11N/Dot11Ax byte domains themselves.
///
/// The HT and HE MCS/guard-interval overrides are explicit certification/test
/// controls. Ordinary operation leaves them `None` and retains the values
/// selected by the recovered schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxRatePolicy {
    pub association_phy: PhyMode,
    pub high_throughput_enabled: bool,
    pub fallback_legacy_rate: LegacyRate,
    pub fallback_ht_mcs: HtMcs,
    pub fallback_ht_guard_interval: HtGuardInterval,
    /// Fixed typed MCS for certification; `None` keeps schedule ownership.
    pub ht_mcs_override: Option<HtMcs>,
    pub ht_guard_interval_override: Option<HtGuardInterval>,
    /// Fixed standard HE-SU MCS for certification; `None` keeps the Dot11Ax
    /// schedule selection.
    pub he_mcs_override: Option<HeMcs>,
    /// Fixed HE-SU GI/LTF selector for certification. DCM and trigger-based
    /// profiles use their own typed rate owners instead of this SU override.
    pub he_guard_interval_and_ltf_override: Option<HeGuardIntervalAndLtf>,
    /// Exact typed DCM certification rate. It takes precedence over the
    /// ordinary HE MCS/GI overrides only when the peer capability admits it.
    pub he_dcm_override: Option<HeDcmRate>,
    pub he_800ns_gi_ltf: HeGuardIntervalAndLtf,
    /// Capability for the exact HT width selected by `association_phy`.
    pub peer_supports_ht_short_guard_interval: bool,
    pub peer_supports_ldpc: bool,
    pub peer_dcm_receive: HeDcmConstellation,
}

impl StaTxRatePolicy {
    const fn ht_width(self) -> Option<HtChannelWidth> {
        match self.association_phy {
            PhyMode::Ht40 => Some(HtChannelWidth::Mhz40),
            PhyMode::Ht20 | PhyMode::He20 => Some(HtChannelWidth::Mhz20),
            PhyMode::Legacy => None,
        }
    }

    const fn qualify_ht_guard_interval(self, requested: HtGuardInterval) -> HtGuardInterval {
        match requested {
            HtGuardInterval::Short400Ns if !self.peer_supports_ht_short_guard_interval => {
                HtGuardInterval::Long800Ns
            }
            guard_interval => guard_interval,
        }
    }

    /// Conservative data rate used when no owned schedule representation can
    /// be published, including the still-proprietary Long Range arena.
    pub const fn fallback_rate(self) -> TxPhyRate {
        if !self.high_throughput_enabled {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        }
        let Some(channel_width) = self.ht_width() else {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        };
        let mcs = match self.ht_mcs_override {
            Some(mcs) => mcs,
            None => self.fallback_ht_mcs,
        };
        let guard_interval =
            self.qualify_ht_guard_interval(match self.ht_guard_interval_override {
                Some(guard_interval) => guard_interval,
                None => self.fallback_ht_guard_interval,
            });
        TxPhyRate::Ht(HtRate::new(mcs, guard_interval, channel_width))
    }

    pub const fn he_dcm_override_is_supported(self) -> bool {
        match self.he_dcm_override {
            Some(rate) => rate.is_supported_by(self.peer_dcm_receive, self.peer_supports_ldpc),
            None => true,
        }
    }

    /// Decode one complete Rust-owned schedule and apply negotiated format
    /// capabilities without exposing vendor rate bytes to the application.
    pub fn rate_for_schedule(self, schedule: RateScheduleRef) -> TxPhyRate {
        if !self.high_throughput_enabled {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        }
        let Some(ht_width) = self.ht_width() else {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        };
        let Some(rate) =
            TxPhyRate::from_rate_control_schedule(schedule, ht_width, self.he_800ns_gi_ltf)
        else {
            return self.fallback_rate();
        };
        let rate = match rate {
            TxPhyRate::Ht(rate) => TxPhyRate::Ht(HtRate::new(
                self.ht_mcs_override.unwrap_or(rate.mcs),
                self.qualify_ht_guard_interval(
                    self.ht_guard_interval_override
                        .unwrap_or(rate.guard_interval),
                ),
                rate.channel_width,
            )),
            TxPhyRate::He(rate) => match self.he_dcm_override {
                Some(dcm) if self.he_dcm_override_is_supported() => TxPhyRate::He(dcm.rate()),
                _ => TxPhyRate::He(HeRate::new(
                    self.he_mcs_override.unwrap_or(rate.mcs()),
                    self.he_guard_interval_and_ltf_override
                        .unwrap_or(rate.guard_interval_and_ltf()),
                )),
            },
            rate => rate,
        };
        match rate {
            TxPhyRate::He(rate) if self.peer_supports_ldpc && !rate.is_dcm() => {
                TxPhyRate::He(HeRate::ldpc(rate.mcs(), rate.guard_interval_and_ltf()))
            }
            _ => rate,
        }
    }
}

/// What the S31 transmit owner reads from an association's rate control:
/// its schedules as S31 rates, its ACK-SNR samples and its one hardware
/// side effect.
pub trait StaRateControlTx {
    /// PHY rate selected by the ordinary per-peer schedule.
    fn tx_rate(&self, policy: StaTxRatePolicy) -> TxPhyRate;

    /// PHY rate selected by the independent A-MPDU schedule when available.
    fn ampdu_tx_rate(&self, policy: StaTxRatePolicy) -> TxPhyRate;

    /// Consume the ACK-SNR sample, if any, from one typed TX completion.
    ///
    /// Failed completions are ignored by [`TxCompletion::ack_snr_sample`], so
    /// callers cannot accidentally feed timeout metadata into the filter.
    fn observe_tx_completion(&mut self, completion: TxCompletion);

    /// Reproduce the sole hardware side effect of the association
    /// transition.
    fn program_hardware<H: BeamformingReportHardware>(
        &self,
        hardware: &mut H,
    ) -> Result<(), MacHeBeamformingReportProfileError>;
}

impl StaRateControlTx for StaRateControlAssociation {
    fn tx_rate(&self, policy: StaTxRatePolicy) -> TxPhyRate {
        policy.rate_for_schedule(self.current_schedule())
    }

    fn ampdu_tx_rate(&self, policy: StaTxRatePolicy) -> TxPhyRate {
        policy.rate_for_schedule(
            self.current_ampdu_schedule()
                .unwrap_or_else(|| self.current_schedule()),
        )
    }

    fn observe_tx_completion(&mut self, completion: TxCompletion) {
        if let Some(sample) = completion.ack_snr_sample() {
            self.update_ack_snr(sample);
        }
    }

    fn program_hardware<H: BeamformingReportHardware>(
        &self,
        hardware: &mut H,
    ) -> Result<(), MacHeBeamformingReportProfileError> {
        program_beamforming_report(self.beamforming_report(), hardware)
    }
}

#[cfg(test)]
mod tests;
