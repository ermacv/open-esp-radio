//! Running a scenario's cases on a connected device and its verdict.

use std::{io::Write, time::Duration};

use oer_hil_link::{Boot, Link, LinkError};

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

/// Run `cases` on the device behind `link` after checking that its image
/// serves every service of `scenario`. A case that sees a reboot fails and
/// the next one runs on the new boot; a request without a response leaves
/// the boot's remaining cases incomplete.
pub fn cases(link: &Link, scenario: &Scenario, cases: &[&Case]) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    let mut boot = match ready(link, scenario) {
        Ok(boot) => boot,
        Err(reason) => {
            outcomes.push(Outcome::stand(reason));
            return outcomes;
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
            let mut cx = Cx::new(link, boot);
            let verdict = match (case.run)(&mut cx) {
                Ok(()) => Verdict::Pass,
                Err(Failure::Fail(reason)) => Verdict::Fail(reason),
                Err(Failure::Incomplete(reason)) => Verdict::Incomplete(reason),
            };
            unconfirmed = cx.unconfirmed;
            verdict
        };
        for note in link.take_notes() {
            eprintln!("device: {note}");
        }
        outcomes.push(Outcome {
            case: Some(case.name),
            verdict,
        });
        if oer_os_process::cancellation_requested() {
            outcomes.push(Outcome::stand(LinkError::Cancelled.to_string()));
            break;
        }
        match link.boot(HELLO_TIMEOUT) {
            Ok(next) if next.boot_id != boot.boot_id => {
                boot = next;
                unconfirmed = None;
            }
            Ok(_) => {}
            Err(error) => {
                outcomes.push(Outcome::stand(error.to_string()));
                break;
            }
        }
    }
    outcomes
}

/// The running boot, once its image serves every service of `scenario`.
fn ready(link: &Link, scenario: &Scenario) -> Result<Boot, String> {
    let boot = link.boot(HELLO_TIMEOUT).map_err(|error| error.to_string())?;
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

/// Print every outcome and the summary to `out`; the scenario's verdict:
/// a failure over an incomplete case over a pass.
pub fn report(scenario: &Scenario, outcomes: &[Outcome], out: &mut impl Write) -> Verdict {
    let mut counts = [0_usize; 3];
    let mut worst = Verdict::Pass;
    for outcome in outcomes {
        let name = outcome.case.unwrap_or("stand");
        let line = match &outcome.verdict {
            Verdict::Pass => format!("case {name}: pass"),
            Verdict::Fail(reason) => format!("case {name}: fail: {reason}"),
            Verdict::Incomplete(reason) => format!("case {name}: incomplete: {reason}"),
        };
        let _ = writeln!(out, "{line}");
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
    let [passed, failed, incomplete] = counts;
    let _ = writeln!(
        out,
        "scenario {}: {passed} pass, {failed} fail, {incomplete} incomplete",
        scenario.name
    );
    worst
}
