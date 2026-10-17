//! The single owner of the station TSF writes.
//!
//! The MAC's interface-zero timer counts the station's TSF. Every write to
//! it goes through [`StationTsf`], which records the write in the
//! portable [`TsfRelation`]: a follow within the drift since the last sample
//! keeps the relation's generation, a jump starts a new one. The hardware
//! write takes a [`StationTsfWrite`] that only this module constructs, so a
//! backend implements [`StationTsfHardware`] but no caller writes the timer
//! around the owner.

use oer_ieee80211_lower_mac::{TsfGeneration, TsfRelation, TsfSetKind, TsfTimingError};
use oer_ieee80211_mac::tsf::TsfInstant;

/// The uncertainty of a station TSF sample: the set value and the reading
/// beside it are each a count of the station TSF counter's microseconds.
pub const STATION_TSF_SAMPLE_UNCERTAINTY: oer_ieee80211_mac::tsf::TsfDuration =
    oer_ieee80211_mac::tsf::TsfDuration::from_micros(1);

/// The capability to write the station TSF, which only [`StationTsf`]
/// holds.
#[derive(Debug)]
pub struct StationTsfWrite {
    _owner: (),
}

/// The station TSF of the MAC's interface-zero timer.
pub trait StationTsfHardware {
    fn station_tsf(&mut self) -> u64;

    /// Replace the station TSF; only [`StationTsf`] can call it.
    fn set_station_tsf(&mut self, write: StationTsfWrite, value: u64);
}

/// The owner of the station TSF writes and their relation to the radio
/// clock. It lives as long as its user (one association of the role, one
/// port core); its generations come from an epoch its creator supplies, a
/// number no other TSF relation owner took, so a later owner's generations
/// never repeat an earlier one's.
#[derive(Debug, Eq, PartialEq)]
pub struct StationTsf {
    relation: TsfRelation,
}

impl StationTsf {
    /// An owner in `epoch`, a number no other owner took.
    pub const fn new(epoch: u32) -> Self {
        Self {
            relation: TsfRelation::new(epoch, STATION_TSF_SAMPLE_UNCERTAINTY),
        }
    }

    /// The generation of the station TSF's relation.
    pub const fn generation(&self) -> TsfGeneration {
        self.relation.generation()
    }

    /// The station TSF now.
    pub fn read<H: StationTsfHardware>(&self, hardware: &mut H) -> TsfInstant {
        TsfInstant::from_micros(hardware.station_tsf())
    }

    /// Set the station TSF, a jump or a follow of drift
    /// ([`TsfRelation::set`]).
    pub fn set<H: StationTsfHardware>(
        &mut self,
        hardware: &mut H,
        value: TsfInstant,
    ) -> Result<TsfSetKind, TsfTimingError> {
        let current = self.read(hardware);
        let kind = self.relation.set(current, value)?;
        hardware.set_station_tsf(StationTsfWrite { _owner: () }, value.as_micros());
        Ok(kind)
    }

    /// Start a new generation without a sample: the station follows another
    /// TSF (an association, a channel change).
    pub fn break_relation(&mut self) {
        self.relation.break_relation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Timer {
        tsf: u64,
        writes: usize,
    }

    impl StationTsfHardware for Timer {
        fn station_tsf(&mut self) -> u64 {
            self.tsf
        }

        fn set_station_tsf(&mut self, _: StationTsfWrite, value: u64) {
            self.tsf = value;
            self.writes += 1;
        }
    }

    #[test]
    fn the_owner_writes_the_timer_and_keeps_the_relation_through_drift() {
        let mut timer = Timer::default();
        let mut owner = StationTsf::new(1);
        let tsf = TsfInstant::from_micros;
        assert_eq!(
            owner.set(&mut timer, tsf(1_000_000)).unwrap(),
            TsfSetKind::Jump
        );
        assert_eq!((timer.tsf, timer.writes), (1_000_000, 1));
        let generation = owner.generation();
        timer.tsf += 1_024_000;
        let corrected = tsf(timer.tsf + 50);
        assert_eq!(owner.set(&mut timer, corrected).unwrap(), TsfSetKind::Drift);
        assert_eq!(owner.generation(), generation);
        timer.tsf += 102_400;
        let jumped = tsf(timer.tsf + 50);
        assert_eq!(owner.set(&mut timer, jumped).unwrap(), TsfSetKind::Jump);
        assert_ne!(owner.generation(), generation);
        assert_eq!(timer.writes, 3);
    }
}
