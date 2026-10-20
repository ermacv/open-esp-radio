//! Portable EDCA contention: the CSMA/CA backoff draw a lower-MAC backend
//! uses for its own queues below the port (the ESP32-S31 driver's
//! `EdcaBackoffState`). The backoff does not cross the lower-MAC port. [`EdcaContention`] keeps the contention window of one
//! access category (IEEE 802.11-2020 10.23.2): it starts at `CWmin`, doubles
//! after each failed attempt of a frame up to `CWmax` and returns to
//! `CWmin` when the frame ends. The random source is injected, so a seeded
//! source makes the draws deterministic.

use oer_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};
use oer_ieee80211_wire::qos::WmmAccessCategory;

/// An AC's advertised contention and locally configured MSDU lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaAcParameters {
    /// Contention, admission-control and TXOP fields advertised on air.
    pub wmm: WmmAcParameters,
    /// Maximum retention from software admission, in 1024-us TUs.
    pub msdu_lifetime_tu: u16,
}

/// A VIF's complete EDCA table. Lifetimes are local MIB attributes, not
/// fields of the on-air WMM Parameter Element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaParameterSet {
    wmm: WmmParameterSet,
    msdu_lifetimes_tu: [u16; 4],
}

impl EdcaParameterSet {
    /// Default and largest standard MSDU retention time, in TUs.
    // SOURCE: IEEE 802.11-2020 Annex C, dot11EDCATableMSDULifetime.
    pub const DEFAULT_MSDU_LIFETIME_TU: u16 = 500;

    /// Non-AP defaults before a BSS is known.
    // SOURCE: IEEE 802.11-2020 9.4.2.28, Table 9-155 (OFDM/ERP/HT/HE).
    pub const STANDARD_DEFAULT: Self = Self::from_wmm(WmmParameterSet::STANDARD_DEFAULT);

    /// Lifetimes in ACI order: BE, BK, VI, VO. SupportedEdca validates
    /// the whole table before it can be installed.
    pub const fn new(wmm: WmmParameterSet, msdu_lifetimes_tu: [u16; 4]) -> Self {
        Self {
            wmm,
            msdu_lifetimes_tu,
        }
    }

    /// Advertised contention with the standard local lifetime for each AC.
    pub const fn from_wmm(wmm: WmmParameterSet) -> Self {
        Self::new(wmm, [Self::DEFAULT_MSDU_LIFETIME_TU; 4])
    }

    /// Advertised fields, excluding the local lifetime attributes.
    pub const fn wmm(self) -> WmmParameterSet {
        self.wmm
    }

    /// Complete record for this access category.
    pub const fn access_category(self, category: WmmAccessCategory) -> EdcaAcParameters {
        EdcaAcParameters {
            wmm: self.wmm.access_category(category),
            msdu_lifetime_tu: *category.select(&self.msdu_lifetimes_tu),
        }
    }
}

impl Default for EdcaParameterSet {
    fn default() -> Self {
        Self::STANDARD_DEFAULT
    }
}

/// The largest contention-window exponent: the four-bit ECW fields of the
/// EDCA Parameter Set.
pub const MAX_CW_EXPONENT: u8 = 15;

/// A source of uniformly distributed random words for backoff draws.
pub trait BackoffEntropy {
    fn next_u32(&mut self) -> u32;
}

impl<F: FnMut() -> u32> BackoffEntropy for F {
    fn next_u32(&mut self) -> u32 {
        self()
    }
}

/// The contention window of one access category and the retry count of its
/// current frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaContention {
    cw_min_exponent: u8,
    cw_max_exponent: u8,
    retries: u8,
}

impl EdcaContention {
    /// The window of `ECWmin` and `ECWmax`, each clamped to
    /// [`MAX_CW_EXPONENT`]; a maximum below the minimum is raised to it.
    pub const fn new(cw_min_exponent: u8, cw_max_exponent: u8) -> Self {
        let cw_min_exponent = if cw_min_exponent > MAX_CW_EXPONENT {
            MAX_CW_EXPONENT
        } else {
            cw_min_exponent
        };
        let cw_max_exponent = if cw_max_exponent > MAX_CW_EXPONENT {
            MAX_CW_EXPONENT
        } else if cw_max_exponent < cw_min_exponent {
            cw_min_exponent
        } else {
            cw_max_exponent
        };
        Self {
            cw_min_exponent,
            cw_max_exponent,
            retries: 0,
        }
    }

    /// The window an AC Parameter Record advertises.
    pub const fn from_wmm(parameters: WmmAcParameters) -> Self {
        Self::new(parameters.ecw_min, parameters.ecw_max)
    }

    /// The exponent `n` of the current window `CW = 2^n - 1`.
    pub const fn cw_exponent(self) -> u8 {
        let raised = self.cw_min_exponent.saturating_add(self.retries);
        if raised > self.cw_max_exponent {
            self.cw_max_exponent
        } else {
            raised
        }
    }

    /// The current window `CW` in slots.
    pub const fn cw(self) -> u16 {
        ((1_u32 << self.cw_exponent()) - 1) as u16
    }

    /// Failed attempts of the current frame.
    pub const fn retries(self) -> u8 {
        self.retries
    }

    /// Draw the backoff slots of the next attempt uniformly from `0..=CW`.
    pub fn draw(self, entropy: &mut impl BackoffEntropy) -> u16 {
        // CW is 2^n - 1 below 2^16: its mask keeps only low bits.
        let [b0, b1, ..] = entropy.next_u32().to_le_bytes();
        u16::from_le_bytes([b0, b1]) & self.cw()
    }

    /// Double the window after a failed attempt of the frame (an ACK or CTS
    /// timeout or a collision), until it reaches `CWmax`.
    pub fn record_failure(&mut self) {
        if self.cw_exponent() < self.cw_max_exponent {
            self.retries += 1;
        }
    }

    /// Return to `CWmin` when the frame ends: after its success, or after
    /// the caller's retry limit discarded it.
    pub fn reset(&mut self) {
        self.retries = 0;
    }

    /// Install a new `ECWmin` and `ECWmax`, clamped as by [`Self::new`],
    /// while a frame may be in progress: the current window is kept where it
    /// still lies inside the new bounds, lowered to a smaller `CWmax` and
    /// raised to a larger `CWmin`.
    pub fn reconfigure(&mut self, cw_min_exponent: u8, cw_max_exponent: u8) {
        let current = self.cw_exponent();
        let bounds = Self::new(cw_min_exponent, cw_max_exponent);
        let current = current.clamp(bounds.cw_min_exponent, bounds.cw_max_exponent);
        *self = Self {
            retries: current - bounds.cw_min_exponent,
            ..bounds
        };
    }

    /// The exponent of `CWmin`.
    pub const fn cw_min_exponent(self) -> u8 {
        self.cw_min_exponent
    }

    /// The exponent of `CWmax`.
    pub const fn cw_max_exponent(self) -> u8 {
        self.cw_max_exponent
    }
}

/// The backoff of attempt `retries + 1` of a frame under `parameters`,
/// without keeping state: the window is `CWmin` doubled `retries` times,
/// bounded by `CWmax`.
pub fn draw_backoff(
    parameters: WmmAcParameters,
    retries: u8,
    entropy: &mut impl BackoffEntropy,
) -> u16 {
    let mut contention = EdcaContention::from_wmm(parameters);
    for _ in 0..retries {
        contention.record_failure();
    }
    contention.draw(entropy)
}

#[cfg(test)]
mod tests;
