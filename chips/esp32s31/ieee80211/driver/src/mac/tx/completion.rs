//! The meaning of a completion event, independent of its published owner.

use oer_esp32s31_hal::types::MacTxCompletionObservation;
use oer_ieee80211_port::TxStatus;

/// Whether the vendor handler observes a publication result at this event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionObservation<T = TxStatus> {
    /// A known observation; the driver may consume its published attempt.
    Observed(T),
    /// The handler does not end the frame exchange. Retain the publication
    /// until a later result or hardware timeout; no terminal or rate sample.
    NoEffect,
}

/// Hardware contradicts the checked publication or reports no defined status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionFault {
    /// The backend published with an installation the cipher cannot use.
    SecurityKeyError,
    /// A status outside all recovered completion-dispatch branches.
    InvalidStatus { status: u8 },
}

/// Value-only input to the completion decoder. No attempt, buffer or driver
/// is borrowed or consumed. The library completion dispatcher supplies zero
/// for the vendor handlers' extra arguments; only hardware status and detail
/// belong to this decoder. Nonzero ABI arguments live in the vendor probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionRaw {
    status: u8,
    detail: u8,
}

impl CompletionRaw {
    /// Ordinary completion dispatch passes zero to both error handlers.
    ///
    /// SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxComplete +0x19a
    /// clears RTS a3; +0x1cc clears TxError a2. rom[]::lmacProcessTxComplete
    /// +0x152 clears RTS a3 too. libpp[wdev.o]::wdev_funcs_init +0x900 puts
    /// the library RTS handler into the ROM table at offset 892.
    pub const fn ordinary(raw: MacTxCompletionObservation) -> Self {
        Self {
            status: raw.status(),
            detail: raw.detail(),
        }
    }
}

/// Decode an event without owning the publication. NoEffect deliberately
/// carries no status with which a driver could finish that attempt.
/// Faults belong to the backend's existing Poisoned path, never TxOutcome.
pub const fn decode(raw: CompletionRaw) -> Result<CompletionObservation, CompletionFault> {
    use CompletionObservation::Observed;
    // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxComplete +0x188,
    // +0x1a2, +0x1b0, +0x1d0 and +0x1be dispatch success, RTS, CTS,
    // TxError and ACK respectively. Status 3 reaches wifi_log, then
    // unconditional wifi_assert(0) at +0x252 (lmac.c:1575); statuses beyond
    // the dispatch table also stop. Model that vendor stop as Poisoned.
    // SOURCE(esp32s31): libpp[pp.o]::ppTask +0x19c calls the library
    // libpp[lmac.o]::lmacProcessTxComplete directly. libpp[wdev.o]::wdev_funcs_init
    // +0x42c/+0x430 loads that same address and +0x434 installs it in the
    // dispatch table. Thus ROM table calls also execute the library copy;
    // the divergent rom[]::lmacProcessTxComplete status-3 branch is not
    // on this path. These citations register the executable provenance of
    // that divergence.
    match raw.status {
        0 => Ok(Observed(TxStatus::Success)),
        1 => rts_error(raw.detail),
        2 => Ok(Observed(TxStatus::CtsTimeout)),
        4 => tx_error(raw.detail),
        5 => Ok(Observed(TxStatus::AckTimeout)),
        status => Err(CompletionFault::InvalidStatus { status }),
    }
}

const fn rts_error(detail: u8) -> Result<CompletionObservation, CompletionFault> {
    use CompletionObservation::{NoEffect, Observed};
    match detail {
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0x4e..+0x6e accepts
        // 1, 3..5 and 0xa0..0xad and tail-calls libpp[lmac.o]::lmacProcessCollision +0x124.
        1 | 3..=5 | 0xa0..=0xad => Ok(Observed(TxStatus::Collision)),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0xe6 calls
        // libpp[lmac.o]::lmacEndFrameExchangeSequence(inst, 1, 0), the same
        // arguments as libpp[lmac.o]::lmacProcessTxseckiderr +0x26. This is a key fault.
        0xc0 => Err(CompletionFault::SecurityKeyError),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0x72..+0xaa only logs
        // every remaining detail. It does not end the publication.
        _ => Ok(NoEffect),
    }
}

const fn tx_error(detail: u8) -> Result<CompletionObservation, CompletionFault> {
    use CompletionObservation::Observed;
    match detail {
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xe2 dispatches detail 0
        // to libpp[lmac.o]::lmacProcessCtsTimeout, independently of a2.
        0 => Ok(Observed(TxStatus::CtsTimeout)),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xb2 dispatches 1 and
        // 3..5 to libpp[lmac.o]::lmacProcessCollision only when a2 == 0.
        1 | 3..=5 => Ok(Observed(TxStatus::Collision)),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0x98 calls
        // libpp[lmac.o]::lmacProcessTxseckiderr; that function's +0x26 ends FES(inst, 1, 0).
        0xc0 => Err(CompletionFault::SecurityKeyError),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xca dispatches the other
        // details to libpp[lmac.o]::lmacProcessAckTimeout; the dispatcher supplies a2 = 0.
        _ => Ok(Observed(TxStatus::AckTimeout)),
    }
}

impl<T> CompletionObservation<T> {
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> CompletionObservation<U> {
        match self {
            Self::Observed(value) => CompletionObservation::Observed(map(value)),
            Self::NoEffect => CompletionObservation::NoEffect,
        }
    }
}
