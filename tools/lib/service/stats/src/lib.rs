//! Numerical statistics for host analyses.
//!
//! - [`student_t_critical`]: the exact two-sided critical value of
//!   Student's t for any positive, possibly fractional, number of degrees of
//!   freedom (a Welch interval's are fractional). It inverts the two-sided
//!   tail `P(|T| > t) = I_{ν/(ν+t²)}(ν/2, 1/2)` by bisection on `t`, with the
//!   regularized incomplete beta function `I` evaluated by its continued
//!   fraction (modified Lentz) and `ln Γ` by the Lanczos approximation
//!   (g = 7, n = 9), both accurate to about 1e-15 relative. No table and no
//!   normal approximation: the value at 32 degrees of freedom and 95 % is
//!   2.…, not a rounded 2.000.
//! - [`balanced_swaps`]: the seeded order of the rounds of a two-arm
//!   experiment, balanced pair by pair.
//!
//! Depends on nothing; callers own the meaning of what they compare.

/// Degrees of freedom above which the quantile is evaluated at this value.
/// Student's t quantile falls as the degrees of freedom grow, so the value
/// here is at least the true quantile of any larger number. Its measured
/// distance above the limit (the normal quantile) is about `(z² + 1) / (4ν)`
/// relative: 8.3e-8 at 90 %, 1.22e-7 at 95 %, 1.91e-7 at 99 % and 2.96e-7
/// at 99.9 % confidence, below 3e-7 (the tests measure it). The bisection's own
/// last step adds about 1e-16 relative, either way.
const LARGEST_FREEDOM: f64 = 1e7;

/// The two-sided critical value `t` of Student's t distribution with
/// `freedom` degrees of freedom at `confidence` (0 < confidence < 1):
/// `P(|T| ≤ t) = confidence`. `None` when `confidence` is outside (0, 1) or
/// `freedom` is not a positive number; an infinite `freedom` is evaluated as
/// 10^7 degrees of freedom.
pub fn student_t_critical(confidence: f64, freedom: f64) -> Option<f64> {
    if !(0.0..1.0).contains(&confidence) || confidence == 0.0 || freedom.is_nan() || freedom <= 0.0
    {
        return None;
    }
    let freedom = freedom.min(LARGEST_FREEDOM);
    let alpha = 1.0 - confidence;
    // The tail falls monotonically in t: bracket, then bisect to the last
    // representable step.
    let mut high = 1.0_f64;
    while two_sided_tail(high, freedom) > alpha {
        high *= 2.0;
        if !high.is_finite() {
            return None;
        }
    }
    let mut low = 0.0_f64;
    for _ in 0..2048 {
        let middle = low + (high - low) / 2.0;
        if middle <= low || middle >= high {
            break;
        }
        if two_sided_tail(middle, freedom) > alpha {
            low = middle;
        } else {
            high = middle;
        }
    }
    // `high` is the upper end of the last bracket: the smallest value whose
    // evaluated tail is within `alpha`, to the bisection's last step.
    Some(high)
}

/// The two-sided tail `P(|T| > t)` of Student's t with `freedom` degrees of
/// freedom, for `t ≥ 0` and `freedom > 0`.
pub fn student_t_two_sided_tail(t: f64, freedom: f64) -> Option<f64> {
    (t >= 0.0 && freedom > 0.0 && t.is_finite())
        .then(|| two_sided_tail(t, freedom.min(LARGEST_FREEDOM)))
}

fn two_sided_tail(t: f64, freedom: f64) -> f64 {
    let square = t * t;
    // x and 1 − x computed separately, so neither loses precision when the
    // other is close to 1.
    let x = freedom / (freedom + square);
    let complement = square / (freedom + square);
    regularized_incomplete_beta(x, complement, freedom / 2.0, 0.5)
}

/// `I_x(a, b)` for `x + complement = 1`, `a, b > 0`.
fn regularized_incomplete_beta(x: f64, complement: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if complement <= 0.0 {
        return 1.0;
    }
    let front = (a * x.ln() + b * complement.ln() - ln_beta(a, b)).exp();
    // The continued fraction converges fast below the mean of the
    // distribution; above it, use I_x(a, b) = 1 − I_{1−x}(b, a).
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_continued_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_continued_fraction(b, a, complement) / b
    }
}

/// The continued fraction of the incomplete beta function, evaluated by the
/// modified Lentz method.
fn beta_continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    const EPSILON: f64 = 1e-16;
    let guard = |value: f64| if value.abs() < TINY { TINY } else { value };
    let (sum, plus, minus) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 / guard(1.0 - sum * x / plus);
    let mut fraction = d;
    for m in 1..=100_000 {
        let m = f64::from(m);
        let twice = 2.0 * m;
        let even = m * (b - m) * x / ((minus + twice) * (a + twice));
        d = 1.0 / guard(1.0 + even * d);
        c = guard(1.0 + even / c);
        fraction *= d * c;
        let odd = -(a + m) * (sum + m) * x / ((a + twice) * (plus + twice));
        d = 1.0 / guard(1.0 + odd * d);
        c = guard(1.0 + odd / c);
        let step = d * c;
        fraction *= step;
        if (step - 1.0).abs() < EPSILON {
            break;
        }
    }
    fraction
}

fn ln_beta(a: f64, b: f64) -> f64 {
    ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
}

/// `ln Γ(x)` for `x > 0`, Lanczos approximation with g = 7 and nine
/// coefficients.
fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const COEFFICIENTS: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection: Γ(x) Γ(1 − x) = π / sin(πx).
        return (std::f64::consts::PI / (std::f64::consts::PI * x).sin().abs()).ln()
            - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let t = x + G + 0.5;
    let series = COEFFICIENTS[1..]
        .iter()
        .enumerate()
        .fold(COEFFICIENTS[0], |sum, (index, coefficient)| {
            sum + coefficient / (x + index as f64 + 1.0)
        });
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + series.ln()
}

/// The order of `rounds` rounds of a two-arm experiment: `true` where a
/// round runs the second arm first.
///
/// Rounds come in consecutive pairs; within each pair one round runs the
/// first arm first and the other the second arm first, and which of the two
/// leads is a bit of a SplitMix64 stream seeded with `seed`. Every complete
/// pair is therefore balanced, an even number of rounds has each arm first
/// exactly half the time, and the sequence is reproducible from the seed.
pub fn balanced_swaps(seed: u64, rounds: usize) -> Vec<bool> {
    let mut state = seed;
    let mut swaps = Vec::with_capacity(rounds);
    while swaps.len() < rounds {
        let leads_swapped = split_mix_64(&mut state) & 1 == 1;
        swaps.push(leads_swapped);
        if swaps.len() < rounds {
            swaps.push(!leads_swapped);
        }
    }
    swaps
}

/// One step of SplitMix64 (Steele, Lea and Flood, 2014).
fn split_mix_64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound of the relative distance between the quantile at
    /// [`LARGEST_FREEDOM`] and the normal quantile, 90 % to 99.9 %.
    const MEASURED_BOUND: f64 = 3e-7;

    /// Two-sided standard normal quantiles: the limit of Student's t.
    const NORMAL: [(f64, f64); 4] = [
        (0.90, 1.644_853_626_951_472_2),
        (0.95, 1.959_963_984_540_054),
        (0.99, 2.575_829_303_548_900_4),
        (0.999, 3.290_526_731_491_894_5),
    ];

    #[test]
    fn the_largest_freedom_stays_within_its_documented_distance_of_the_normal_quantile() {
        for (confidence, normal) in NORMAL {
            let t = student_t_critical(confidence, f64::INFINITY).unwrap();
            let relative = (t - normal) / normal;
            // The first-order expansion of the t quantile: (z² + 1) / (4ν).
            let expected = (normal * normal + 1.0) / (4.0 * LARGEST_FREEDOM);
            assert!(relative > 0.0, "{confidence}: {relative:e}");
            assert!(
                relative < MEASURED_BOUND,
                "{confidence}: {relative:e} vs {expected:e}"
            );
        }
    }

    #[test]
    fn known_quantiles_are_exact() {
        let t = student_t_critical(0.95, 32.0).unwrap();
        assert!((t - 2.036_933_343_460_101_7).abs() < 1e-12, "{t}");
        let t = student_t_critical(0.95, 1.0).unwrap();
        assert!((t - 12.706_204_736_174_698).abs() < 1e-9, "{t}");
    }

    /// Two-sided 95 % quantiles computed independently (numerical
    /// integration of the density), to 1e-8.
    const QUANTILES_95: [(f64, f64); 8] = [
        (1.0, 12.706_204_74),
        (2.0, 4.302_652_73),
        (2.5, 3.574_654_84),
        (30.0, 2.042_272_46),
        (31.0, 2.039_513_45),
        (60.0, 2.000_297_82),
        (61.0, 1.999_623_58),
        (32.0, 2.036_933_34),
    ];

    #[test]
    fn quantiles_match_on_both_sides_of_the_old_table_boundaries() {
        for (freedom, expected) in QUANTILES_95 {
            let t = student_t_critical(0.95, freedom).unwrap();
            assert!(
                (t - expected).abs() < 1e-6,
                "df {freedom}: {t} vs {expected}"
            );
        }
        // The old table's steps (2.000 for 31..=60, 1.960 above) were too
        // narrow just past each boundary.
        assert!(student_t_critical(0.95, 31.0).unwrap() > 2.0);
        assert!(student_t_critical(0.95, 61.0).unwrap() > 1.96);
    }

    #[test]
    fn a_huge_freedom_approaches_the_normal_quantile_from_above() {
        let normal = 1.959_963_984_540_054;
        for freedom in [1e7, 1e12, f64::INFINITY] {
            let t = student_t_critical(0.95, freedom).unwrap();
            assert!(t >= normal, "df {freedom}: {t}");
            assert!((t - 1.959_964).abs() < 1e-6, "df {freedom}: {t}");
        }
    }

    #[test]
    fn invalid_inputs_have_no_quantile() {
        for (confidence, freedom) in [
            (0.0, 10.0),
            (1.0, 10.0),
            (-0.5, 10.0),
            (1.5, 10.0),
            (f64::NAN, 10.0),
            (0.95, 0.0),
            (0.95, -3.0),
            (0.95, f64::NAN),
        ] {
            assert_eq!(
                student_t_critical(confidence, freedom),
                None,
                "{confidence}, {freedom}"
            );
        }
        assert_eq!(student_t_two_sided_tail(-1.0, 10.0), None);
        assert_eq!(student_t_two_sided_tail(1.0, 0.0), None);
        assert_eq!(student_t_two_sided_tail(f64::INFINITY, 10.0), None);
    }

    #[test]
    fn the_tail_at_the_critical_value_is_the_complement_of_the_confidence() {
        for freedom in [1.0, 2.5, 16.0, 31.0, 61.0, 1000.0] {
            let t = student_t_critical(0.95, freedom).unwrap();
            let tail = student_t_two_sided_tail(t, freedom).unwrap();
            assert!((tail - 0.05).abs() < 1e-12, "df {freedom}: {tail}");
        }
    }

    #[test]
    fn balanced_swaps_balance_every_pair_and_repeat_per_seed() {
        for seed in [0, 1, 42, u64::MAX] {
            let swaps = balanced_swaps(seed, 10);
            assert_eq!(swaps.len(), 10);
            for pair in swaps.chunks(2) {
                assert_ne!(pair[0], pair[1], "seed {seed}: {swaps:?}");
            }
            assert_eq!(swaps.iter().filter(|swapped| **swapped).count(), 5);
            assert_eq!(balanced_swaps(seed, 10), swaps);
        }
        // An odd count keeps every complete pair balanced.
        let odd = balanced_swaps(7, 5);
        assert_eq!(odd.len(), 5);
        assert_eq!(&odd[..4], &balanced_swaps(7, 4)[..]);
        // The seed decides the leaders.
        let orders: std::collections::BTreeSet<Vec<bool>> =
            (0..16).map(|seed| balanced_swaps(seed, 16)).collect();
        assert!(orders.len() > 1);
    }
}
