use oer_esp32s31_phy::PhyState;

use super::*;

fn sources() -> MacSources {
    MacSources {
        pti: [1, 3, 10, 15].map(MacCoexPti::from_osi_value),
        tx_power: PhyState::default().tx_target_power_profile(),
        slow_clock: MacSlowClockCalibration::Unavailable,
        delay_random: 0x1234_5678,
    }
}

#[test]
fn each_coex_event_reads_its_own_priority() {
    let mut sources = sources();
    for (event, value) in MAC_COEX_EVENTS.into_iter().zip([1, 3, 10, 15]) {
        assert_eq!(sources.mac_coex_pti(event).value(), value);
    }
}

#[test]
fn the_tx_power_pairs_are_the_profiles_and_the_platform_values_pass_through() {
    let mut sources = sources();
    let profile = PhyState::default().tx_target_power_profile();
    for rate in 0..=40 {
        let pair = profile.pair(rate);
        assert_eq!(
            sources.mac_tx_power_pair(rate),
            MacTxPowerPair {
                primary: pair.primary,
                alternate: pair.alternate,
            }
        );
    }
    assert_eq!(sources.mac_delay_random(), 0x1234_5678);
    assert_eq!(
        sources.mac_slow_clock_calibration(),
        MacSlowClockCalibration::Unavailable
    );
}
