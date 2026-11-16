use super::*;
use oer_protocols_radio_regulatory::{Country, FrequencySpan, Rule, RuleFlags};

/// The production PHY's power of each rate code under `ceiling` dBm.
fn production(ceiling: i8) -> impl Fn(u8) -> i8 {
    let targets = PhyConfig::production().tx_target_power_profile();
    move |rate| targets.pair(rate).primary.min(ceiling)
}

#[test]
fn the_power_elements_follow_the_production_targets() {
    // HE-MCS 0..9 at 20, 20, 20, 20, 19, 19, 18, 18, 16, 15: the indices of
    // the vendor run the archive recorded (open-esp-radio-rs `7abbf8eb6`).
    let power = elements(-11, production(20)).unwrap();
    assert_eq!(power.power_capability.minimum_dbm(), -11);
    assert_eq!(power.power_capability.maximum_dbm(), 20);
    assert_eq!(
        power.ul_mu_power.relative_to_rate_16(),
        [0, 0, 0, 1, 1, 2, 2, 4, 5]
    );
}

#[test]
fn the_ceiling_bounds_every_rate() {
    let power = elements(-11, production(15)).unwrap();
    assert_eq!(power.power_capability.maximum_dbm(), 15);
    assert_eq!(power.ul_mu_power.relative_to_rate_16(), [0; 9]);
}

#[test]
fn a_ceiling_below_the_minimum_gives_no_power_elements() {
    assert!(matches!(
        elements(-11, production(-12)),
        Err(HePowerError::Capability(_))
    ));
    // A higher rate above HE-MCS 0 has no UL-MU difference.
    assert!(matches!(
        elements(-11, |rate| if rate == 25 { 21 } else { 20 }),
        Err(HePowerError::UlMu(_))
    ));
}

/// A domain whose channel 1 is permitted 30 dBm and channel 13 10 dBm EIRP.
const TWO_CEILINGS: RegulatoryDomain = {
    const RULES: &[Rule] = &[
        Rule::new(
            match FrequencySpan::new(2_401_000, 2_423_000) {
                Some(span) => span,
                None => panic!("channel 1's span"),
            },
            20_000,
            3_000,
            RuleFlags::NONE,
        ),
        Rule::new(
            match FrequencySpan::new(2_461_000, 2_483_000) {
                Some(span) => span,
                None => panic!("channel 13's span"),
            },
            20_000,
            1_000,
            RuleFlags::NONE,
        ),
    ];
    match RegulatoryDomain::new(
        match Country::new(*b"ZZ") {
            Ok(country) => country,
            Err(_) => panic!("a country code"),
        },
        RULES,
    ) {
        Ok(domain) => domain,
        Err(_) => panic!("disjoint rules"),
    }
};

const LIMITS: PowerLimits = PowerLimits {
    antenna_gain_mbm: 0,
    chip_maximum_mbm: 2_000,
    owner_cap_mbm: None,
};

/// Each channel's elements follow its own ceiling: channel 1's are the
/// production targets (its ceiling is the chip's 20 dBm), channel 13's are
/// bounded at 10 dBm on every rate; a channel the domain does not permit
/// gives none.
#[test]
fn each_channel_has_the_elements_of_its_ceiling() {
    let wide_open = he_power(&TWO_CEILINGS, 1, LIMITS).unwrap();
    assert_eq!(
        wide_open,
        elements(MAC_MINIMUM_TX_POWER_DBM, production(20)).unwrap()
    );
    assert_eq!(wide_open.power_capability.maximum_dbm(), 20);
    let bounded = he_power(&TWO_CEILINGS, 13, LIMITS).unwrap();
    assert_eq!(bounded.power_capability.maximum_dbm(), 10);
    assert_eq!(bounded.ul_mu_power.relative_to_rate_16(), [0; 9]);
    assert_ne!(wide_open, bounded);
    assert_eq!(
        he_power(&TWO_CEILINGS, 6, LIMITS),
        Err(HePowerError::Channel)
    );
}
