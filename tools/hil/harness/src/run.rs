//! Running a scenario's cases on a connected device and its verdict.

use std::{io::Write, time::Duration};

use oer_hil_link::{Boot, Link, LinkError};

use crate::dut::{Device, Dut};
use crate::finish::{Cleanup, Ending};
use crate::{Case, Cx, Failure, Scenario, keys};

/// How long the running boot may take to say Hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// What one case, or the stand before any case, came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Outcome {
    /// The case's name; `None` for the stand and the device's readiness.
    pub case: Option<&'static str>,
    pub verdict: Verdict,
}

impl Outcome {
    /// The stand, the image or the link failed before the cases.
    pub fn stand(reason: String) -> Self {
        Self {
            case: None,
            verdict: Verdict::Incomplete(reason),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    Pass,
    Fail(String),
    Incomplete(String),
}

impl Verdict {
    /// The process exit code of a scenario whose worst outcome is this.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Fail(_) => 1,
            Self::Incomplete(_) => 2,
        }
    }
}

/// `cases` that `names` selects, in the scenario's order; every case when
/// `names` is empty. An unknown name is an error.
pub(crate) fn select<'a>(cases: &'a [Case], names: &[String]) -> Result<Vec<&'a Case>, String> {
    if let Some(unknown) = names
        .iter()
        .find(|name| !cases.iter().any(|case| case.name == name.as_str()))
    {
        return Err(format!("the scenario has no case `{unknown}`"));
    }
    Ok(cases
        .iter()
        .filter(|case| names.is_empty() || names.iter().any(|name| name == case.name))
        .collect())
}

/// One measurement a case recorded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measurement {
    pub case: &'static str,
    pub name: String,
    pub value: serde_json::Value,
}

/// What running the cases came to.
pub(crate) struct Run {
    pub outcomes: Vec<Outcome>,
    pub measurements: Vec<Measurement>,
    /// How the cases left the device, for finishing.
    pub ending: Ending,
}

/// Run `cases` on `dut` after checking that it runs the
/// built image and that the image serves every service of `scenario`,
/// printing each outcome to `out` as it is known. A case that sees a reboot
/// fails and the next one runs on the new boot; a request without a
/// response leaves the boot's remaining cases incomplete.
pub(crate) fn cases(
    dut: &mut Dut<'_>,
    scenario: &Scenario,
    cases: &[&Case],
    out: &mut impl Write,
) -> Run {
    let image_crc32 = dut.image_crc32();
    let mut outcomes = Vec::new();
    let mut measurements = Vec::new();
    let mut record = |outcome: Outcome, out: &mut dyn Write| {
        print(&outcome, out);
        outcomes.push(outcome);
    };
    let ready = match dut.link() {
        Some(link) => ready(link, scenario, image_crc32),
        None => Err("the device under test has no link".to_owned()),
    };
    let mut boot = match ready {
        Ok(boot) => boot,
        Err(reason) => {
            record(Outcome::stand(reason), out);
            return Run {
                outcomes,
                measurements,
                ending: Ending { unconfirmed: false },
            };
        }
    };
    let mut unconfirmed = None;
    for case in cases {
        let verdict = if let Some(request_id) = unconfirmed {
            Verdict::Incomplete(format!(
                "request {request_id} of boot {:016x} is unconfirmed",
                boot.boot_id
            ))
        } else {
            let mut cx = Cx::new(dut, boot);
            let verdict = match (case.run)(&mut cx) {
                Ok(()) => Verdict::Pass,
                Err(Failure::Fail(reason)) => Verdict::Fail(reason),
                Err(Failure::Incomplete(reason)) => Verdict::Incomplete(reason),
            };
            // A reset during the case moved it to a later boot.
            boot = cx.boot;
            unconfirmed = cx.unconfirmed;
            measurements.extend(
                cx.measurements
                    .into_iter()
                    .map(|(name, value)| Measurement {
                        case: case.name,
                        name,
                        value,
                    }),
            );
            verdict
        };
        let Some(link) = dut.link() else {
            record(
                Outcome {
                    case: Some(case.name),
                    verdict,
                },
                out,
            );
            record(
                Outcome::stand("the device under test has no link".to_owned()),
                out,
            );
            break;
        };
        print_notes(link);
        record(
            Outcome {
                case: Some(case.name),
                verdict,
            },
            out,
        );
        if oer_os_process::cancellation_requested() {
            record(Outcome::stand(LinkError::Cancelled.to_string()), out);
            break;
        }
        match link.boot(HELLO_TIMEOUT) {
            Ok(next) if next.boot_id != boot.boot_id => {
                if next.hello.image_crc32 != image_crc32 {
                    record(
                        Outcome::stand(format!(
                            "the board rebooted into image {:08x}, not the built {image_crc32:08x}",
                            next.hello.image_crc32
                        )),
                        out,
                    );
                    break;
                }
                boot = next;
                unconfirmed = None;
            }
            Ok(_) => {}
            Err(error) => {
                record(Outcome::stand(error.to_string()), out);
                break;
            }
        }
    }
    Run {
        outcomes,
        measurements,
        ending: Ending {
            unconfirmed: unconfirmed.is_some(),
        },
    }
}

/// The results file of a run: the scenario, the board, every outcome and
/// every measurement.
pub(crate) fn results(
    scenario: &Scenario,
    board: &str,
    unix: u64,
    outcomes: &[Outcome],
    measurements: &[Measurement],
) -> serde_json::Value {
    serde_json::json!({
        "scenario": scenario.name,
        "image": scenario.image,
        "board": board,
        "unix": unix,
        "outcomes": outcomes.iter().map(|outcome| {
            let (verdict, reason) = match &outcome.verdict {
                Verdict::Pass => ("pass", None),
                Verdict::Fail(reason) => ("fail", Some(reason)),
                Verdict::Incomplete(reason) => ("incomplete", Some(reason)),
            };
            serde_json::json!({
                "case": outcome.case.unwrap_or("stand"),
                "verdict": verdict,
                "reason": reason,
            })
        }).collect::<Vec<_>>(),
        "measurements": measurements.iter().map(|measurement| serde_json::json!({
            "case": measurement.case,
            "name": measurement.name,
            "value": measurement.value,
        })).collect::<Vec<_>>(),
    })
}

/// Print the device's diagnostic lines and how many were lost.
fn print_notes(link: &Link) {
    let notes = link.take_notes();
    for note in notes.lines {
        eprintln!("device: {note}");
    }
    if notes.lost > 0 {
        eprintln!("device: {} diagnostic lines lost", notes.lost);
    }
}

/// Print one outcome as a line of `out`.
pub(crate) fn print(outcome: &Outcome, out: &mut dyn Write) {
    let name = outcome.case.unwrap_or("stand");
    let line = match &outcome.verdict {
        Verdict::Pass => format!("case {name}: pass"),
        Verdict::Fail(reason) => format!("case {name}: fail: {reason}"),
        Verdict::Incomplete(reason) => format!("case {name}: incomplete: {reason}"),
    };
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The running boot, once it runs the image of identity `image_crc32` and
/// that image serves every service of `scenario`.
fn ready(link: &Link, scenario: &Scenario, image_crc32: u32) -> Result<Boot, String> {
    let boot = link
        .boot(HELLO_TIMEOUT)
        .map_err(|error| error.to_string())?;
    if boot.hello.image_crc32 != image_crc32 {
        return Err(format!(
            "the board runs image {:08x}, not the built {image_crc32:08x}",
            boot.hello.image_crc32
        ));
    }
    let served = keys::served(link, &boot).map_err(|error| format!("image keys: {error}"))?;
    let missing = keys::missing(scenario.services, &served);
    if missing.is_empty() {
        Ok(boot)
    } else {
        let missing = missing.iter().map(ToString::to_string).collect::<Vec<_>>();
        Err(format!(
            "the image does not serve the scenario's services: missing keys {}",
            missing.join(", ")
        ))
    }
}

/// Print the cleanup and the summary of `outcomes` to `out`; the
/// scenario's verdict: a failure over an incomplete case over a pass, and a
/// passing scenario whose `cleanup` was not clean is incomplete.
pub fn summary(
    scenario: &Scenario,
    outcomes: &[Outcome],
    cleanup: Option<&Cleanup>,
    out: &mut impl Write,
) -> Verdict {
    let mut counts = [0_usize; 3];
    let mut worst = Verdict::Pass;
    for outcome in outcomes {
        let index = usize::from(outcome.verdict.exit_code());
        if let Some(count) = counts.get_mut(index) {
            *count += 1;
        }
        if outcome.verdict.exit_code() == 1
            || (outcome.verdict.exit_code() == 2 && worst == Verdict::Pass)
        {
            worst = outcome.verdict.clone();
        }
    }
    if outcomes.is_empty() {
        worst = Verdict::Incomplete("no case ran".to_owned());
    }
    let problem = cleanup.and_then(Cleanup::problem);
    match (cleanup, &problem) {
        (Some(_), Some(problem)) => {
            let _ = writeln!(out, "cleanup: {problem}");
        }
        (Some(_), None) => {
            let _ = writeln!(out, "cleanup: clean");
        }
        (None, _) => {}
    }
    if let (Verdict::Pass, Some(problem)) = (&worst, &problem) {
        worst = Verdict::Incomplete(format!("cleanup: {problem}"));
    }
    let [passed, failed, incomplete] = counts;
    let _ = writeln!(
        out,
        "scenario {}: {passed} pass, {failed} fail, {incomplete} incomplete",
        scenario.name
    );
    worst
}
