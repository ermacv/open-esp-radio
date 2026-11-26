//! Timing Synchronization Function time on air (IEEE 802.11-2020 11.1.3):
//! the clock domain of a BSS's TSF, the Timestamp field beacons and probe
//! responses carry, and the time unit beacon intervals and timeouts count in.

use oer_portable_time::{DomainDuration, DomainInstant};

/// The clock domain of IEEE 802.11 Timing Synchronization Functions:
/// microseconds of a BSS's TSF (IEEE Std 802.11-2020 11.1.3), the protocol
/// time beacons carry and target beacon transmission times, DTIMs and TWT
/// service periods are scheduled in.
pub enum Ieee80211Tsf {}

/// An instant of a TSF, without the interface whose TSF it is.
pub type TsfInstant = DomainInstant<Ieee80211Tsf>;

/// A span of a TSF: beacon intervals, TBTT leads and TWT periods.
pub type TsfDuration = DomainDuration<Ieee80211Tsf>;

/// One time unit (TU) in microseconds.
///
/// SOURCE: IEEE Std 802.11-2020, 3.1 (definition of "time unit (TU)":
/// 1024 µs).
const MICROS_PER_TIME_UNIT: u64 = 1024;

/// A count of time units (TU, 1024 µs) in a 16-bit field: a Beacon Interval
/// (IEEE Std 802.11-2020 9.4.1.3) or a Block Ack Timeout Value (9.4.1.15).
/// The field says how long, not on which clock: its reader names the
/// domain of the span it takes ([`Self::span`]).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TimeUnits(u16);

impl TimeUnits {
    /// No time.
    pub const ZERO: Self = Self(0);

    /// `units` time units.
    pub const fn new(units: u16) -> Self {
        Self(units)
    }

    /// The count of time units.
    pub const fn get(self) -> u16 {
        self.0
    }

    /// The span of the domain `D` these units last: every count fits, as
    /// `u16::MAX * 1024` is below `2^26`.
    pub const fn span<D>(self) -> DomainDuration<D> {
        DomainDuration::from_micros((self.0 as u64).wrapping_mul(MICROS_PER_TIME_UNIT))
    }
}

/// A count of time units in the 32-bit Timeout Interval Value of an
/// association comeback time (IEEE Std 802.11-2020 9.4.2.48, Timeout
/// Interval Type 3): how long the access point asks the station to wait.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ComebackTime(u32);

impl ComebackTime {
    /// `units` time units.
    pub const fn new(units: u32) -> Self {
        Self(units)
    }

    /// The count of time units.
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The span of the domain `D` the comeback lasts: every count fits, as
    /// `u32::MAX * 1024` is below `2^42`.
    pub const fn span<D>(self) -> DomainDuration<D> {
        DomainDuration::from_micros((self.0 as u64).wrapping_mul(MICROS_PER_TIME_UNIT))
    }
}

/// The Timestamp field's octet count.
pub const TIMESTAMP_LEN: usize = 8;

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.10 (Timestamp field): the
    /// sender's TSF timer in microseconds when the frame's first data
    /// symbol reached the antenna.
    pub struct TimestampField[8] (little) {
        pub micros, with_micros: {u64} = 0..64;
    }
}

impl TimestampField {
    /// The TSF instant the field names.
    pub const fn instant(self) -> TsfInstant {
        TsfInstant::from_micros(self.micros())
    }

    /// The field naming `at`.
    pub const fn of(at: TsfInstant) -> Self {
        Self::from_bytes([0; TIMESTAMP_LEN]).with_micros(at.as_micros())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_unit_is_1024_microseconds_of_the_readers_domain() {
        enum Station {}
        assert_eq!(TimeUnits::new(1).span::<Ieee80211Tsf>().as_micros(), 1024);
        assert_eq!(TimeUnits::new(100).span::<Station>().as_micros(), 102_400);
        assert_eq!(
            TimeUnits::new(u16::MAX).span::<Station>().as_micros(),
            u64::from(u16::MAX) * 1024
        );
        assert_eq!(TimeUnits::ZERO.get(), 0);
        assert_eq!(
            ComebackTime::new(u32::MAX).span::<Station>().as_micros(),
            u64::from(u32::MAX) * 1024
        );
        assert_eq!(ComebackTime::new(3).get(), 3);
    }

    #[test]
    fn the_timestamp_field_round_trips_little_endian() {
        let field = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
        let timestamp = TimestampField::from_bytes(field).instant();
        assert_eq!(timestamp.as_micros(), 0x0102_0304_0506_0708);
        assert_eq!(TimestampField::of(timestamp).to_bytes(), field);
    }
}
