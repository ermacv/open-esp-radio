//! The model's contention window of one access category.
//!
//! The window starts at `CWmin`, doubles after each failed attempt of a
//! frame up to `CWmax` and returns to `CWmin` when the frame ends
//! (IEEE Std 802.11-2020, 10.23.2.2). The model draws its backoff from an
//! injected word, so its draws are deterministic.

use oer_ieee80211_wire::extensions::wmm::WmmAcParameters;

/// The four-bit ECW fields of the EDCA Parameter Set bound every exponent.
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.28, Figure 9-354 (ECWmin/ECWmax).
const MAX_EXPONENT: u8 = 15;

/// The contention window of one model queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModelWindow {
    minimum: u8,
    maximum: u8,
    current: u8,
}

impl ModelWindow {
    /// The window an AC Parameter Record advertises, at `CWmin`.
    pub(crate) fn from_wmm(parameters: WmmAcParameters) -> Self {
        let (minimum, maximum) = bounds(parameters.ecw_min.get(), parameters.ecw_max.get());
        Self {
            minimum,
            maximum,
            current: minimum,
        }
    }

    /// Install new bounds, keeping the current exponent inside them.
    pub(crate) fn reconfigure(&mut self, ecw_min: u8, ecw_max: u8) {
        let (minimum, maximum) = bounds(ecw_min, ecw_max);
        self.minimum = minimum;
        self.maximum = maximum;
        self.current = self.current.max(minimum).min(maximum);
    }

    /// Return to `CWmin` when the frame ends.
    pub(crate) fn reset(&mut self) {
        self.current = self.minimum;
    }

    /// Double the window after a failed attempt, up to `CWmax`.
    pub(crate) fn record_failure(&mut self) {
        self.current = self.current.saturating_add(1).min(self.maximum);
    }

    /// The current window `CW = 2^n - 1`, in slots.
    pub(crate) fn cw(self) -> u16 {
        u16::MAX
            .checked_shr(u32::from(16_u8.saturating_sub(self.current)))
            .unwrap_or(0)
    }

    /// A backoff uniformly drawn from `0..=CW` with the low bits of `word`.
    pub(crate) fn draw(self, word: u32) -> u16 {
        let [low, high, ..] = word.to_le_bytes();
        u16::from_le_bytes([low, high]) & self.cw()
    }
}

/// Exponents clamped to four bits, with a maximum no smaller than the minimum.
fn bounds(ecw_min: u8, ecw_max: u8) -> (u8, u8) {
    let minimum = ecw_min.min(MAX_EXPONENT);
    (minimum, ecw_max.min(MAX_EXPONENT).max(minimum))
}

#[cfg(test)]
mod tests;
