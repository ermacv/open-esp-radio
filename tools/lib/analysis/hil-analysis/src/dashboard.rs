//! The runs part of the stand's live page: the newest runs and where a
//! running one is.

use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle_format::RunBundle;
use oer_hil_run_bundle_format::run::PlanDisposition;
use oer_hil_run_bundle_format::run::RunState;
use oer_hil_schema::run::RunEventKind;
use serde_json::{Value, json};

use crate::run::Run;

/// The `count` newest runs of `store`, newest first. Run directories start
/// with their start time, so only those are read.
pub fn newest_runs(store: &RunStore, count: usize) -> Vec<Value> {
    store
        .ids_newest_first()
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name.starts_with(|character: char| character.is_ascii_digit()))
        .filter_map(|name| Run::load(&store.run(&name)))
        .take(count)
        .map(|run| {
            let manifest = run.bundle.manifest();
            json!({
                "id": run.id(),
                "started_millis": run.started_millis(),
                "state": if run.abandoned { "abandoned" } else { run.state().id() },
                "outcome": run.outcome(),
                "commit": Some(run.commit()).filter(|commit| !commit.is_empty())
                    .map(|commit| &commit[..commit.len().min(12)]),
                "dirty": run.dirty(),
                "checkout": run.bundle.checkout(),
                "scenarios": run.scenarios().iter().map(|scenario| json!({
                    "id": scenario.scenario,
                    "outcome": scenario.outcome,
                })).collect::<Vec<_>>(),
                "progress": (run.state() == RunState::Running && !run.abandoned)
                    .then(|| progress(&run.bundle))
                    .flatten(),
                // A run still marked running whose runner is gone ended
                // without sealing its bundle.
                "abandoned": run.abandoned,
                "report": run.directory().join("report.html").is_file(),
                "experiment_arm": manifest.experiment.as_ref().map(|experiment| experiment.arm),
            })
        })
        .collect()
}

/// Where a running run is: its latest step, the scenario it is in, how many
/// of its planned scenarios finished, the planned scenarios in plan order and
/// the latest outcome of each finished one (`blocked` for a blocked one).
/// `None` when its events or plan cannot be read or use a vocabulary this
/// build does not know.
pub fn progress(bundle: &RunBundle) -> Option<Value> {
    let events = bundle.events().ok()?;
    let plan = bundle.plan().ok()??;
    let mut scenarios = Vec::new();
    for entry in &plan.entries {
        if entry.disposition == PlanDisposition::Selected && !scenarios.contains(&entry.scenario) {
            scenarios.push(entry.scenario.clone());
        }
    }
    let planned = plan
        .entries
        .iter()
        .filter(|entry| entry.disposition == PlanDisposition::Selected)
        .count();
    let finished = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                RunEventKind::ScenarioFinished | RunEventKind::ScenarioBlocked
            )
        })
        .count();
    let mut outcomes = serde_json::Map::new();
    for event in &events {
        let outcome = match event.kind {
            RunEventKind::ScenarioFinished => match event.outcome {
                Some(outcome) => json!(outcome),
                None => continue,
            },
            RunEventKind::ScenarioBlocked => json!("blocked"),
            _ => continue,
        };
        if let Some(scenario) = &event.scenario {
            outcomes.insert(scenario.clone(), outcome);
        }
    }
    let latest = events.last()?;
    let current = events
        .iter()
        .rev()
        .find(|event| event.kind == RunEventKind::ScenarioStarted)
        .filter(|started| {
            !events.iter().any(|event| {
                event.kind == RunEventKind::ScenarioFinished
                    && event.scenario == started.scenario
                    && event.timestamp_unix_millis >= started.timestamp_unix_millis
            })
        })
        .and_then(|started| started.scenario.clone());
    Some(json!({
        "step": latest.kind.id(),
        "step_since_millis": latest.timestamp_unix_millis,
        "scenario": current,
        "finished": finished,
        "planned": planned,
        "scenarios": scenarios,
        "outcomes": outcomes,
    }))
}

#[cfg(test)]
mod tests;
