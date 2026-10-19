//! Finding, explaining and comparing the store's runs.
//!
//! These queries read typed bundles through [`Run`]; a value outside this
//! build's vocabulary makes a bundle unreadable here instead of silently
//! matching nothing. They never decide qualification.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle::store::Notes;
use oer_hil_run_bundle_format::run::FailureKind;
use oer_hil_run_bundle_format::run::Observations;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RunState;
use oer_hil_schema::run::RunEventKind;

use crate::{
    Result,
    run::Run,
    samples::{self, Sample},
};

/// Selection of `runs list`.
#[derive(Debug, Default)]
pub struct Filter {
    pub scenario: Option<String>,
    pub outcome: Option<String>,
    pub image: Option<String>,
    pub since_millis: Option<u64>,
}

impl Filter {
    pub fn matches(&self, run: &Run) -> bool {
        let scenario = self
            .scenario
            .as_ref()
            .is_none_or(|wanted| run.scenario(wanted).is_some());
        let outcome = self
            .outcome
            .as_ref()
            .is_none_or(|wanted| match &self.scenario {
                Some(scenario) => run
                    .scenario(scenario)
                    .is_some_and(|result| result.outcome.id() == wanted),
                None => run.status() == wanted,
            });
        scenario
            && outcome
            && self.image.as_ref().is_none_or(|wanted| {
                run.bundle.manifest().firmware.iter().any(|firmware| {
                    firmware.image.id() == wanted
                        || firmware.application_sha256.starts_with(wanted.as_str())
                })
            })
            && self
                .since_millis
                .is_none_or(|since| run.started_millis() >= since)
    }
}

/// `millis` as a local date and time.
pub(crate) fn date(millis: u64) -> String {
    let seconds = millis / 1000;
    let output = oer_os_process::command("date")
        .args(["-d", &format!("@{seconds}"), "+%Y-%m-%d %H:%M"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| seconds.to_string())
}

fn checkout(run: &Run) -> String {
    run.bundle.checkout().unwrap_or_else(|| String::from("?"))
}

/// One line per run.
pub fn list_line(run: &Run) -> String {
    let scenarios = run
        .scenarios()
        .iter()
        .map(|scenario| format!("{}={}", scenario.scenario, scenario.outcome))
        .collect::<Vec<_>>()
        .join(" ");
    let images = run
        .bundle
        .manifest()
        .firmware
        .iter()
        .map(|firmware| {
            let sha = &firmware.application_sha256;
            format!("{}:{}", firmware.image.id(), &sha[..sha.len().min(8)])
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{} {} {:<10} {:<22} {:<10} [{}] {}",
        run.id(),
        date(run.started_millis()),
        run.status(),
        checkout(run),
        run.short_commit(),
        images,
        scenarios
    )
}

/// A log line with control and replacement characters of raw binary output
/// shown as `·`.
fn printable(line: &str) -> String {
    line.chars()
        .map(|c| {
            if c == '\u{fffd}' || (c.is_control() && c != '\t') {
                '·'
            } else {
                c
            }
        })
        .collect()
}

/// A repetition's typed results in one line each: the claim the workload
/// makes and the names of the observations it recorded.
fn observation_summary(observations: &Observations) -> String {
    let mut text = String::new();
    if let Some(claim) = &observations.claim {
        text.push_str(&format!("      claim: {}", claim.result));
        if !claim.not_proven.is_empty() {
            text.push_str(&format!(" (not proven: {})", claim.not_proven.join(", ")));
        }
        text.push('\n');
    }
    if !observations.observations.is_empty() {
        let names = observations
            .observations
            .iter()
            .map(|observation| observation.name.as_str())
            .collect::<Vec<_>>();
        text.push_str(&format!("      observations: {}\n", names.join(", ")));
    }
    text
}

/// Why a run did not pass: per scenario and repetition, the failure, the
/// measurements that missed their criteria, the workload's claim and
/// observations, cleanup failures, host USB events, and where to look next.
pub fn why(run: &Run, tail_lines: usize) -> String {
    let mut text = format!(
        "run {} ({}) from {} at {}: {}\n",
        run.id(),
        date(run.started_millis()),
        checkout(run),
        run.short_commit(),
        run.status()
    );
    if run.scenarios().is_empty() {
        text.push_str(&format!(
            "  no suite result; the run ended in state `{}`; see {}\n",
            if run.abandoned {
                "abandoned"
            } else {
                run.state().id()
            },
            run.directory().join("events.jsonl").display()
        ));
    }
    for scenario in run.scenarios() {
        let image = scenario.image.id();
        if scenario.outcome.is_passed() {
            text.push_str(&format!("  {} [{image}]: passed\n", scenario.scenario));
            continue;
        }
        text.push_str(&format!(
            "  {} [{image}]: {}\n",
            scenario.scenario, scenario.outcome
        ));
        if let Some(failure) = &scenario.failure {
            text.push_str(&format!("    failure ({}):\n", failure.kind));
            for line in failure.message.lines() {
                text.push_str(&format!("      {}\n", printable(line)));
            }
        }
        for repetition in &scenario.repetitions {
            if repetition.outcome.is_passed() {
                continue;
            }
            let missed = repetition
                .measurements
                .iter()
                .filter(|m| m.verdict.is_some_and(|verdict| !verdict.is_passed()))
                .collect::<Vec<_>>();
            let cause = match (
                repetition.failure.as_ref().map(|f| f.kind),
                missed.is_empty(),
            ) {
                (_, false) => "criterion: measured values missed their acceptance criteria",
                (Some(FailureKind::Infrastructure), true) => "infrastructure fault",
                (Some(FailureKind::Precondition), true) => "precondition not met",
                (Some(FailureKind::ImageBuild | FailureKind::ImageFlash), true) => {
                    "image build or flash"
                }
                (Some(FailureKind::Scenario), true) => "scenario failure",
                (Some(FailureKind::Hang), true) => "the target hung; its watchdog reset it",
                (Some(FailureKind::UnexpectedReset), true) => {
                    "the target reset for a reason the runner did not cause"
                }
                (None, true) => "no recorded failure",
            };
            text.push_str(&format!(
                "    repetition {}: {} — {cause}\n",
                repetition.repetition, repetition.outcome
            ));
            if let Some(failure) = &repetition.failure {
                text.push_str(&format!(
                    "      failure ({}): {}\n",
                    failure.kind, failure.message
                ));
            }
            for measurement in missed {
                text.push_str(&format!(
                    "      missed {}: {} {} (criterion {}, verdict failed)\n",
                    measurement.name,
                    measurement.value,
                    measurement.unit,
                    measurement
                        .threshold
                        .as_ref()
                        .map_or_else(|| String::from("—"), ToString::to_string),
                ));
            }
            match run.bundle.cleanup(repetition) {
                Ok(records) => {
                    for failure in records.iter().filter_map(|record| record.failure.as_ref()) {
                        text.push_str(&format!("      cleanup failed: {failure}\n"));
                    }
                }
                Err(error) => text.push_str(&format!("      cleanup record unreadable: {error}\n")),
            }
            match run.bundle.observations(repetition) {
                Ok(Some(observations)) => text.push_str(&observation_summary(&observations)),
                Ok(None) => {}
                Err(error) => text.push_str(&format!("      observations unreadable: {error}\n")),
            }
            // Whether the host saw a board's USB bridge drop off the bus.
            match run.bundle.usb_events(repetition) {
                Ok(events) => {
                    for event in events {
                        text.push_str(&format!(
                            "      host {event} at +{:.3} s of the run\n",
                            (event.realtime_micros / 1000).saturating_sub(run.started_millis())
                                as f64
                                / 1000.0
                        ));
                    }
                }
                Err(error) => {
                    text.push_str(&format!("      host USB events unreadable: {error}\n"));
                }
            }
            let directory = run.bundle.repetition_directory(repetition);
            text.push_str(&format!("      artifacts: {}\n", directory.display()));
            let log = directory.join("uart.log");
            if tail_lines > 0
                && let Ok(content) = fs::read_to_string(&log)
            {
                let lines = content.lines().collect::<Vec<_>>();
                text.push_str(&format!(
                    "      last {} lines of uart.log:\n",
                    tail_lines.min(lines.len())
                ));
                for line in &lines[lines.len().saturating_sub(tail_lines)..] {
                    text.push_str(&format!("        | {}\n", printable(line)));
                }
            }
        }
    }
    text
}

/// Where a run's artifacts live: the bundle directory in the store, its
/// reports, and per scenario and repetition the outcome and the directory
/// with the files it holds.
pub fn show(run: &Run) -> String {
    let directory =
        fs::canonicalize(run.directory()).unwrap_or_else(|_| run.directory().to_owned());
    let mut text = format!(
        "run {} ({}) from {} at {}: {}\n  directory: {}\n",
        run.id(),
        date(run.started_millis()),
        checkout(run),
        run.short_commit(),
        run.status(),
        directory.display()
    );
    for report in ["manifest.json", "suite.json", "events.jsonl", "report.html"] {
        if directory.join(report).exists() {
            text.push_str(&format!(
                "  {report}: {}\n",
                directory.join(report).display()
            ));
        }
    }
    for scenario in run.scenarios() {
        text.push_str(&format!(
            "  {} [{}]: {}\n",
            scenario.scenario,
            scenario.image.id(),
            scenario.outcome
        ));
        if scenario.repetitions.is_empty() {
            continue;
        }
        for repetition in &scenario.repetitions {
            let path = directory.join(&repetition.artifact_directory);
            text.push_str(&format!(
                "    repetition {}: {} {}\n",
                repetition.repetition,
                repetition.outcome,
                path.display()
            ));
            let mut entries = fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.path().is_dir() {
                        name + "/"
                    } else {
                        name
                    }
                })
                .collect::<Vec<_>>();
            entries.sort();
            if !entries.is_empty() {
                text.push_str(&format!("      {}\n", entries.join("  ")));
            }
        }
    }
    text
}

/// The samples of `scenario` in `run`, by measurement name; an error when
/// the run reports incompatible metrics ([`samples::samples`]).
fn samples_of(run: &Run, scenario: &str) -> Result<BTreeMap<String, Sample>> {
    Ok(run
        .suite
        .as_ref()
        .map(samples::samples)
        .transpose()
        .map_err(|error| format!("run {}: {error}", run.id()))?
        .unwrap_or_default()
        .into_iter()
        .filter(|sample| sample.metric.scenario == scenario)
        .map(|sample| (sample.metric.name.clone(), sample))
        .collect())
}

/// The measurements of two runs side by side, for their common scenarios:
/// each side's repetitions compared with the one noise-aware
/// [`samples::compare`]. A measurement the two runs report as different
/// metrics (unit, semantics or direction) is shown but never judged.
pub fn compare(a: &Run, b: &Run, filter: Option<&str>) -> Result<String> {
    let mut text = format!(
        "A {} {} {}\nB {} {} {}\n",
        a.id(),
        a.short_commit(),
        a.status(),
        b.id(),
        b.short_commit(),
        b.status()
    );
    let scenarios = a
        .scenarios()
        .iter()
        .map(|s| s.scenario.clone())
        .filter(|id| b.scenario(id).is_some())
        .collect::<BTreeSet<_>>();
    if scenarios.is_empty() {
        text.push_str("no common scenario\n");
    }
    for scenario in scenarios {
        let side = |run: &Run| {
            run.scenario(&scenario)
                .map_or(("—", 0), |s| (s.outcome.id(), s.repetitions.len()))
        };
        let ((outcome_a, repetitions_a), (outcome_b, repetitions_b)) = (side(a), side(b));
        text.push_str(&format!(
            "{scenario}: A {outcome_a} ({repetitions_a} repetitions), B {outcome_b} \
             ({repetitions_b} repetitions)\n"
        ));
        let (left, right) = (samples_of(a, &scenario)?, samples_of(b, &scenario)?);
        for name in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
            if filter.is_some_and(|filter| !name.contains(filter)) {
                continue;
            }
            let (x, y) = (left.get(name), right.get(name));
            let unit = x
                .or(y)
                .map_or(String::new(), |sample| sample.metric.unit.to_string());
            let mean = |sample: Option<&Sample>| {
                sample
                    .and_then(Sample::spread)
                    .map_or_else(|| String::from("—"), |spread| format!("{:.3}", spread.mean))
            };
            let judged = match (x, y) {
                (Some(x), Some(y)) if x.metric.incompatibility(&y.metric).is_some() => x
                    .metric
                    .incompatibility(&y.metric)
                    .map(|reason| format!("not comparable: {reason}")),
                (Some(x), Some(y)) => {
                    samples::compare(x.better(), &x.values, &y.values).map(|compared| {
                        let relative = if compared.a.mean != 0.0 {
                            format!(
                                "{:+.1}%",
                                compared.difference / compared.a.mean.abs() * 100.0
                            )
                        } else {
                            format!("{:+}", compared.difference)
                        };
                        format!("{relative} {}", compared.verdict)
                    })
                }
                _ => None,
            }
            .unwrap_or_else(|| String::from("—"));
            text.push_str(&format!(
                "  {name:<60} {:>14} {:>14} {unit:<8} {judged}\n",
                mean(x),
                mean(y)
            ));
        }
    }
    Ok(text)
}

/// Outcome and measurement means of `scenario` across runs, oldest first:
/// every measurement, or with `filter` those whose name contains it. Each
/// metric identity is its own column: a name whose unit, semantics or
/// direction changed between runs never continues one column.
pub fn history(runs: &[Run], scenario: &str, filter: Option<&str>) -> Result<String> {
    let mut text = String::new();
    let mut names = BTreeSet::new();
    let mut rows = Vec::new();
    for run in runs {
        let Some(result) = run.scenario(scenario) else {
            continue;
        };
        let values = samples_of(run, scenario)?
            .into_iter()
            .filter(|(name, _)| filter.is_none_or(|filter| name.contains(filter)))
            .filter_map(|(_, sample)| {
                let label = format!(
                    "{} [{}, semantics {}, better {}]",
                    sample.metric.name,
                    sample.metric.unit,
                    sample.metric.semantics,
                    sample
                        .metric
                        .better
                        .map_or("undeclared", samples::Better::id)
                );
                Some((label, sample.spread()?.mean))
            })
            .collect::<BTreeMap<_, _>>();
        names.extend(values.keys().cloned());
        rows.push((run, result, values));
    }
    if rows.is_empty() {
        return Ok(format!("no run contains {scenario}\n"));
    }
    for name in &names {
        text.push_str(&format!("  column {name}\n"));
    }
    for (run, result, values) in rows {
        let columns = names
            .iter()
            .map(|name| {
                values
                    .get(name)
                    .map_or_else(|| String::from("—"), |value| format!("{value:.3}"))
            })
            .collect::<Vec<_>>()
            .join("  ");
        let failure = result
            .repetitions
            .iter()
            .find_map(|r| r.failure.as_ref().map(|failure| failure.message.clone()))
            .map(|message| format!("  {}", message.chars().take(80).collect::<String>()))
            .unwrap_or_default();
        text.push_str(&format!(
            "{} {} {:<22} {:<10} {:<8} {columns}{failure}\n",
            run.id(),
            date(run.started_millis()),
            checkout(run),
            run.short_commit(),
            result.outcome,
        ));
    }
    Ok(text)
}

/// How stable `scenario` is over `runs`: its pass rate and how many of its
/// newest runs in a row did not pass.
pub fn stability(runs: &[Run], scenario: &str) -> String {
    let outcomes = runs
        .iter()
        .filter_map(|run| run.scenario(scenario))
        .map(|result| result.outcome)
        .collect::<Vec<_>>();
    let passed = outcomes
        .iter()
        .filter(|outcome| outcome.is_passed())
        .count();
    let streak = outcomes
        .iter()
        .rev()
        .take_while(|outcome| !outcome.is_passed())
        .count();
    format!(
        "{scenario}: {passed} of {} runs passed ({:.0}%), {streak} newest in a row did not pass\n",
        outcomes.len(),
        100.0 * passed as f64 / outcomes.len().max(1) as f64
    )
}

/// How one scenario's repetitions ended across runs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stability {
    pub scenario: String,
    pub passed: usize,
    /// Failures of the code under test.
    pub failed: usize,
    /// Failures of the stand: broken or blocked repetitions.
    pub stand: usize,
    /// The newest failure's message.
    pub last_failure: Option<String>,
}

impl Stability {
    pub fn repetitions(&self) -> usize {
        self.passed + self.failed + self.stand
    }

    /// Whether the scenario both passed and did not: its result depends on
    /// more than the code under test.
    pub fn is_flaky(&self) -> bool {
        self.passed > 0 && self.failed + self.stand > 0
    }
}

/// Each scenario's repetitions across `runs`, oldest first, that ended as
/// passed, failed, broken or blocked.
pub fn stabilities(runs: &[Run]) -> Vec<Stability> {
    let mut by_scenario: BTreeMap<String, Stability> = BTreeMap::new();
    for run in runs {
        for scenario in run.scenarios() {
            let entry = by_scenario
                .entry(scenario.scenario.clone())
                .or_insert_with(|| Stability {
                    scenario: scenario.scenario.clone(),
                    ..Stability::default()
                });
            for repetition in &scenario.repetitions {
                match repetition.outcome {
                    Outcome::Passed => entry.passed += 1,
                    Outcome::Failed => entry.failed += 1,
                    Outcome::Broken | Outcome::Blocked => entry.stand += 1,
                    Outcome::Skipped | Outcome::Interrupted | Outcome::BoardQuarantined => {
                        continue;
                    }
                }
                if let Some(failure) = &repetition.failure {
                    entry.last_failure = Some(failure.message.clone());
                }
            }
        }
    }
    by_scenario.into_values().collect()
}

/// The scenarios of `stabilities` with at least `minimum` repetitions that
/// did not always pass, least passing first, with `quarantined` marked.
pub fn flaky_report(stabilities: &[Stability], minimum: usize, quarantined: &Notes) -> String {
    let mut listed = stabilities
        .iter()
        .filter(|entry| entry.repetitions() >= minimum && entry.failed + entry.stand > 0)
        .collect::<Vec<_>>();
    listed.sort_by(|a, b| {
        (a.passed * b.repetitions())
            .cmp(&(b.passed * a.repetitions()))
            .then_with(|| a.scenario.cmp(&b.scenario))
    });
    let mut report = String::new();
    for entry in listed {
        let rate = 100 * entry.passed / entry.repetitions().max(1);
        report.push_str(&format!(
            "{:>3}% {} ({} of {} passed; {} failed, {} stand){}{}\n",
            rate,
            entry.scenario,
            entry.passed,
            entry.repetitions(),
            entry.failed,
            entry.stand,
            if entry.is_flaky() { ", flaky" } else { "" },
            if quarantined.contains_key(&entry.scenario) {
                ", quarantined"
            } else {
                ""
            },
        ));
        if let Some(message) = &entry.last_failure {
            let message = message.lines().next().unwrap_or_default();
            report.push_str(&format!(
                "      last: {}\n",
                message.chars().take(120).collect::<String>()
            ));
        }
    }
    if report.is_empty() {
        report.push_str("every scenario with enough repetitions passed each time\n");
    }
    report
}

/// How long a stored observer build no run names yet is kept: a starting
/// run stores its build before its manifest names it.
pub const OBSERVER_GRACE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Remove the observer builds of `store` that none of its runs names;
/// returns how many were removed.
pub fn collect_observers(store: &RunStore) -> Result<usize> {
    let named = store
        .bundles()?
        .iter()
        .filter_map(|bundle| {
            bundle
                .manifest()
                .observer()
                .filter(|record| {
                    record["schema"] == oer_hil_run_bundle_format::observer::store::REFERENCED
                })
                .and_then(oer_hil_run_bundle_format::observer::store::build_digest)
                .map(str::to_owned)
        })
        .collect();
    Ok(oer_hil_run_bundle_format::observer::store::collect_garbage(
        store.observers(),
        &named,
        OBSERVER_GRACE,
    )
    .map_err(|error| error.to_string())?
    .len())
}

/// Follow the run in `directory`, handing `report` each step its events
/// record, until it ends; the exit status for its outcome: 0 passed, 1
/// failed, broken, blocked or skipped, 2 interrupted, abandoned or on a
/// quarantined board.
pub fn wait(directory: &Path, mut report: impl FnMut(String)) -> Result<u8> {
    let mut seen = 0;
    loop {
        let run = Run::load(directory);
        if let Some(run) = &run {
            let events = run.bundle.events()?;
            for event in &events[seen.min(events.len())..] {
                report(format!(
                    "{} {}{}{}",
                    event.timestamp_unix_millis / 1000,
                    event.kind.id(),
                    event
                        .scenario
                        .as_deref()
                        .map(|scenario| format!(" {scenario}"))
                        .unwrap_or_default(),
                    event
                        .outcome
                        .map(|outcome| format!(" {}", outcome.id()))
                        .unwrap_or_default()
                ));
                if matches!(
                    event.kind,
                    RunEventKind::RunFinished | RunEventKind::RunInterrupted
                ) {
                    return Ok(exit_status(
                        Run::load(directory).and_then(|run| run.outcome()),
                    ));
                }
            }
            seen = events.len();
            // A runner that died leaves its run running forever.
            if run.abandoned {
                report(String::from("abandoned: the run's runner is gone"));
                return Ok(2);
            }
            // A finished run ends waiting even when its events stop short.
            if run.state() != RunState::Running {
                report(format!(
                    "{}: {}",
                    run.state().id(),
                    run.outcome().map_or("no outcome", Outcome::id)
                ));
                return Ok(exit_status(run.outcome()));
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// The exit status [`wait`] reports for a run's `outcome`.
pub fn exit_status(outcome: Option<Outcome>) -> u8 {
    match outcome {
        Some(Outcome::Passed) => 0,
        Some(Outcome::Failed | Outcome::Broken | Outcome::Blocked | Outcome::Skipped) => 1,
        Some(Outcome::Interrupted | Outcome::BoardQuarantined) | None => 2,
    }
}

#[cfg(test)]
pub(crate) mod tests;
