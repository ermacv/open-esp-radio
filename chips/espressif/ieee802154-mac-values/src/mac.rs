// SPDX-License-Identifier: Apache-2.0
//! Checked ACK timeout, receive status, diagnostic counters and ETM routes.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver` and its public MAC low-level vocabulary
//! (Copyright Espressif Systems (Shanghai) Co Ltd, Apache-2.0)
//! and modified by Vladislav Ermachenkov; see the root `NOTICE`.

use crate::{Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode};

/// Hardware quantum used by the source-confirmed vendor ACK-timeout conversion.
pub const IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS: u32 = 16;

/// Largest microsecond request that fits the complete sixteen-bit timeout field.
pub const IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS: u32 =
    u16::MAX as u32 * IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS;

/// Checked ACK timeout represented in complete hardware field units.
///
/// The vendor conversion is `ceil(microseconds / 16)`. Construction from
/// microseconds rejects values that need more than sixteen field bits; it
/// never truncates or wraps. [`Self::from_units`] also represents every field
/// image, including zero, without inventing a narrower semantic domain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154AckTimeout {
    units: u16,
}

/// A requested ACK timeout that cannot be represented by the hardware field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154AckTimeoutError {
    attempted_microseconds: u32,
}

impl Ieee802154AckTimeoutError {
    /// Return the rejected duration.
    pub const fn attempted_microseconds(self) -> u32 {
        self.attempted_microseconds
    }
}

impl Ieee802154AckTimeout {
    /// Construct any of the complete sixteen-bit hardware field values.
    pub const fn from_units(units: u16) -> Self {
        Self { units }
    }

    /// Convert microseconds with the source-confirmed `(micros + 15) / 16`
    /// rule, expressed without an overflowing addition.
    pub const fn from_microseconds(microseconds: u32) -> Result<Self, Ieee802154AckTimeoutError> {
        let units = microseconds.div_ceil(IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS);

        if units <= u16::MAX as u32 {
            Ok(Self {
                units: units as u16,
            })
        } else {
            Err(Ieee802154AckTimeoutError {
                attempted_microseconds: microseconds,
            })
        }
    }

    /// Return the complete sixteen-bit field value.
    pub const fn units(self) -> u16 {
        self.units
    }

    /// Return the effective, upward-quantized timeout in microseconds.
    pub const fn effective_microseconds(self) -> u32 {
        self.units as u32 * IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS
    }
}

#[cfg(test)]
mod ack_timeout_tests;

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

/// One of the two MAC timers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154Timer {
    /// TIMER0: ACK watchdog and timed transmit.
    Timer0,
    /// TIMER1: timed receive.
    Timer1,
}
