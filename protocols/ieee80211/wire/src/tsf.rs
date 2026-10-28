//! Timing Synchronization Function values on air (IEEE 802.11-2020 11.1.3):
//! the Timestamp field beacons and probe responses carry, and the time unit
//! beacon intervals and timeouts count in.

/// One time unit (TU) in microseconds.
///
/// SOURCE: IEEE Std 802.11-2020, 3.1 (definition of "time unit (TU)":
/// 1024 µs).
pub const MICROS_PER_TIME_UNIT: u64 = 1024;

/// The length of `units` time units in microseconds; total, as a `u16`
/// count of 1024 µs never exceeds `u64`.
pub const fn time_units_micros(units: u16) -> u64 {
    // `u16::MAX * 1024` is below `2^26`.
    (units as u64).wrapping_mul(MICROS_PER_TIME_UNIT)
}

/// The Timestamp field: the sender's TSF timer in microseconds when the
/// frame's first data symbol reached the antenna.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.10 (Timestamp field).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TsfTimestamp(u64);

impl TsfTimestamp {
    /// The Timestamp field's octet count.
    pub const LEN: usize = 8;

    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// The field as it appears on air (little-endian).
    pub const fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(u64::from_le_bytes(bytes))
    }

    pub const fn to_bytes(self) -> [u8; Self::LEN] {
        self.0.to_le_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_unit_is_1024_microseconds() {
        assert_eq!(time_units_micros(1), 1024);
        assert_eq!(time_units_micros(100), 102_400);
        assert_eq!(time_units_micros(u16::MAX), u64::from(u16::MAX) * 1024);
    }

    #[test]
    fn the_timestamp_field_round_trips_little_endian() {
        let field = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
        let timestamp = TsfTimestamp::from_bytes(field);
        assert_eq!(timestamp.as_micros(), 0x0102_0304_0506_0708);
        assert_eq!(timestamp.to_bytes(), field);
    }
}
