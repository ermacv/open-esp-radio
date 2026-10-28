//! The relation of an interface's TSF to the port's radio clock across the
//! TSF writes the backend makes.

use oer_ieee80211_port::{Ieee80211Instant, TSF_DRIFT_PPM, TsfGeneration};
use oer_ieee80211_wire::tsf::{TsfDuration, TsfInstant};

/// The relation of one interface's TSF to the radio clock across TSF sets:
/// its generation and its last sample, the value the TSF was last set to.
///
/// The backend that owns an interface's TSF writes keeps one and records
/// every write. A set within what the relation predicts since its last
/// sample keeps the generation: [`TSF_DRIFT_PPM`] of the time since then,
/// rounded up, plus the sample's uncertainty. A beacon follow after missed
/// beacons corrects more drift and stays within it; a larger set, and the
/// first one, is a jump and starts a new generation. The TSF is not modular
/// here: a set to a value across 2^64, or one after the counter passed 2^64
/// since the last sample (a reading below it), is a jump, so the instants of
/// one generation always compare in order.
#[derive(Debug, Eq, PartialEq)]
pub struct TsfRelation {
    generations: oer_ieee80211_port::GenerationSource<TsfInstant, Ieee80211Instant>,
    last_sample: LastSample,
    sample_uncertainty: TsfDuration,
}

/// The value the TSF was last set to in the current generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LastSample {
    /// No set since the relation began or broke: the next set is a jump.
    Unanchored,
    /// The last set's value.
    Set(TsfInstant),
}

/// What a TSF set did to its relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfSetKind {
    /// Within the drift since the last sample: the generation stays.
    Drift,
    /// Beyond it, or the first set: a new generation.
    Jump,
}

/// A TSF set could not establish a representable timing relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfTimingError {
    /// An elapsed span could not order its current and earlier samples.
    ReversedTime,
    /// Drift plus sample uncertainty exceeds the TSF duration range.
    ToleranceOverflow,
}

impl TsfRelation {
    /// A relation without a sample in `epoch`, a number its owner took from
    /// the shared source, whose samples are each uncertain by
    /// `sample_uncertainty` (the backend's TSF counter resolution).
    pub const fn new(epoch: u32, sample_uncertainty: TsfDuration) -> Self {
        Self {
            generations: oer_ieee80211_port::GenerationSource::new(epoch),
            last_sample: LastSample::Unanchored,
            sample_uncertainty,
        }
    }

    /// The current generation.
    pub const fn generation(&self) -> TsfGeneration {
        self.generations.current()
    }

    /// Start a new generation without a sample: the TSF jumped (a new
    /// association, a channel change, a timer restart).
    pub fn break_relation(&mut self) {
        self.generations.jump();
        self.last_sample = LastSample::Unanchored;
    }

    /// How far a set `elapsed` after the last sample may move the TSF
    /// without breaking the relation.
    pub fn tolerance(&self, elapsed: TsfDuration) -> Result<TsfDuration, TsfTimingError> {
        // The product of a u64 span and the fixed ppm coefficient fits u128.
        let product = u128::from(elapsed.as_micros())
            .checked_mul(u128::from(TSF_DRIFT_PPM))
            .ok_or(TsfTimingError::ToleranceOverflow)?;
        let drift = (product / 1_000_000)
            .checked_add(u128::from(product % 1_000_000 != 0))
            .ok_or(TsfTimingError::ToleranceOverflow)?;
        let drift = u64::try_from(drift).map_err(|_| TsfTimingError::ToleranceOverflow)?;
        TsfDuration::from_micros(drift)
            .checked_add(self.sample_uncertainty)
            .ok_or(TsfTimingError::ToleranceOverflow)
    }

    /// Record a set of the TSF from `current`, the value read just before
    /// it, to `value`.
    pub fn set(
        &mut self,
        current: TsfInstant,
        value: TsfInstant,
    ) -> Result<TsfSetKind, TsfTimingError> {
        let kind = match self.last_sample {
            // The counter only advances from the last set: a reading below
            // it passed 2^64.
            LastSample::Set(last) if current < last => TsfSetKind::Jump,
            LastSample::Set(last) => {
                let elapsed = current
                    .checked_duration_since(last)
                    .ok_or(TsfTimingError::ReversedTime)?;
                let moved = value.as_micros().abs_diff(current.as_micros());
                if moved <= self.tolerance(elapsed)?.as_micros() {
                    TsfSetKind::Drift
                } else {
                    TsfSetKind::Jump
                }
            }
            LastSample::Unanchored => TsfSetKind::Jump,
        };
        if kind == TsfSetKind::Jump {
            self.break_relation();
        }
        self.last_sample = LastSample::Set(value);
        Ok(kind)
    }
}

#[cfg(test)]
mod tests;
