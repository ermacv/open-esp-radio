//! Running a scenario's cases on a connected device and its verdict.

use std::{
    io::Write,
    time::{Duration, Instant},
};

use oer_protocols_hil_protocol::base::{GetBootStatus, GetLinkHealth};
use oer_tools_hil_link::{Boot, Link, LinkError};

use crate::dut::{Device, Dut};
use crate::finish::{Cleanup, Ending};
use crate::{Case, Cx, DeviceCx, Failure, Role, Scenario, keys};

/// How long the running boot may take to say Hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the link may take to answer after a request got no response.
const LINK_PROBE_TIMEOUT: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 5000 });

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
    /// How the cases left each device, in the scenario's order of roles,
    /// for finishing.
    pub endings: Vec<Ending>,
}

/// Run `cases` on `duts`, one a role of `scenario` in its order, after
/// checking that each runs its built image and that the image serves every
/// service of its role, printing each outcome to `out` as it is known. A
/// case that sees a reboot fails and the next one runs on the new boot; a
/// request without a response leaves the remaining cases incomplete.
pub(crate) fn cases(
    duts: &mut [Dut<'_>],
    scenario: &Scenario,
    cases: &[&Case],
    out: &mut impl Write,
) -> Run {
    let roles = scenario.roles();
    let mut outcomes = Vec::new();
    let mut measurements = Vec::new();
    let mut record = |outcome: Outcome, out: &mut dyn Write| {
        print(&outcome, out);
        outcomes.push(outcome);
    };
    let unready =
        |record: &mut dyn FnMut(Outcome, &mut dyn Write), reason: String, out: &mut dyn Write| {
            record(Outcome::stand(reason), out);
        };
    let endings = |duts: &[Dut<'_>]| vec![Ending { unconfirmed: false }; duts.len()];
    if roles.is_empty() || roles.len() != duts.len() {
        unready(
            &mut record,
            format!(
                "the scenario has {} devices under test and the stand gave {}",
                roles.len(),
                duts.len()
            ),
            out,
        );
        let endings = endings(duts);
        return Run {
            outcomes,
            measurements,
            endings,
        };
    }
    let mut boots = Vec::with_capacity(duts.len());
    for (dut, role) in duts.iter().zip(roles) {
        let ready = match dut.link() {
            Some(link) => ready(link, role, dut.image_crc32()),
            None => Err("the device under test has no link".to_owned()),
        };
        match ready {
            Ok(boot) => boots.push(boot),
            Err(reason) => {
                unready(&mut record, device_reason(roles, role, reason), out);
                let endings = endings(duts);
                return Run {
                    outcomes,
                    measurements,
                    endings,
                };
            }
        }
    }
    let mut unconfirmed: Vec<Option<u32>> = vec![None; duts.len()];
    'cases: for case in cases {
        let mut lost: Vec<Option<u32>> = vec![None; duts.len()];
        let pending = roles
            .iter()
            .zip(&unconfirmed)
            .zip(&boots)
            .find_map(|((role, unconfirmed), boot)| unconfirmed.map(|id| (role, id, boot)));
        let verdict = if let Some((role, request_id, boot)) = pending {
            Verdict::Incomplete(device_reason(
                roles,
                role,
                format!(
                    "request {request_id} of boot {:016x} is unconfirmed",
                    boot.boot_id
                ),
            ))
        } else {
            let mut devices = duts
                .iter_mut()
                .zip(roles)
                .zip(boots.iter().cloned())
                .map(|((dut, role), boot)| DeviceCx::new(role.name, dut, boot));
            let (Some(first), others) = (devices.next(), devices.collect::<Vec<_>>()) else {
                break 'cases;
            };
            let mut cx = Cx::new(first, others);
            let verdict = match (case.run)(&mut cx) {
                Ok(()) => Verdict::Pass,
                Err(Failure::Fail(reason)) => Verdict::Fail(reason),
                Err(Failure::Incomplete(reason)) => Verdict::Incomplete(reason),
            };
            measurements.extend(cx.measurements.drain(..).map(|(name, value)| Measurement {
                case: case.name,
                name,
                value,
            }));
            for (index, device) in cx.devices().enumerate() {
                // A reset during the case moved it to a later boot.
                if let Some(boot) = boots.get_mut(index) {
                    *boot = device.boot;
                }
                if let Some(slot) = unconfirmed.get_mut(index) {
                    *slot = device.unconfirmed;
                }
                if let Some(slot) = lost.get_mut(index) {
                    *slot = device.unconfirmed;
                }
            }
            verdict
        };
        for (index, (dut, role)) in duts.iter().zip(roles).enumerate() {
            let Some(link) = dut.link() else {
                record(
                    Outcome {
                        case: Some(case.name),
                        verdict: verdict.clone(),
                    },
                    out,
                );
                record(
                    Outcome::stand(device_reason(
                        roles,
                        role,
                        "the device under test has no link".to_owned(),
                    )),
                    out,
                );
                break 'cases;
            };
            print_notes(link, roles, role);
            if let (Some(Some(request_id)), Some(boot)) = (lost.get(index), boots.get(index)) {
                print_link_probe(link, boot, *request_id, LINK_PROBE_TIMEOUT, out);
            }
        }
        record(
            Outcome {
                case: Some(case.name),
                verdict,
            },
            out,
        );
        if oer_tools_os_process::cancellation_requested() {
            record(Outcome::stand(LinkError::Cancelled.to_string()), out);
            break;
        }
        for (index, (dut, role)) in duts.iter().zip(roles).enumerate() {
            let (Some(link), Some(boot)) = (dut.link(), boots.get_mut(index)) else {
                break 'cases;
            };
            match link.boot(HELLO_TIMEOUT) {
                Ok(next) if next.boot_id != boot.boot_id => {
                    if next.hello.image_crc32 != dut.image_crc32() {
                        record(
                            Outcome::stand(device_reason(
                                roles,
                                role,
                                format!(
                                    "the board rebooted into image {:08x}, not the built {:08x}",
                                    next.hello.image_crc32,
                                    dut.image_crc32()
                                ),
                            )),
                            out,
                        );
                        break 'cases;
                    }
                    print_previous_boot(link, &next, out);
                    *boot = next;
                    if let Some(slot) = unconfirmed.get_mut(index) {
                        *slot = None;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    record(
                        Outcome::stand(device_reason(roles, role, error.to_string())),
                        out,
                    );
                    break 'cases;
                }
            }
        }
    }
    Run {
        outcomes,
        measurements,
        endings: unconfirmed
            .iter()
            .map(|unconfirmed| Ending {
                unconfirmed: unconfirmed.is_some(),
            })
            .collect(),
    }
}

/// `reason`, named by `role` when the scenario has several devices.
fn device_reason(roles: &[Role], role: &Role, reason: String) -> String {
    if roles.len() > 1 {
        format!("{}: {reason}", role.name)
    } else {
        reason
    }
}

/// The results file of a run: first the source its images were built from,
/// then the scenario, each device's role, image, board and build, every
/// outcome and every measurement.
#[derive(serde::Serialize)]
pub(crate) struct ResultsFile {
    #[serde(flatten)]
    pub(crate) source: crate::source::Source,
    #[serde(flatten)]
    pub(crate) rest: serde_json::Map<String, serde_json::Value>,
}

/// The results file of a run of `source`.
pub(crate) fn results(
    source: &crate::source::Source,
    scenario: &Scenario,
    boards: &[(String, oer_tools_image_bundle::BuildChoice)],
    outcomes: &[Outcome],
    measurements: &[Measurement],
) -> ResultsFile {
    ResultsFile {
        source: source.clone(),
        rest: run_results(scenario, boards, outcomes, measurements),
    }
}

fn run_results(
    scenario: &Scenario,
    boards: &[(String, oer_tools_image_bundle::BuildChoice)],
    outcomes: &[Outcome],
    measurements: &[Measurement],
) -> serde_json::Map<String, serde_json::Value> {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let devices = scenario
        .roles()
        .iter()
        .zip(boards)
        .map(|(role, (board, built))| {
            serde_json::json!({
                "role": role.name,
                "image": role.image,
                "board": board,
                "build": built,
            })
        })
        .collect::<Vec<_>>();
    let outcomes = outcomes
        .iter()
        .map(|outcome| {
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
        })
        .collect::<Vec<_>>();
    let measurements = measurements
        .iter()
        .map(|measurement| {
            serde_json::json!({
                "case": measurement.case,
                "name": measurement.name,
                "value": measurement.value,
            })
        })
        .collect::<Vec<_>>();
    [
        ("scenario", serde_json::Value::from(scenario.name)),
        ("devices", devices.into()),
        ("unix", unix.into()),
        ("outcomes", outcomes.into()),
        ("measurements", measurements.into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

/// Print the device's diagnostic lines and how many were lost, each named
/// by its role when the scenario has several devices.
fn print_notes(link: &Link, roles: &[Role], role: &Role) {
    let notes = link.take_notes();
    let name = if roles.len() > 1 { role.name } else { "device" };
    for note in notes.lines {
        eprintln!("{name}: {note}");
    }
    if notes.lost > 0 {
        eprintln!("{name}: {} diagnostic lines lost", notes.lost);
    }
}

/// Print to `out` whether the console of `boot` still answers after
/// `request_id` got no response, and how long it took: its base module
/// answers apart from the image's services, so an answer tells a stuck
/// service from a stopped console or a stalled line.
pub(crate) fn print_link_probe(
    link: &Link,
    boot: &Boot,
    request_id: u32,
    timeout: Duration,
    out: &mut dyn Write,
) {
    let asked = Instant::now();
    let line = match link.call(boot, GetLinkHealth, timeout) {
        Ok(health) => format!(
            "request {request_id}: the console answers after {:?}: {health:?}",
            asked.elapsed()
        ),
        Err(error) => format!("request {request_id}: the console does not answer: {error}"),
    };
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Print to `out` how the boot before `next` ended: its reset reason and
/// the record it kept for the next boot.
fn print_previous_boot(link: &Link, next: &Boot, out: &mut dyn Write) {
    let line = match link.call(next, GetBootStatus, HELLO_TIMEOUT) {
        Ok(evidence) => format!(
            "boot {:016x}: the previous boot ended with reset {:?} (raw {}), post-mortem {:?}, platform panic {:?}",
            next.boot_id,
            evidence.reset_reason,
            evidence.raw_reset_reason,
            evidence.post_mortem,
            evidence.platform_panic
        ),
        Err(error) => format!("boot {:016x}: no boot status: {error}", next.boot_id),
    };
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
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
/// that image serves every service of `role`.
fn ready(link: &Link, role: &Role, image_crc32: u32) -> Result<Boot, String> {
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
    let missing = keys::missing(role.services, &served);
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
