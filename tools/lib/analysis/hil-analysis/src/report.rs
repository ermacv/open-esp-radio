//! JUnit and HTML projections of the canonical run record, which a run
//! seals beside its suite. They are views: no reader decides an outcome
//! from them.

use std::fmt::Write as _;

use oer_hil_run_bundle::run::Views;
use oer_hil_run_bundle_format::run::MeasurementVerdict;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RepetitionResult;
use oer_hil_run_bundle_format::run::RunManifest;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_run_bundle_format::run::SuiteResult;

/// Both views of a sealed run; the renderer the runner hands
/// [`oer_hil_run_bundle::run::RunSession::finish`].
pub fn views(suite: &SuiteResult, manifest: &RunManifest) -> Views {
    Views {
        junit: junit(suite, manifest),
        html: html(suite, manifest),
    }
}

pub fn junit(suite: &SuiteResult, manifest: &RunManifest) -> String {
    let mut tests = 0_usize;
    let mut failures = 0_usize;
    let mut errors = 0_usize;
    let mut skipped = 0_usize;
    for scenario in &suite.scenarios {
        if scenario.repetitions.is_empty() {
            tests += 1;
            match scenario.outcome {
                Outcome::Failed => failures += 1,
                Outcome::Broken
                | Outcome::Blocked
                | Outcome::Interrupted
                | Outcome::BoardQuarantined => errors += 1,
                Outcome::Skipped => skipped += 1,
                Outcome::Passed => {}
            }
            continue;
        }
        for repetition in &scenario.repetitions {
            tests += 1;
            match repetition.outcome {
                Outcome::Failed => failures += 1,
                Outcome::Broken
                | Outcome::Blocked
                | Outcome::Interrupted
                | Outcome::BoardQuarantined => errors += 1,
                Outcome::Skipped => skipped += 1,
                Outcome::Passed => {}
            }
        }
    }

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        xml,
        "<testsuites name=\"open-esp-radio-hil\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{:.3}\">",
        suite.duration_millis as f64 / 1_000.0
    );
    let _ = writeln!(
        xml,
        "  <testsuite name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{:.3}\">",
        xml_escape(&suite.target),
        suite.duration_millis as f64 / 1_000.0
    );
    xml.push_str("    <properties>\n");
    let _ = writeln!(
        xml,
        "      <property name=\"run_id\" value=\"{}\"/>",
        xml_escape(&suite.run_id)
    );
    let _ = writeln!(
        xml,
        "      <property name=\"git_commit\" value=\"{}\"/>",
        xml_escape(&manifest.repository.commit)
    );
    let _ = writeln!(
        xml,
        "      <property name=\"git_dirty\" value=\"{}\"/>",
        manifest.repository.dirty
    );
    let _ = writeln!(
        xml,
        "      <property name=\"workspace_sha256\" value=\"{}\"/>",
        manifest.repository.workspace_sha256
    );
    let _ = writeln!(
        xml,
        "      <property name=\"firmware_replayed_from\" value=\"{}\"/>",
        xml_escape(&firmware_replay_summary(manifest))
    );
    let _ = writeln!(
        xml,
        "      <property name=\"cell_id\" value=\"{}\"/>",
        xml_escape(&manifest.cell.cell_id)
    );
    let _ = writeln!(
        xml,
        "      <property name=\"device_id\" value=\"{}\"/>",
        xml_escape(&manifest.cell.device_id)
    );
    xml.push_str("    </properties>\n");
    for scenario in &suite.scenarios {
        if scenario.repetitions.is_empty() {
            render_junit_case(&mut xml, scenario, None);
            continue;
        }
        for repetition in &scenario.repetitions {
            render_junit_case(&mut xml, scenario, Some(repetition));
        }
    }
    xml.push_str("  </testsuite>\n</testsuites>\n");
    xml
}

fn render_junit_case(
    xml: &mut String,
    scenario: &ScenarioResult,
    repetition: Option<&RepetitionResult>,
) {
    let name = repetition.map_or_else(
        || scenario.scenario.clone(),
        |repetition| {
            format!(
                "{}[repetition-{:03}]",
                scenario.scenario, repetition.repetition
            )
        },
    );
    let outcome = repetition.map_or(scenario.outcome, |repetition| repetition.outcome);
    let duration_millis = repetition.map_or(0, |repetition| repetition.duration_millis);
    let failure = repetition.map_or(scenario.failure.as_ref(), |repetition| {
        repetition.failure.as_ref()
    });
    let _ = writeln!(
        xml,
        "    <testcase classname=\"hil.{}\" name=\"{}\" time=\"{:.3}\">",
        scenario.image.id(),
        xml_escape(&name),
        duration_millis as f64 / 1_000.0
    );
    let message = failure.map_or("", |failure| failure.message.as_str());
    match outcome {
        Outcome::Passed => {}
        Outcome::Failed => {
            let kind = failure.map_or("scenario", |failure| failure.kind.id());
            let _ = writeln!(
                xml,
                "      <failure type=\"{}\" message=\"{}\">{}</failure>",
                xml_escape(kind),
                xml_escape(message),
                xml_escape(message)
            );
        }
        Outcome::Broken | Outcome::Blocked | Outcome::Interrupted | Outcome::BoardQuarantined => {
            let kind = failure.map_or("infrastructure", |failure| failure.kind.id());
            let _ = writeln!(
                xml,
                "      <error type=\"{}\" message=\"{}\">{}</error>",
                xml_escape(kind),
                xml_escape(message),
                xml_escape(message)
            );
        }
        Outcome::Skipped => {
            let _ = writeln!(xml, "      <skipped message=\"{}\"/>", xml_escape(message));
        }
    }
    let mut system_output = String::new();
    if let Some(repetition) = repetition {
        let _ = writeln!(
            system_output,
            "artifacts={}",
            repetition.artifact_directory.display()
        );
        for measurement in &repetition.measurements {
            let _ = writeln!(
                system_output,
                "measurement.{}={} {}",
                measurement.name,
                measurement.value,
                measurement.unit.id(),
            );
        }
    }
    if !system_output.is_empty() {
        let _ = writeln!(
            xml,
            "      <system-out>{}</system-out>",
            xml_escape(system_output.trim_end())
        );
    }
    xml.push_str("    </testcase>\n");
}

pub fn html(suite: &SuiteResult, manifest: &RunManifest) -> String {
    let mut rows = String::new();
    for scenario in &suite.scenarios {
        let detail = scenario.failure.as_ref().map_or_else(
            || {
                let failures = scenario
                    .repetitions
                    .iter()
                    .filter_map(|entry| entry.failure.as_ref())
                    .map(|failure| failure.message.as_str())
                    .collect::<Vec<_>>();
                failures.join("; ")
            },
            |failure| failure.message.clone(),
        );
        let attachments = scenario
            .repetitions
            .iter()
            .flat_map(|repetition| repetition.attachments.iter())
            .map(|attachment| {
                let path = attachment.path.display().to_string();
                format!(
                    "<a href=\"{}\">{}</a>",
                    html_escape(&path),
                    html_escape(
                        &attachment
                            .path
                            .file_name()
                            .unwrap_or(attachment.path.as_os_str())
                            .to_string_lossy(),
                    )
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let measurements = scenario
            .repetitions
            .iter()
            .filter(|repetition| !repetition.measurements.is_empty())
            .map(measurement_details)
            .collect::<Vec<_>>()
            .join("");
        let _ = writeln!(
            rows,
            "<tr><td>{}</td><td>{}</td><td class=\"{}\">{:?}</td><td>{}/{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&scenario.scenario),
            scenario.image.id(),
            if scenario.outcome.is_passed() {
                "pass"
            } else {
                "fail"
            },
            scenario.outcome,
            scenario
                .repetitions
                .iter()
                .filter(|entry| entry.outcome.is_passed())
                .count(),
            scenario.required_repetitions,
            html_escape(&detail),
            if attachments.is_empty() {
                "&mdash;"
            } else {
                &attachments
            },
            if measurements.is_empty() {
                "&mdash;"
            } else {
                &measurements
            },
        );
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>HIL {run}</title>\
         <style>body{{font:14px system-ui,sans-serif;margin:2rem;max-width:1200px}}\
         table{{border-collapse:collapse;width:100%}}th,td{{border:1px solid #bbb;padding:.45rem;text-align:left}}\
         .pass{{color:#087830;font-weight:700}}.fail{{color:#b42318;font-weight:700}}code{{background:#eee;padding:.1rem .25rem}}</style>\
         </head><body><h1>Open ESP radio HIL</h1>\
         <p>Run <code>{run}</code> · cell <code>{cell}</code> · device <code>{device}</code> · commit <code>{commit}</code>{dirty} · firmware {firmware}</p>\
         <p>Outcome: <strong class=\"{class}\">{outcome:?}</strong> · {passed}/{total} scenarios passed · {duration:.3} s</p>\
         <table><thead><tr><th>Scenario</th><th>Image</th><th>Outcome</th><th>Repetitions</th><th>Failure</th><th>Artifacts</th><th>Measurements</th></tr></thead>\
         <tbody>{rows}</tbody></table></body></html>\n",
        run = html_escape(&suite.run_id),
        cell = html_escape(&manifest.cell.cell_id),
        device = html_escape(&manifest.cell.device_id),
        commit = html_escape(&manifest.repository.commit),
        dirty = if manifest.repository.dirty {
            " · dirty workspace"
        } else {
            ""
        },
        firmware = html_escape(&firmware_replay_summary(manifest)),
        class = if suite.outcome.is_passed() {
            "pass"
        } else {
            "fail"
        },
        outcome = suite.outcome,
        passed = suite.counts.passed,
        total = suite.counts.scenarios,
        duration = suite.duration_millis as f64 / 1_000.0,
    )
}

fn measurement_details(repetition: &RepetitionResult) -> String {
    let mut rows = String::new();
    let mut failed = 0;
    for measurement in &repetition.measurements {
        let threshold = measurement.threshold.map_or_else(String::new, |threshold| {
            format!(
                " ({} {} {})",
                threshold.comparison.symbol(),
                threshold.value,
                measurement.unit.id()
            )
        });
        let class = if measurement.verdict == Some(MeasurementVerdict::Failed) {
            failed += 1;
            "fail"
        } else {
            ""
        };
        let _ = write!(
            rows,
            "<span class=\"{class}\"><code>{}</code>={} {}{threshold}</span><br>",
            html_escape(&measurement.name),
            measurement.value,
            measurement.unit.id()
        );
    }
    format!(
        "<details><summary>Repetition {}: {:?} · {} measurements · {failed} failed criteria</summary>{rows}</details>",
        repetition.repetition,
        repetition.outcome,
        repetition.measurements.len()
    )
}

fn firmware_replay_summary(manifest: &RunManifest) -> String {
    let origins = manifest
        .firmware
        .iter()
        .filter_map(|artifact| {
            artifact
                .replayed_from
                .as_ref()
                .map(|origin| format!("{}:{}", artifact.image.id(), origin.source_run_id))
        })
        .collect::<Vec<_>>();
    if origins.is_empty() {
        String::from("current-build")
    } else {
        format!("replay({})", origins.join(","))
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn html_escape(value: &str) -> String {
    xml_escape(value)
}

#[cfg(test)]
mod tests;
