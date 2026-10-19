//! How many samples a bound needs, from the bound itself rather than habit.
//!
//! A bound reported as the maximum of `n` independent samples covers the
//! quantile `q` with confidence `c` once `qⁿ ≤ 1 − c`. The loss-rate and
//! comparison statistics of the architecture arrive with their first
//! consumer.

/// A probability strictly between zero and one.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Probability(f64);

impl Probability {
    /// `None` outside (0, 1), NaN included.
    pub const fn new(value: f64) -> Option<Self> {
        if 0.0 < value && value < 1.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

/// A confidence strictly between zero and one.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Confidence(f64);

impl Confidence {
    /// `None` outside (0, 1), NaN included.
    pub const fn new(value: f64) -> Option<Self> {
        if 0.0 < value && value < 1.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

/// The smallest `n` with `qⁿ ≤ 1 − c`: the maximum of `n` independent
/// samples then lies at or above the `q` quantile with confidence `c`.
pub fn quantile_samples(quantile: Probability, confidence: Confidence) -> u64 {
    // ln(1 − c) through `ln_1p`, so a confidence too small for `1.0 − c`
    // to differ from one still gives a negative logarithm; both logarithms
    // are negative for arguments in (0, 1), and at least one sample is
    // always needed, since q⁰ = 1 > 1 − c.
    let n = ((-confidence.get()).ln_1p() / quantile.get().ln()).ceil();
    (n as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_maximum_of_2995_samples_covers_the_999th_permille_at_95_percent() {
        let confidence = Confidence::new(0.95).unwrap();
        assert_eq!(
            quantile_samples(Probability::new(0.999).unwrap(), confidence),
            2_995
        );
        assert_eq!(
            quantile_samples(Probability::new(0.99).unwrap(), confidence),
            299
        );
    }

    #[test]
    fn a_tiny_confidence_still_needs_one_sample() {
        let tiny = Confidence::new(1e-17).unwrap();
        assert_eq!(quantile_samples(Probability::new(0.999).unwrap(), tiny), 1);
        let low = Confidence::new(0.5).unwrap();
        // 0.5ⁿ ≤ 0.5 at n = 1.
        assert_eq!(quantile_samples(Probability::new(0.5).unwrap(), low), 1);
    }

    #[test]
    fn values_outside_the_open_interval_are_refused() {
        for value in [0.0, 1.0, -0.5, 1.5, f64::NAN] {
            assert!(Probability::new(value).is_none(), "{value}");
            assert!(Confidence::new(value).is_none(), "{value}");
        }
    }
}
