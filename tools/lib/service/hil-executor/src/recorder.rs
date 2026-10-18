//! What one scenario run measures besides its trace.

use core::fmt;
use std::collections::BTreeMap;

use oer_hil_model::{MeasuredValue, MeasurementName, Unit};

/// Why a count or measurement was not recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordError {
    /// The name is already a count or a measurement.
    Collision(MeasurementName),
    /// A count no longer fits in `u64`.
    Overflow(MeasurementName),
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collision(name) => write!(f, "`{name}` is already a count or a measurement"),
            Self::Overflow(name) => write!(f, "count `{name}` overflowed"),
        }
    }
}

impl std::error::Error for RecordError {}

/// The counts and measurements of one run, in the model's terms; the
/// runner converts them into the run record's format. A name is either a
/// count or a measurement, never both, so no record can hide another.
#[derive(Debug, Default)]
pub struct Recorder {
    counts: BTreeMap<MeasurementName, u64>,
    measurements: Vec<MeasuredValue>,
}

impl Recorder {
    fn measured(&self, name: &MeasurementName) -> bool {
        self.measurements
            .iter()
            .any(|measurement| &measurement.name == name)
    }

    /// Adds to a named count; counts become `count` measurements.
    pub fn count(&mut self, name: &MeasurementName, delta: u64) -> Result<(), RecordError> {
        if self.measured(name) {
            return Err(RecordError::Collision(name.clone()));
        }
        let count = self.counts.entry(name.clone()).or_default();
        *count = count
            .checked_add(delta)
            .ok_or_else(|| RecordError::Overflow(name.clone()))?;
        Ok(())
    }

    pub fn measure(&mut self, measurement: MeasuredValue) -> Result<(), RecordError> {
        if self.counts.contains_key(&measurement.name) || self.measured(&measurement.name) {
            return Err(RecordError::Collision(measurement.name));
        }
        self.measurements.push(measurement);
        Ok(())
    }

    /// Every measurement: the explicit ones, then each count by name.
    pub fn measurements(&self) -> Vec<MeasuredValue> {
        let mut all = self.measurements.clone();
        all.extend(self.counts.iter().map(|(name, &value)| MeasuredValue {
            name: name.clone(),
            value,
            unit: Unit::Count,
        }));
        all
    }
}

#[cfg(test)]
mod tests {
    use oer_hil_model::name;

    use super::*;

    #[test]
    fn counts_become_measurements() {
        let mut recorder = Recorder::default();
        let attempts = name!(MeasurementName, "connect.attempts");
        recorder.count(&attempts, 2).unwrap();
        recorder.count(&attempts, 1).unwrap();
        assert_eq!(
            recorder.measurements(),
            vec![MeasuredValue {
                name: attempts,
                value: 3,
                unit: Unit::Count
            }]
        );
    }

    #[test]
    fn a_name_is_a_count_or_a_measurement_never_both() {
        let mut recorder = Recorder::default();
        let lost = name!(MeasurementName, "connect.lost");
        recorder.count(&lost, 1).unwrap();
        let same = MeasuredValue {
            name: lost.clone(),
            value: 9,
            unit: Unit::Count,
        };
        assert_eq!(recorder.measure(same), Err(RecordError::Collision(lost)));
        let drain = name!(MeasurementName, "drain");
        let value = || MeasuredValue {
            name: drain.clone(),
            value: 4,
            unit: Unit::Microseconds,
        };
        recorder.measure(value()).unwrap();
        assert_eq!(
            recorder.measure(value()),
            Err(RecordError::Collision(drain.clone()))
        );
        assert_eq!(
            recorder.count(&drain, 1),
            Err(RecordError::Collision(drain))
        );
    }

    #[test]
    fn an_overflowing_count_is_refused() {
        let mut recorder = Recorder::default();
        let name = name!(MeasurementName, "frames");
        recorder.count(&name, u64::MAX).unwrap();
        assert_eq!(recorder.count(&name, 1), Err(RecordError::Overflow(name)));
    }
}
