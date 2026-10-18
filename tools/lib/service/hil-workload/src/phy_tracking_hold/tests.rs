use oer_hil_protocol::phy::PhyTrackingHoldClients;

use super::*;

fn timing(maximum_micros: u32) -> PhyTrackingHoldTiming {
    PhyTrackingHoldTiming {
        completed: 1,
        failed: 0,
        maximum_micros,
        elapsed_micros: maximum_micros,
    }
}

fn sample(configuration: &'static str, load: Load, hold: u32) -> Sample {
    Sample {
        configuration,
        load,
        evidence: PhyTrackingHoldEvidence {
            tick: PhyTrackingHoldTick::Tracked,
            clients: Some(PhyTrackingHoldClients {
                wifi: true,
                bluetooth: false,
                ieee802154: false,
            }),
            tracking: timing(hold * 2),
            grant_protect_common: timing(hold),
            grant_protect_transmit: timing(hold / 2),
            wifi_baseband_off: timing(hold - 1),
            ..PhyTrackingHoldEvidence::default()
        },
    }
}

fn every_configuration(load: Load) -> Vec<Sample> {
    CONFIGURATIONS
        .iter()
        .map(|configuration| sample(configuration.name, load, 100))
        .collect()
}

/// Each configuration and load keeps its own longest hold, so a tick under
/// traffic is never hidden by a shorter idle one.
#[test]
fn bounds_keep_the_longest_hold_per_configuration_and_load() {
    let samples = [
        sample("calibration", Load::Traffic, 900),
        sample("calibration", Load::Traffic, 1_200),
        sample("calibration", Load::Idle, 700),
    ];
    let bounds = bounds(&samples);
    let traffic = bounds
        .iter()
        .find(|(name, load, _)| *name == "calibration" && *load == Load::Traffic)
        .unwrap()
        .2;
    assert_eq!(traffic.ticks, 2);
    assert_eq!(traffic.grant_protect_common_micros, 1_200);
    assert_eq!(traffic.tracking_micros, 2_400);
    let idle = bounds
        .iter()
        .find(|(name, load, _)| *name == "calibration" && *load == Load::Idle)
        .unwrap()
        .2;
    assert_eq!(idle.grant_protect_common_micros, 700);
}

#[test]
fn a_complete_set_of_ticks_passes() {
    let mut samples = every_configuration(Load::Traffic);
    samples.extend(every_configuration(Load::Idle));
    assert!(check(&samples, &bounds(&samples)).is_ok());
}

/// A failed, skipped or malformed tick, or a configuration that timed no
/// tick under a measured load, refuses the repetition.
#[test]
fn a_failed_skipped_malformed_or_missing_tick_fails() {
    for broken in [
        |sample: &mut Sample| sample.evidence.tick = PhyTrackingHoldTick::Failed,
        |sample: &mut Sample| sample.evidence.tick = PhyTrackingHoldTick::Skipped,
        |sample: &mut Sample| sample.evidence.invalid = true,
    ] {
        let mut samples = every_configuration(Load::Traffic);
        broken(&mut samples[1]);
        assert!(check(&samples, &bounds(&samples)).is_err());
    }
    let mut samples = every_configuration(Load::Idle);
    samples.retain(|sample| sample.configuration != "both");
    assert!(check(&samples, &bounds(&samples)).is_err());
}
