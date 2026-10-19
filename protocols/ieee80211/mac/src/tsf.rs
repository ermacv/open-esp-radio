//! The clock domain of IEEE 802.11 Timing Synchronization Functions
//! (IEEE 802.11-2020 11.1.3): the protocol time of a BSS, which beacons
//! carry and target beacon transmission times, DTIMs and TWT service periods
//! are scheduled in.

/// The clock domain of IEEE 802.11 Timing Synchronization Functions:
/// microseconds of a BSS's TSF.
pub enum Ieee80211Tsf {}

/// An instant of a TSF, without the interface whose TSF it is.
pub type TsfInstant = oer_time::RadioInstant<Ieee80211Tsf>;

/// A span of a TSF: beacon intervals, TBTT leads and TWT periods.
pub type TsfDuration = oer_time::RadioDuration<Ieee80211Tsf>;

/// The length of `units` time units (TU) of 1024 µs of a TSF.
pub const fn time_units(units: u16) -> TsfDuration {
    TsfDuration::from_micros(units as u64 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_unit_is_1024_microseconds() {
        assert_eq!(time_units(1).as_micros(), 1024);
        assert_eq!(time_units(100).as_micros(), 102_400);
        assert_eq!(time_units(u16::MAX).as_micros(), u64::from(u16::MAX) * 1024);
    }
}
