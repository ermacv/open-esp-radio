//! Saturating diagnostics for the station and its service loop.

/// A diagnostic count. Exhaustion keeps the maximum and never changes
/// protocol state, operation identity, or admission capacity.
///
/// Diagnostics belong to the station and its service, outside the lower-MAC
/// port contract:
///
/// ```compile_fail,E0432
/// use oer_ieee80211_lower_mac::Counter;
/// let mut count = Counter::ZERO;
/// count.increment();
/// ```
///
/// ```
/// use oer_ieee80211_sta::Counter;
/// let mut count = Counter::ZERO;
/// count.increment();
/// assert_eq!(count.get(), 1);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counter(u32);

impl Counter {
    /// No events observed.
    pub const ZERO: Self = Self(0);

    /// Count one event.
    pub fn increment(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    /// Combined diagnostic counts, with the same saturation policy.
    pub const fn combined(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Snapshot for reporting.
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturation_never_wraps_a_diagnostic_count() {
        let mut count = Counter(u32::MAX - 1);
        count.increment();
        count.increment();
        assert_eq!(count.get(), u32::MAX);
        assert_eq!(count.combined(count).get(), u32::MAX);
    }
}
