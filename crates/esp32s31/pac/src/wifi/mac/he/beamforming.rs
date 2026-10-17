//! Typed HE beamforming report-rate configuration.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

/// Four-byte ER-SU ACK-rate image selected by the recovered rate policy.
///
/// The byte values are hardware rate encodings, not booleans. Keeping the
/// choice typed prevents MAC policy from manufacturing other undocumented
/// images for this register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeErSuAckRateProfile {
    /// Ordinary report response selected by a zero HAL argument.
    Ordinary,
    /// Extended-range report response selected by a nonzero HAL argument.
    ExtendedRange,
}

impl MacHeErSuAckRateProfile {
    pub const fn encoded_byte(self) -> u8 {
        match self {
            Self::Ordinary => 0x80,
            Self::ExtendedRange => 0xa0,
        }
    }
}

/// Invalid input to the recovered `hal_he_set_bf_report_rate` transform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeBeamformingReportProfileError {
    /// The hardware signaling-mode field is two bits wide.
    SignalMode(u8),
    /// Mode zero accepts a direct five-bit rate; nonzero modes accept the two
    /// recovered ten-rate descriptor ranges `0x10..=0x19` and `0x1a..=0x23`.
    RateCode { signal_mode: u8, rate_code: u16 },
}

/// One profile replicated into the BPSK, QPSK and 16-QAM report selectors.
///
/// Construction performs the exact rate normalization from complete
/// `hal_he_set_bf_report_rate`, but rejects inputs for which the blob would
/// rely on unsigned wrap and field truncation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeBeamformingReportProfile {
    signal_mode: u8,
    normalized_rate: u8,
    dcm: bool,
    extended_range_single_user: bool,
}

impl MacHeBeamformingReportProfile {
    /// Build a bounded profile from the four HAL arguments.
    pub const fn from_hal_arguments(
        signal_mode: u8,
        rate_code: u16,
        dcm: bool,
        extended_range_single_user: bool,
    ) -> Result<Self, MacHeBeamformingReportProfileError> {
        if signal_mode > 3 {
            return Err(MacHeBeamformingReportProfileError::SignalMode(signal_mode));
        }

        let normalized_rate = if signal_mode == 0 {
            if rate_code > 0x1f {
                return Err(MacHeBeamformingReportProfileError::RateCode {
                    signal_mode,
                    rate_code,
                });
            }
            rate_code as u8
        } else if rate_code >= 0x10 && rate_code <= 0x19 {
            (rate_code - 0x10) as u8
        } else if rate_code >= 0x1a && rate_code <= 0x23 {
            (rate_code - 0x1a) as u8
        } else {
            return Err(MacHeBeamformingReportProfileError::RateCode {
                signal_mode,
                rate_code,
            });
        };

        Ok(Self {
            signal_mode,
            normalized_rate,
            dcm,
            extended_range_single_user,
        })
    }

    pub const fn signal_mode(self) -> u8 {
        self.signal_mode
    }

    pub const fn normalized_rate(self) -> u8 {
        self.normalized_rate
    }

    pub const fn dcm(self) -> bool {
        self.dcm
    }

    pub const fn extended_range_single_user(self) -> bool {
        self.extended_range_single_user
    }
}

impl WifiRadioRegisters {
    /// Publish one report profile to 16-QAM, QPSK and BPSK selectors.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `libpp[hal_mac_ctl.o]::hal_he_set_bf_report_rate`, `libpp.a[hal_debug.o]`
    /// `dbg_read_bfr_rate`, and `libpp.a[trc.o]`
    /// `libpp[trc.o]::trc_set_bf_report_rate`. The three fresh-read RMWs and their
    /// high-to-low order are preserved.
    pub fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        let report = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_prefix
            .bf_report_rate();

        report.modify(|_, w| {
            w.qam16_rate()
                .set(profile.normalized_rate)
                .qam16_signal_mode()
                .set(profile.signal_mode)
                .qam16_dcm()
                .bit(profile.dcm)
                .qam16_ersu()
                .bit(profile.extended_range_single_user)
        });
        report.modify(|_, w| {
            w.qpsk_rate()
                .set(profile.normalized_rate)
                .qpsk_signal_mode()
                .set(profile.signal_mode)
                .qpsk_dcm()
                .bit(profile.dcm)
                .qpsk_ersu()
                .bit(profile.extended_range_single_user)
        });
        report.modify(|_, w| {
            w.bpsk_rate()
                .set(profile.normalized_rate)
                .bpsk_signal_mode()
                .set(profile.signal_mode)
                .bpsk_dcm()
                .bit(profile.dcm)
                .bpsk_ersu()
                .bit(profile.extended_range_single_user)
        });
    }

    /// Publish the ER-SU ACK-rate profile selected alongside the report rate.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `libpp[hal_mac_ctl.o]::hal_he_set_ersu_ack_rate`, size `0x4e`, reached from complete
    /// `libpp[trc.o]::trc_set_bf_report_rate`, size `0x52`.
    /// The four fresh-read byte RMWs and their low-to-high order are
    /// instruction-exact.
    pub fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        let encoded = profile.encoded_byte();
        let ack = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .ersu_ack_rate();

        ack.modify(|_, w| w.rate_0().set(encoded));
        ack.modify(|_, w| w.rate_1().set(encoded));
        ack.modify(|_, w| w.rate_2().set(encoded));
        ack.modify(|_, w| w.rate_3().set(encoded));
    }
}

#[cfg(test)]
mod tests;
