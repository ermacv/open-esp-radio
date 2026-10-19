use super::*;

#[derive(Clone, Copy, Debug)]
struct Span(u32, u32);

impl RadioChannel for Span {
    fn span(&self) -> FrequencySpan {
        FrequencySpan::new(self.0, self.1).unwrap()
    }
}

const RULES: &[Rule] = &[
    Rule::new(
        FrequencySpan::new(2_400_000, 2_483_500).unwrap(),
        40_000,
        2000,
        RuleFlags::NONE,
    ),
    Rule::new(
        FrequencySpan::new(5_150_000, 5_250_000).unwrap(),
        80_000,
        2300,
        RuleFlags::NO_IR,
    ),
    Rule::new(
        FrequencySpan::new(5_250_000, 5_350_000).unwrap(),
        80_000,
        2000,
        RuleFlags::DFS,
    ),
];

fn domain() -> RegulatoryDomain {
    RegulatoryDomain::new(Country::new(*b"XX").unwrap(), RULES).unwrap()
}

const NO_LIMITS: PowerLimits = PowerLimits {
    antenna_gain_mbm: 0,
    chip_maximum_mbm: i32::MAX,
    owner_cap_mbm: None,
};

#[test]
fn country_is_two_uppercase_letters() {
    assert!(Country::new(*b"DE").is_ok());
    assert!(Country::new(*b"de").is_err());
    assert!(Country::new(*b"D1").is_err());
}

#[test]
fn domain_rejects_unsorted_and_overlapping_rules() {
    static OVERLAP: [Rule; 2] = [
        Rule::new(
            FrequencySpan::new(2_400_000, 2_450_000).unwrap(),
            40_000,
            2000,
            RuleFlags::NONE,
        ),
        Rule::new(
            FrequencySpan::new(2_440_000, 2_483_500).unwrap(),
            40_000,
            2000,
            RuleFlags::NONE,
        ),
    ];
    static UNSORTED: [Rule; 2] = [
        Rule::new(
            FrequencySpan::new(5_150_000, 5_250_000).unwrap(),
            80_000,
            2000,
            RuleFlags::NONE,
        ),
        Rule::new(
            FrequencySpan::new(2_400_000, 2_483_500).unwrap(),
            40_000,
            2000,
            RuleFlags::NONE,
        ),
    ];
    let country = Country::new(*b"XX").unwrap();
    assert_eq!(
        RegulatoryDomain::new(country, &OVERLAP),
        Err(InvalidDomain::Overlapping { index: 1 })
    );
    assert_eq!(
        RegulatoryDomain::new(country, &UNSORTED),
        Err(InvalidDomain::Overlapping { index: 1 })
    );
}

#[test]
fn permits_a_channel_inside_a_rule_with_its_ceiling() {
    let permitted = domain().permit(Span(2_427_000, 2_447_000)).unwrap();
    assert_eq!(permitted.initiate(), Initiate::Allowed);
    assert_eq!(permitted.eirp_limit_mbm(), 2000);
}

#[test]
fn refuses_a_channel_partly_outside_the_rules() {
    assert_eq!(
        domain().permit(Span(2_473_000, 2_495_000)).unwrap_err(),
        Refused::NotAllocated
    );
    assert_eq!(
        domain().permit(Span(900_000, 910_000)).unwrap_err(),
        Refused::NotAllocated
    );
}

#[test]
fn refuses_a_channel_wider_than_its_rule() {
    assert_eq!(
        domain().permit(Span(2_400_000, 2_480_000)).unwrap_err(),
        Refused::Bandwidth
    );
}

#[test]
fn no_initiating_radiation_is_passive_only() {
    let permitted = domain().permit(Span(5_170_000, 5_190_000)).unwrap();
    assert_eq!(permitted.initiate(), Initiate::PassiveOnly);
}

#[test]
fn dfs_channels_are_refused() {
    assert_eq!(
        domain().permit(Span(5_250_000, 5_270_000)).unwrap_err(),
        Refused::RadarDetectionRequired
    );
}

#[test]
fn a_channel_across_touching_rules_takes_the_least_ceiling_and_every_flag() {
    // Spans two rules: the NO-IR one at 23 dBm and the DFS one at 20 dBm.
    assert_eq!(
        domain().permit(Span(5_240_000, 5_260_000)).unwrap_err(),
        Refused::RadarDetectionRequired
    );
    static TOUCHING: [Rule; 2] = [
        Rule::new(
            FrequencySpan::new(2_400_000, 2_440_000).unwrap(),
            40_000,
            2000,
            RuleFlags::NONE,
        ),
        Rule::new(
            FrequencySpan::new(2_440_000, 2_483_500).unwrap(),
            40_000,
            1000,
            RuleFlags::NO_IR,
        ),
    ];
    let touching = RegulatoryDomain::new(Country::new(*b"XX").unwrap(), &TOUCHING).unwrap();
    let permitted = touching.permit(Span(2_430_000, 2_450_000)).unwrap();
    assert_eq!(permitted.eirp_limit_mbm(), 1000);
    assert_eq!(permitted.initiate(), Initiate::PassiveOnly);
}

#[test]
fn a_gap_between_rules_refuses() {
    static GAP: [Rule; 2] = [
        Rule::new(
            FrequencySpan::new(2_400_000, 2_440_000).unwrap(),
            40_000,
            2000,
            RuleFlags::NONE,
        ),
        Rule::new(
            FrequencySpan::new(2_441_000, 2_483_500).unwrap(),
            40_000,
            2000,
            RuleFlags::NONE,
        ),
    ];
    let gap = RegulatoryDomain::new(Country::new(*b"XX").unwrap(), &GAP).unwrap();
    assert_eq!(
        gap.permit(Span(2_430_000, 2_450_000)).unwrap_err(),
        Refused::NotAllocated
    );
}

#[test]
fn ceiling_is_the_least_of_domain_less_antenna_chip_and_owner_cap() {
    let permitted = domain().permit(Span(2_427_000, 2_447_000)).unwrap();
    assert_eq!(permitted.tx_ceiling(NO_LIMITS).mbm(), 2000);
    let board = PowerLimits {
        antenna_gain_mbm: 300,
        ..NO_LIMITS
    };
    assert_eq!(permitted.tx_ceiling(board).mbm(), 1700);
    let chip = PowerLimits {
        chip_maximum_mbm: 1500,
        ..board
    };
    assert_eq!(permitted.tx_ceiling(chip).mbm(), 1500);
    let capped = PowerLimits {
        owner_cap_mbm: Some(1000),
        ..chip
    };
    assert_eq!(permitted.tx_ceiling(capped).mbm(), 1000);
    let loose_cap = PowerLimits {
        owner_cap_mbm: Some(1900),
        ..chip
    };
    assert_eq!(permitted.tx_ceiling(loose_cap).mbm(), 1500);
}

#[test]
fn ceiling_rounds_down_to_the_unit() {
    assert_eq!(TxCeiling(1799).quarter_dbm(), 71);
    assert_eq!(TxCeiling(1799).dbm(), 17);
    assert_eq!(TxCeiling(-1).quarter_dbm(), -1);
    assert_eq!(TxCeiling(100_000).quarter_dbm(), i8::MAX);
    assert_eq!(TxCeiling(-100_000).dbm(), i8::MIN);
}

#[test]
fn indoor_only_rules_are_reported() {
    static INDOOR: [Rule; 1] = [Rule::new(
        FrequencySpan::new(5_150_000, 5_250_000).unwrap(),
        80_000,
        2300,
        RuleFlags::NO_OUTDOOR,
    )];
    let indoor = RegulatoryDomain::new(Country::new(*b"XX").unwrap(), &INDOOR).unwrap();
    assert!(
        indoor
            .permit(Span(5_170_000, 5_190_000))
            .unwrap()
            .indoor_only()
    );
    assert!(
        !domain()
            .permit(Span(2_427_000, 2_447_000))
            .unwrap()
            .indoor_only()
    );
}

#[test]
fn germany_permits_the_2_4_ghz_channels_one_to_thirteen_and_not_fourteen() {
    let germany = RegulatoryDomain::of(Country::new(*b"DE").unwrap()).unwrap();
    for number in 1..=13_u32 {
        let centre = 2_407_000 + 5_000 * number;
        let permitted = germany
            .permit(Span(centre - 10_000, centre + 10_000))
            .unwrap();
        assert_eq!(permitted.initiate(), Initiate::Allowed);
        assert_eq!(permitted.eirp_limit_mbm(), 2000);
    }
    assert_eq!(
        germany.permit(Span(2_473_000, 2_495_000)).unwrap_err(),
        Refused::NotAllocated
    );
    // 5 GHz channel 52 (DFS) is refused; channel 36 is permitted indoors only.
    assert_eq!(
        germany.permit(Span(5_250_000, 5_270_000)).unwrap_err(),
        Refused::RadarDetectionRequired
    );
    assert!(
        germany
            .permit(Span(5_170_000, 5_190_000))
            .unwrap()
            .indoor_only()
    );
}

#[test]
fn a_country_without_generated_rules_has_no_domain() {
    assert!(RegulatoryDomain::of(Country::new(*b"ZZ").unwrap()).is_none());
}
