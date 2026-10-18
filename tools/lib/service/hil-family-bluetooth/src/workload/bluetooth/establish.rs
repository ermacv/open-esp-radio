//! Bounded retry of an LE connection the DUT never received.
//!
//! A peer's CONNECT_IND can collide in the air with another device's
//! response to the same `ADV_IND`: the stand's adapter then reports the
//! connection created and closes it as "Connection Failed to be Established"
//! (0x3E), while the DUT never saw it. A central retries such a connection;
//! the fixture does the same, but only when the DUT's own observation proves
//! the connection never reached it, and it records every attempt so the loss
//! stays visible as a measurement. The host's connect alone proves nothing:
//! the kernel may report the socket connected before the first procedure, so
//! an attempt succeeds only once the DUT confirms the connection
//! ([`CONFIRMATION`]).

use crate::Result;
use oer_hil_run_bundle_format::run::{Better, Measurement, MeasurementUnit};

/// Attempts per connection, the first included.
pub(super) const ATTEMPTS: u32 = 3;

/// How long the DUT may take to report a connection the host opened. A
/// connection the DUT never received ends on the host within about six
/// connection intervals; the DUT reports a received one at once.
pub(super) const CONFIRMATION: std::time::Duration = std::time::Duration::from_secs(2);

/// The outcome of one connection attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Outcome {
    /// The attempt succeeded.
    Established,
    /// The attempt failed and the DUT never saw the connection.
    NotEstablished,
    /// The attempt failed and the DUT saw the connection: not retried.
    Failed,
    /// The DUT's observation of the attempt failed: not retried.
    Unobserved,
}

/// One connection attempt, as the scenario's observations record it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct Attempt {
    /// What the connection was for.
    pub phase: &'static str,
    /// The DUT connection number the attempt was for, counted from 1 in
    /// the DUT's application epoch.
    pub dut_connection: u32,
    /// 1-based.
    pub attempt: u32,
    pub outcome: Outcome,
    /// The host-side error of a failed attempt.
    pub host_error: Option<String>,
}

/// Open the connection `dut_connection` until the DUT confirms it.
///
/// One attempt runs `connect` on the host, then `confirm`, which tells
/// whether the DUT reported the connection within [`CONFIRMATION`]. A
/// connection the DUT did not confirm is closed on the host with `release`.
/// A failed attempt is retried, at most [`ATTEMPTS`] times in all, only
/// while `unseen` proves that the DUT never saw it; any other failure ends at
/// once with its host-side error, and exhausted attempts end with the last
/// one. An error of `confirm`, `release` or `unseen` is a failure of the
/// observation itself and ends at once with its own cause and the attempt's
/// host-side error. Every attempt lands in `attempts`, whichever way it ends.
/// The closures share `state`, such as the scenario's sample log.
#[allow(clippy::too_many_arguments, reason = "one attempt's four steps")]
pub(super) fn establish<S, T>(
    phase: &'static str,
    dut_connection: u32,
    attempts: &mut Vec<Attempt>,
    state: &mut S,
    mut connect: impl FnMut(&mut S) -> Result<T>,
    mut confirm: impl FnMut(&mut S) -> Result<bool>,
    mut release: impl FnMut(&mut S, T) -> Result<()>,
    mut unseen: impl FnMut(&mut S) -> Result<bool>,
) -> Result<T> {
    let mut record = |attempt, outcome, host_error: Option<&str>| {
        attempts.push(Attempt {
            phase,
            dut_connection,
            attempt,
            outcome,
            host_error: host_error.map(str::to_owned),
        });
    };
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        let host_error = match connect(state) {
            Ok(peer) => {
                match confirm(state) {
                    Ok(true) => {
                        record(attempt, Outcome::Established, None);
                        return Ok(peer);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        record(attempt, Outcome::Unobserved, None);
                        return Err(error);
                    }
                }
                let host_error = format!("DUT did not confirm connection {dut_connection}");
                if let Err(error) = release(state, peer) {
                    record(attempt, Outcome::Unobserved, Some(&host_error));
                    return unobserved(error, &host_error);
                }
                host_error
            }
            Err(error) => error.to_string(),
        };
        match unseen(state) {
            Ok(true) => record(attempt, Outcome::NotEstablished, Some(&host_error)),
            Ok(false) => {
                record(attempt, Outcome::Failed, Some(&host_error));
                return Err(host_error.into());
            }
            Err(error) => {
                record(attempt, Outcome::Unobserved, Some(&host_error));
                return unobserved(error, &host_error);
            }
        }
        last = host_error;
    }
    Err(format!(
        "{phase}: no connection reached the DUT in {ATTEMPTS} attempts; the last host \
         error: {last}"
    )
    .into())
}

/// An observation's failure after the attempt's own host-side error.
fn unobserved<T>(error: impl std::fmt::Display, host_error: &str) -> Result<T> {
    Err(format!("{error} (after the host error: {host_error})").into())
}

/// The repetition's connection attempts as measurements: all of them, and
/// those the DUT never received. HIL records the loss; whether it is
/// acceptable is qualification's decision.
pub(super) fn measurements(attempts: &[Attempt]) -> [Measurement; 2] {
    let lost = attempts
        .iter()
        .filter(|attempt| attempt.outcome == Outcome::NotEstablished)
        .count() as u64;
    [
        Measurement::observed(
            "connection-attempts",
            attempts.len() as u64,
            MeasurementUnit::Count,
        ),
        Measurement::observed("connections-not-established", lost, MeasurementUnit::Count)
            .better(Better::Lower),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted host and DUT: each attempt's host result and whether the
    /// DUT confirmed and saw it.
    #[derive(Default)]
    struct Script {
        host: Vec<Result<u8>>,
        confirmed: Vec<bool>,
        seen: Vec<bool>,
        released: Vec<u8>,
    }

    fn run(script: &mut Script, attempts: &mut Vec<Attempt>) -> Result<u8> {
        establish(
            "connect",
            2,
            attempts,
            script,
            |s| s.host.remove(0),
            |s| Ok(s.confirmed.remove(0)),
            |s, peer| {
                s.released.push(peer);
                Ok(())
            },
            |s| Ok(!s.seen.remove(0)),
        )
    }

    #[test]
    fn a_host_failure_the_dut_never_saw_is_retried_and_recorded() {
        let mut script = Script {
            host: vec![Err("ATT socket disconnected".into()), Ok(7)],
            confirmed: vec![true],
            seen: vec![false],
            ..Script::default()
        };
        let mut attempts = Vec::new();
        assert_eq!(run(&mut script, &mut attempts).unwrap(), 7);
        assert_eq!(
            attempts.iter().map(|a| a.outcome).collect::<Vec<_>>(),
            [Outcome::NotEstablished, Outcome::Established]
        );
        assert_eq!(
            attempts[0].host_error.as_deref(),
            Some("ATT socket disconnected")
        );
        assert!(attempts.iter().all(|a| a.dut_connection == 2));
    }

    #[test]
    fn a_host_connection_the_dut_never_confirmed_is_released_and_retried() {
        let mut script = Script {
            host: vec![Ok(1), Ok(2)],
            confirmed: vec![false, true],
            seen: vec![false],
            ..Script::default()
        };
        let mut attempts = Vec::new();
        assert_eq!(run(&mut script, &mut attempts).unwrap(), 2);
        assert_eq!(script.released, [1]);
        assert_eq!(
            attempts[0].host_error.as_deref(),
            Some("DUT did not confirm connection 2")
        );
        assert_eq!(attempts[1].outcome, Outcome::Established);
    }

    #[test]
    fn an_unconfirmed_connection_the_dut_saw_is_released_and_fails_at_once() {
        let mut script = Script {
            host: vec![Ok(1)],
            confirmed: vec![false],
            seen: vec![true],
            ..Script::default()
        };
        let mut attempts = Vec::new();
        let error = run(&mut script, &mut attempts).unwrap_err();
        assert_eq!(error.to_string(), "DUT did not confirm connection 2");
        assert_eq!(script.released, [1]);
        assert_eq!(
            attempts.iter().map(|a| a.outcome).collect::<Vec<_>>(),
            [Outcome::Failed]
        );
    }

    #[test]
    fn a_host_failure_the_dut_saw_ends_at_once() {
        let mut script = Script {
            host: vec![Err("ATT socket disconnected".into())],
            seen: vec![true],
            ..Script::default()
        };
        let mut attempts = Vec::new();
        let error = run(&mut script, &mut attempts).unwrap_err();
        assert_eq!(error.to_string(), "ATT socket disconnected");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].outcome, Outcome::Failed);
        assert_eq!(
            attempts[0].host_error.as_deref(),
            Some("ATT socket disconnected")
        );
    }

    #[test]
    fn retries_are_bounded() {
        let mut script = Script {
            host: (0..ATTEMPTS)
                .map(|_| Err("le-connection-abort-by-local".into()))
                .collect(),
            seen: vec![false; ATTEMPTS as usize],
            ..Script::default()
        };
        let mut attempts = Vec::new();
        let error = run(&mut script, &mut attempts).unwrap_err();
        assert_eq!(attempts.len(), ATTEMPTS as usize);
        assert_eq!(
            error.to_string(),
            "connect: no connection reached the DUT in 3 attempts; the last host error: \
             le-connection-abort-by-local"
        );
    }

    #[test]
    fn measurements_count_every_attempt_and_the_lost_ones() {
        let attempt = |outcome| Attempt {
            phase: "connect",
            dut_connection: 1,
            attempt: 1,
            outcome,
            host_error: None,
        };
        let [all, lost] = measurements(&[
            attempt(Outcome::NotEstablished),
            attempt(Outcome::Established),
            attempt(Outcome::Established),
        ]);
        assert_eq!((all.name.as_str(), all.value), ("connection-attempts", 3));
        assert_eq!(
            (lost.name.as_str(), lost.value),
            ("connections-not-established", 1)
        );
        assert_eq!(lost.better, Some(Better::Lower));
        assert!(all.is_consistent() && lost.is_consistent());
    }

    #[test]
    fn a_failed_observation_keeps_the_attempt_and_its_host_error() {
        let mut attempts = Vec::new();
        let confirm = establish(
            "connect",
            1,
            &mut attempts,
            &mut (),
            |_| Ok(1u8),
            |_| Err("missing or insufficient GATT task-stack headroom".into()),
            |_, _| Ok(()),
            |_| Ok(true),
        )
        .unwrap_err();
        assert_eq!(
            confirm.to_string(),
            "missing or insufficient GATT task-stack headroom"
        );
        let unseen = establish(
            "connect",
            1,
            &mut attempts,
            &mut (),
            |_| -> Result<u8> { Err("ATT socket disconnected".into()) },
            |_| Ok(true),
            |_, _| Ok(()),
            |_| Err("serial link lost".into()),
        )
        .unwrap_err();
        assert_eq!(
            unseen.to_string(),
            "serial link lost (after the host error: ATT socket disconnected)"
        );
        let released = establish(
            "connect",
            1,
            &mut attempts,
            &mut (),
            |_| Ok(1u8),
            |_| Ok(false),
            |_, _| Err("ATT fixture deadline".into()),
            |_| Ok(true),
        )
        .unwrap_err();
        assert_eq!(
            released.to_string(),
            "ATT fixture deadline (after the host error: DUT did not confirm connection 1)"
        );
        assert_eq!(
            attempts
                .iter()
                .map(|a| (a.outcome, a.host_error.as_deref()))
                .collect::<Vec<_>>(),
            [
                (Outcome::Unobserved, None),
                (Outcome::Unobserved, Some("ATT socket disconnected")),
                (
                    Outcome::Unobserved,
                    Some("DUT did not confirm connection 1")
                ),
            ]
        );
        assert_eq!(measurements(&attempts)[0].value, 3);
    }
}
