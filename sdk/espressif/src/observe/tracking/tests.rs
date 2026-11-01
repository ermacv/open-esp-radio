use super::*;

fn tracked(temperature: i16, micros: u32, branches: Branches) -> Tick {
    Tick {
        ran: Ran::Tracked(branches),
        temperature: Some(temperature),
        micros,
    }
}

#[test]
fn a_reading_counts_ticks_and_branches_and_keeps_its_epoch_s_extremes() {
    let counter = TrackingCounter::new();
    // In epoch 0: counted, but its extremes are not epoch 1's.
    counter.observe(tracked(90, 5_000, Branches::default()), 0);
    let start = counter.sample(1).unwrap();
    assert_eq!(start.temperature_min, None, "epoch 1 has no tick yet");
    assert_eq!((start.ticks, start.tick_us), (1, 5_000));
    let corrected = Branches {
        rfpll_corrected: true,
        calibration_transmit: true,
        ..Branches::default()
    };
    counter.observe(tracked(40, 300, corrected), 1);
    counter.observe(
        Tick {
            ran: Ran::NotDue,
            temperature: None,
            micros: 2,
        },
        1,
    );
    counter.observe(
        tracked(
            -7,
            1_200,
            Branches {
                wifi_i2c_changed: true,
                ..Branches::default()
            },
        ),
        1,
    );
    counter.observe(tracked(12, 100, Branches::default()), 1);
    counter.observe(
        Tick {
            ran: Ran::Unavailable,
            temperature: None,
            micros: 0,
        },
        1,
    );
    assert_eq!(
        counter.sample(1),
        Ok(TrackingSample {
            ticks: 4,
            not_due: 1,
            unavailable: 1,
            inhibited: 0,
            rfpll_corrected: 1,
            wifi_i2c_changed: 1,
            wifi_tx_power: 0,
            bluetooth_ieee802154_tx_power: 0,
            calibration_common: 0,
            calibration_transmit: 1,
            tick_us: 6_602,
            temperature_last: Some(12),
            temperature_min: Some(-7),
            temperature_max: Some(40),
            longest_tick_us: Some(1_200),
        })
    );
}

#[test]
fn a_reading_of_another_epoch_reports_no_temperature_or_longest_tick() {
    let counter = TrackingCounter::new();
    counter.observe(tracked(30, 10, Branches::default()), 3);
    let sample = counter.sample(4).unwrap();
    assert_eq!(
        (
            sample.temperature_last,
            sample.temperature_min,
            sample.temperature_max,
            sample.longest_tick_us
        ),
        (None, None, None, None)
    );
    assert_eq!(sample.ticks, 1, "the counts are not the epoch's");
}

#[test]
fn a_sample_during_a_write_is_refused() {
    let counter = TrackingCounter::new();
    counter.sequence.store(1, Ordering::Relaxed);
    assert_eq!(counter.sample(0), Err(Concurrent));
}

#[test]
fn the_extreme_temperatures_of_the_type_round_trip() {
    let counter = TrackingCounter::new();
    counter.observe(tracked(i16::MIN, 0, Branches::default()), 1);
    counter.observe(tracked(i16::MAX, u32::MAX, Branches::default()), 1);
    let sample = counter.sample(1).unwrap();
    assert_eq!(
        (
            sample.temperature_min,
            sample.temperature_max,
            sample.temperature_last
        ),
        (Some(i16::MIN), Some(i16::MAX), Some(i16::MAX))
    );
    assert_eq!(
        sample.longest_tick_us,
        Some(u32::MAX - 1),
        "the longest saturates"
    );
}
