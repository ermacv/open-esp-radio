//! Engine-specific timeout, receive status, diagnostic counters and ETM routes.
//!
//! Shared semantic MAC values are defined in
//! `oer_espressif_ieee802154_mac_values`.

use oer_espressif_ieee802154_mac_values::{
    Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode,
};

/// An ACK timeout rounded up to sixteen-microsecond units and checked
/// against the hardware's sixteen-bit field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckTimeout(u16);

/// The rounded ACK timeout exceeds the hardware field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckTimeoutTooLarge;

impl AckTimeout {
    /// Round up to sixteen microseconds, refusing an unrepresentable value.
    pub const fn new(microseconds: u32) -> Result<Self, AckTimeoutTooLarge> {
        let units = microseconds.div_ceil(16);
        if units > u16::MAX as u32 {
            Err(AckTimeoutTooLarge)
        } else {
            Ok(Self(units as u16))
        }
    }

    /// The checked value in hardware units.
    pub const fn units(self) -> u16 {
        self.0
    }
}

#[cfg(test)]
mod ack_timeout_tests {
    use super::{AckTimeout, AckTimeoutTooLarge};

    #[test]
    fn rounds_up_without_wrapping_at_the_field_boundary() {
        for (micros, units) in [
            (0, 0),
            (1, 1),
            (16, 1),
            (17, 2),
            (200, 13),
            (1_048_560, 65_535),
        ] {
            assert_eq!(
                AckTimeout::new(micros).expect("representable").units(),
                units
            );
        }
        for micros in [1_048_561, 1_048_575, 1_048_576, u32::MAX] {
            assert_eq!(AckTimeout::new(micros), Err(AckTimeoutTooLarge));
        }
    }
}

/// Complete `RX_STATUS` observation (`ieee802154_ll_get_rx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStatus {
    filter_fail_reason: u8,
    abort_reason: Ieee802154RxAbortReasonObservation,
    state: Ieee802154RxStateCode,
    preamble_match: bool,
    sfd_match: bool,
}

impl Ieee802154RxStatus {
    /// One observation, as a register model reports it. Observations grant
    /// no write authority.
    pub const fn new(
        filter_fail_reason: u8,
        abort_reason: Ieee802154RxAbortReasonObservation,
        state: Ieee802154RxStateCode,
        preamble_match: bool,
        sfd_match: bool,
    ) -> Self {
        Self {
            filter_fail_reason,
            abort_reason,
            state,
            preamble_match,
            sfd_match,
        }
    }

    /// Raw four-bit filter-failure reason; no values are classified.
    pub const fn filter_fail_reason(&self) -> u8 {
        self.filter_fail_reason
    }

    /// Receive-abort reason of the last abort.
    pub const fn abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        self.abort_reason
    }

    /// Receiver state code.
    pub const fn state(&self) -> Ieee802154RxStateCode {
        self.state
    }

    /// `ieee802154_ll_is_current_rx_frame`: the receiver has passed the SFD
    /// of a frame.
    pub const fn frame_in_progress(&self) -> bool {
        self.state.is_after_receive_sfd()
    }

    /// Whether the preamble matched.
    pub const fn preamble_match(&self) -> bool {
        self.preamble_match
    }

    /// Whether the SFD matched.
    pub const fn sfd_match(&self) -> bool {
        self.sfd_match
    }
}

/// One diagnostic counter of the public common LL (`ieee802154_ll_get_*_cnt`).
///
/// Counters a single chip's LL adds, such as the ESP32-S31 filter and
/// preamble counters, stay with that chip's HAL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154DebugCounter {
    /// SFD timeouts.
    SfdTimeout,
    /// CRC failures.
    CrcError,
    /// Energy-detection aborts.
    EdAbort,
    /// CCA failures.
    CcaFail,
    /// Receive filter failures.
    RxFilterFail,
    /// Receptions without detected RSS.
    NoRssDetect,
    /// Receptions interrupted by coexistence.
    RxAbortCoex,
    /// Receive restarts.
    RxRestart,
    /// ACK transmissions interrupted by coexistence.
    TxAckAbortCoex,
    /// Energy scans interrupted by coexistence.
    EdScanBreakCoex,
    /// ACK receptions interrupted by coexistence.
    RxAckAbortCoex,
    /// ACK reception timeouts.
    RxAckTimeout,
    /// Transmissions interrupted by coexistence.
    TxBreakCoex,
    /// Transmit security failures.
    TxSecurityError,
    /// CCA busy results.
    CcaBusy,
}

/// Modem ETM channel owned by the IEEE 802.15.4 driver.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EtmChannel {
    /// `IEEE802154_ETM_CHANNEL0`, used for timed transmit.
    Channel0,
    /// `IEEE802154_ETM_CHANNEL1`, used for timed receive.
    Channel1,
}

/// Event-to-task route the public driver programs on its ETM channels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EtmRoute {
    /// Channel zero: `ETM_EVENT_TIMER0_OVERFLOW` to `ETM_TASK_TX_START`.
    Timer0ToTxStart,
    /// Channel zero: `ETM_EVENT_TIMER0_OVERFLOW` to `ETM_TASK_ED_TRIG_TX`,
    /// a clear-channel assessment followed by transmit.
    Timer0ToCcaTx,
    /// Channel one: `ETM_EVENT_TIMER1_OVERFLOW` to `ETM_TASK_RX_START`.
    Timer1ToRxStart,
}

impl Ieee802154EtmRoute {
    /// The channel the public driver programs for this route.
    pub const fn channel(self) -> Ieee802154EtmChannel {
        match self {
            Self::Timer0ToTxStart | Self::Timer0ToCcaTx => Ieee802154EtmChannel::Channel0,
            Self::Timer1ToRxStart => Ieee802154EtmChannel::Channel1,
        }
    }
}
