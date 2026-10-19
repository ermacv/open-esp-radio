use super::*;
use crate::{PhyConfig, PhyState, analog::temperature::PhyTemperatureOutcome};

#[test]
fn freshness_includes_acquisition_wait_and_rejects_reversed_clocks() {
    let sample = Observation {
        value: 40,
        acquisition: Acquisition::Window {
            started: 100,
            completed: 180,
        },
    };
    assert_eq!(
        sample.freshness(200, 100),
        Freshness::Fresh { age_micros: 100 }
    );
    assert_eq!(
        sample.freshness(201, 100),
        Freshness::Stale { age_micros: 101 }
    );
    assert_eq!(sample.freshness(179, 100), Freshness::ClockReversed);
    assert_eq!(
        Observation {
            acquisition: Acquisition::Window {
                started: 180,
                completed: 100
            },
            ..sample
        }
        .freshness(200, 100),
        Freshness::ClockReversed
    );
}

#[test]
fn undated_replacement_cannot_inherit_a_previous_measurement_time() {
    let mut state = PhyState::new(PhyConfig::production());
    assert_eq!(
        state.temperature_observation().acquisition,
        Acquisition::Unobserved
    );
    let outcome = PhyTemperatureOutcome {
        temperature: 20,
        sensor_index: 2,
        next_dac: 15,
    };
    state.apply_observed_temperature_outcome(outcome, Some(100), Some(110));
    assert_eq!(
        state.temperature_observation().freshness(120, 30),
        Freshness::Fresh { age_micros: 20 }
    );
    // A channel restore can replace the sensor result. Its completion alone
    // must not preserve the timestamp of the earlier runtime observation.
    state.apply_temperature_outcome(outcome);
    assert_eq!(
        state.temperature_observation().freshness(120, 30),
        Freshness::Unknown
    );
    state.apply_observed_temperature_outcome(outcome, None, Some(130));
    assert_eq!(
        state.temperature_observation().freshness(130, 30),
        Freshness::Unknown
    );
}

#[test]
fn acquisition_provenance_fails_closed_for_unrepresentable_duration_or_reversed_clock() {
    let mut state = PhyState::new(PhyConfig::production());
    let outcome = PhyTemperatureOutcome {
        temperature: 20,
        sensor_index: 2,
        next_dac: 15,
    };
    state.apply_observed_temperature_outcome(outcome, Some(100), Some(99));
    assert_eq!(
        state.temperature_observation().freshness(200, 1000),
        Freshness::ClockReversed
    );
    state.apply_observed_temperature_outcome(outcome, Some(0), Some(u64::MAX));
    assert_eq!(
        state.temperature_observation().acquisition,
        Acquisition::Overlong
    );
    state.apply_observed_temperature_outcome(outcome, Some(u64::MAX - 10), Some(u64::MAX));
    assert_eq!(
        state.temperature_observation().freshness(u64::MAX, 20),
        Freshness::Fresh { age_micros: 10 }
    );
}
