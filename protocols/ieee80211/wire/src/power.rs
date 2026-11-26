//! Powers on the decibel scale the standard writes them in.

/// A power in dBm: decibels over one milliwatt, as a signed octet holds it
/// (IEEE Std 802.11-2020, 9.4.2.15, the Power Capability element's
/// Minimum and Maximum Transmit Power Capability).
///
/// A ratio between two powers is a dB value, not a power, and is no `Dbm`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Dbm(i8);

impl Dbm {
    /// `dbm` dBm.
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    /// The dBm.
    pub const fn get(self) -> i8 {
        self.0
    }

    /// This power over `floor`, in dB, wrapping as a signed octet does.
    pub const fn over(self, floor: Self) -> i8 {
        self.0.wrapping_sub(floor.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Dbm;

    #[test]
    fn a_power_is_its_dbm_and_its_distance_from_a_floor_is_in_db() {
        assert_eq!(Dbm::new(-40).get(), -40);
        assert_eq!(Dbm::new(-40).over(Dbm::new(-95)), 55);
        assert_eq!(Dbm::new(i8::MAX).over(Dbm::new(i8::MIN)), -1);
        assert!(Dbm::new(-40) > Dbm::new(-95));
    }
}
