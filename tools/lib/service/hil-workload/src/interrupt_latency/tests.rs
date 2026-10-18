use super::*;

fn sample(samples: u16, missed: u16) -> Sample {
    Sample {
        load: Load::Traffic,
        evidence: InterruptLatencyEvidence {
            cpu_hz: 400_000_000,
            samples,
            missed,
            minimum_cycles: 400,
            median_cycles: 800,
            p99_cycles: 4_000,
            maximum_cycles: 40_000,
        },
    }
}

/// Cycles convert at the device's CPU clock: 400 cycles at 400 MHz are 1 µs.
#[test]
fn cycles_convert_at_the_device_clock() {
    assert_eq!(nanoseconds(400, 400_000_000), 1_000);
    assert_eq!(nanoseconds(40_000, 400_000_000), 100_000);
}

/// Traffic samples spread over the traffic window after the lead, ending
/// before it: 1024 samples over 14 s are about 12.3 ms apart.
#[test]
fn traffic_samples_spread_over_their_window() {
    let interval = spread(Duration::from_secs(14), 1024);
    assert_eq!(interval.as_micros(), 12_304);
    assert!(interval * 1024 < Duration::from_secs(14));
}

/// A missed sample, or a request that took none, refuses the repetition.
#[test]
fn a_missed_or_empty_measurement_fails() {
    assert!(check(&[sample(1024, 0)]).is_ok());
    assert!(check(&[sample(1023, 1)]).is_err());
    assert!(check(&[sample(0, 0)]).is_err());
}
