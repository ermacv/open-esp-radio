//! Lease duration estimates from earlier leases, and duration text.
//!
//! Estimates only predict when a waiting request starts; no lease is
//! limited by one. Leases are charged the time they hold instead
//! ([`crate::balance`]).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::history::{LeaseOutcome, LeaseRecord};

/// Estimate of work without completed history.
pub const DEFAULT_ESTIMATE: Duration = Duration::from_secs(15 * 60);
/// Completed leases of the same work considered by the estimate.
const HISTORY_SAMPLES: usize = 5;
const MIN_ESTIMATE: u64 = 60;
const ESTIMATE_GRANULARITY: u64 = 30;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum EstimateSource {
    /// The longest of the most recent completed leases of the same work, or
    /// the sum of that for each scenario.
    History {
        samples: usize,
    },
    Default,
}

impl std::fmt::Display for EstimateSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::History { samples } => write!(f, "from {samples} earlier lease(s)"),
            Self::Default => f.write_str("default; no history for this work"),
        }
    }
}

/// Estimate how long `work` holds the stand: earlier leases of the same
/// work, then the sum over `scenarios` of earlier single-scenario leases of
/// each, then the default.
pub(crate) fn estimate(
    work: &str,
    scenarios: &[String],
    history: &[LeaseRecord],
) -> (Duration, EstimateSource) {
    let longest = |matches: &dyn Fn(&LeaseRecord) -> bool| {
        let durations = history
            .iter()
            .rev()
            .filter(|record| record.outcome == LeaseOutcome::Released && matches(record))
            .map(LeaseRecord::duration_secs)
            .take(HISTORY_SAMPLES)
            .collect::<Vec<_>>();
        durations
            .iter()
            .max()
            .map(|longest| (*longest, durations.len()))
    };
    let estimate = longest(&|record| record.work == work).or_else(|| {
        if scenarios.is_empty() {
            return None;
        }
        scenarios
            .iter()
            .try_fold((0, 0), |(total, samples), scenario| {
                let (duration, count) = longest(&|record| {
                    record.scenarios.as_slice() == std::slice::from_ref(scenario)
                })?;
                Some((total + duration, samples + count))
            })
    });
    match estimate {
        Some((seconds, samples)) => {
            let rounded = seconds.div_ceil(ESTIMATE_GRANULARITY) * ESTIMATE_GRANULARITY;
            (
                Duration::from_secs(rounded.max(MIN_ESTIMATE)),
                EstimateSource::History { samples },
            )
        }
        None => (DEFAULT_ESTIMATE, EstimateSource::Default),
    }
}

/// Parse `90s`, `15m`, `1h30m` or a plain number of minutes.
pub fn parse_duration(text: &str) -> crate::Result<Duration> {
    let text = text.trim();
    if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Duration::from_secs(text.parse::<u64>()? * 60));
    }
    let mut total = 0u64;
    let mut number = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        let unit = match character {
            'd' => 86_400,
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => {
                return Err(
                    format!("invalid duration `{text}`; use e.g. 90s, 15m, 1h30m or 7d").into(),
                );
            }
        };
        if number.is_empty() {
            return Err(format!("invalid duration `{text}`; a unit needs a number").into());
        }
        total += number.parse::<u64>()? * unit;
        number.clear();
    }
    if !number.is_empty() || total == 0 {
        return Err(format!("invalid duration `{text}`; use e.g. 90s, 15m or 1h30m").into());
    }
    Ok(Duration::from_secs(total))
}

/// `45s`, `12m30s`, `1h05m`.
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match (seconds / 3600, seconds % 3600 / 60, seconds % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, m, _) => format!("{h}h{m:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(work: &str, seconds: u64, outcome: LeaseOutcome) -> LeaseRecord {
        LeaseRecord {
            id: 0,
            owner: "wifi".into(),
            work: work.into(),
            granted_unix: 1000,
            released_unix: 1000 + seconds,
            outcome,
            charged_ms: 0,
            balance_after_ms: 0,
            reason: None,
            preempted: None,
            scenarios: Vec::new(),
            run: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn durations_round_trip_through_their_display() {
        for (text, seconds) in [
            ("90s", 90),
            ("15m", 900),
            ("1h30m", 5400),
            ("20", 1200),
            ("7d", 604_800),
        ] {
            assert_eq!(parse_duration(text).unwrap(), Duration::from_secs(seconds));
        }
        for text in ["", "m", "10x", "0s", "5m3"] {
            assert!(parse_duration(text).is_err(), "{text}");
        }
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(750)), "12m30s");
        assert_eq!(format_duration(Duration::from_secs(3900)), "1h05m");
    }

    #[test]
    fn estimates_use_the_longest_recent_completed_lease_of_the_same_work() {
        let history = [
            record("run a", 400, LeaseOutcome::Released),
            record("run a", 100, LeaseOutcome::Released),
            record("run a", 5000, LeaseOutcome::HardLimit),
            record("run b", 3000, LeaseOutcome::Released),
            record("run a", 131, LeaseOutcome::Released),
        ];
        assert_eq!(
            estimate("run a", &[], &history),
            (
                Duration::from_secs(420),
                EstimateSource::History { samples: 3 }
            )
        );
        assert_eq!(
            estimate("run c", &[], &history),
            (DEFAULT_ESTIMATE, EstimateSource::Default)
        );
        let short = [record("smoke", 3, LeaseOutcome::Released)];
        assert_eq!(estimate("smoke", &[], &short).0, Duration::from_secs(60));
    }

    #[test]
    fn a_new_series_is_estimated_from_its_scenarios_single_leases() {
        let single = |scenario: &str, seconds| LeaseRecord {
            scenarios: vec![scenario.to_owned()],
            work: format!("run {scenario} --stand-file /x"),
            ..record("", seconds, LeaseOutcome::Released)
        };
        let history = [single("a", 100), single("a", 200), single("b", 50)];
        let series = [String::from("a"), String::from("b")];
        assert_eq!(
            estimate("run a b", &series, &history),
            (
                Duration::from_secs(270),
                EstimateSource::History { samples: 3 }
            )
        );
        let unknown = [String::from("a"), String::from("c")];
        assert_eq!(
            estimate("run a c", &unknown, &history).1,
            EstimateSource::Default
        );
    }
}
