use oer_ieee80211_mac::extensions::wmm::WmmAcParameters;

use super::*;

/// A seeded xorshift32 source: the same seed draws the same backoffs.
struct Seeded(u32);

impl BackoffEntropy for Seeded {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

const BEST_EFFORT: WmmAcParameters = WmmAcParameters {
    admission_control_mandatory: false,
    aifsn: 3,
    ecw_min: 4,
    ecw_max: 10,
    txop_limit_units_32_us: 0,
};

#[test]
fn the_window_doubles_per_failure_up_to_cw_max_and_resets() {
    let mut contention = EdcaContention::from_wmm(BEST_EFFORT);
    let mut windows = [0_u16; 8];
    for window in &mut windows {
        *window = contention.cw();
        contention.record_failure();
    }
    assert_eq!(windows, [15, 31, 63, 127, 255, 511, 1023, 1023]);
    assert_eq!(contention.retries(), 6);
    contention.reset();
    assert_eq!(contention.cw(), 15);
    assert_eq!(contention.retries(), 0);
}

#[test]
fn draws_stay_inside_the_window_and_cover_it() {
    let mut contention = EdcaContention::new(2, 3);
    let mut entropy = Seeded(0x1234_5678);
    let mut seen = [false; 4];
    for _ in 0..200 {
        let drawn = contention.draw(&mut entropy);
        assert!(drawn <= 3);
        seen[usize::from(drawn)] = true;
    }
    assert_eq!(seen, [true; 4]);
    contention.record_failure();
    contention.record_failure();
    for _ in 0..200 {
        assert!(contention.draw(&mut entropy) <= 7);
    }
}

#[test]
fn a_seeded_source_is_deterministic() {
    let draw = |seed| {
        let contention = EdcaContention::from_wmm(BEST_EFFORT);
        let mut entropy = Seeded(seed);
        let mut drawn = [0_u16; 16];
        for slot in &mut drawn {
            *slot = contention.draw(&mut entropy);
        }
        drawn
    };
    assert_eq!(draw(7), draw(7));
    assert_ne!(draw(7), draw(8));
}

#[test]
fn the_stateless_draw_matches_the_window_of_its_retry_count() {
    // An all-ones source draws the window itself.
    let mut ones = || u32::MAX;
    assert_eq!(draw_backoff(BEST_EFFORT, 0, &mut ones), 15);
    assert_eq!(draw_backoff(BEST_EFFORT, 2, &mut ones), 63);
    assert_eq!(draw_backoff(BEST_EFFORT, 200, &mut ones), 1023);
}

#[test]
fn out_of_range_exponents_are_clamped() {
    assert_eq!(EdcaContention::new(20, 30).cw_exponent(), MAX_CW_EXPONENT);
    assert_eq!(EdcaContention::new(5, 2).cw(), 31);
    let mut fixed = EdcaContention::new(5, 2);
    fixed.record_failure();
    assert_eq!(fixed.cw(), 31);
}

#[test]
fn reconfiguration_keeps_the_current_window_inside_the_new_bounds() {
    let mut contention = EdcaContention::new(4, 10);
    contention.record_failure();
    contention.record_failure();
    assert_eq!(contention.cw_exponent(), 6);
    // Still inside: kept.
    contention.reconfigure(5, 10);
    assert_eq!(contention.cw_exponent(), 6);
    // Above a lower maximum: lowered.
    contention.reconfigure(2, 3);
    assert_eq!(contention.cw_exponent(), 3);
    // Below a higher minimum: raised.
    contention.reconfigure(7, 9);
    assert_eq!(contention.cw_exponent(), 7);
    assert_eq!(
        (contention.cw_min_exponent(), contention.cw_max_exponent()),
        (7, 9)
    );
    contention.reset();
    assert_eq!(contention.cw_exponent(), 7);
}
