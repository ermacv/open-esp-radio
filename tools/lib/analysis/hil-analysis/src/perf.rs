//! Performance of gated HIL measurements across commits.
//!
//! A scenario's gated measurements (those with an `at-least` or `at-most`
//! threshold, such as UDP rates) are its performance figures; the threshold
//! also fixes which direction is better. This module summarizes them per
//! commit from the run store, keeps one reviewed baseline per scenario in
//! the store's [`Sidecar::PerfBaselines`] and reports a run whose figures
//! moved the wrong way by more than the baseline's noise. It reads sealed
//! bundles only and never decides qualification.

use std::{collections::BTreeMap, fs};

use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle::store::Sidecar;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::MetricId;
use oer_hil_run_bundle_format::run::RunState;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    run::Run,
    samples::{self, Better, Change, Sample, Spread, change},
};

/// What performance reporting needs of one run, cached per sealed run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunSummary {
    pub version: u32,
    pub id: String,
    pub started_millis: u64,
    pub state: RunState,
    pub commit: Option<String>,
    pub dirty: bool,
    pub networks: Vec<String>,
    /// Layout seeds of the run's images; empty when every image kept the
    /// linker's natural order.
    #[serde(default)]
    pub layout_seeds: Vec<u32>,
    /// The run's gated samples.
    pub samples: Vec<Sample>,
}

/// Bumped whenever [`RunSummary`] or the samples it holds change meaning,
/// so cached summaries are recomputed.
const SUMMARY_VERSION: u32 = 5;

/// The summary of `run`; an error when its suite reports incompatible
/// metrics ([`samples::samples`]).
pub fn summary(run: &Run) -> Result<RunSummary> {
    let manifest = run.bundle.manifest();
    let mut layout_seeds = manifest
        .firmware
        .iter()
        .filter_map(|firmware| firmware.layout_seed)
        .map(std::num::NonZeroU32::get)
        .collect::<Vec<_>>();
    layout_seeds.sort_unstable();
    layout_seeds.dedup();
    // The network implementations recorded in the run's build provenance.
    let mut networks = manifest
        .firmware
        .iter()
        .filter_map(|firmware| run.bundle.build_provenance(firmware).ok().flatten())
        .filter_map(|provenance| provenance.parameters.network)
        .collect::<Vec<_>>();
    networks.sort();
    networks.dedup();
    Ok(RunSummary {
        version: SUMMARY_VERSION,
        id: run.id().to_owned(),
        started_millis: run.started_millis(),
        state: run.state(),
        commit: Some(run.commit().to_owned()).filter(|commit| !commit.is_empty()),
        dirty: run.dirty(),
        networks,
        layout_seeds,
        samples: run
            .suite
            .as_ref()
            .map(samples::samples)
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .filter(|sample| sample.gate.is_some())
            .collect(),
    })
}

/// Which code placement a run measured: the linker's natural order or the
/// seeds its images were shuffled with.
fn layout(seeds: &[u32]) -> String {
    match seeds {
        [] => "natural".to_owned(),
        seeds => format!(
            "seed {}",
            seeds
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// A reviewed reference for one scenario's gated measurements.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Baseline {
    pub run: String,
    pub commit: Option<String>,
    pub reason: String,
    pub by: String,
    pub unix_millis: u64,
    pub measurements: BTreeMap<String, BaselineMeasurement>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BaselineMeasurement {
    pub unit: MeasurementUnit,
    /// The semantics version of the metric the baseline measured.
    pub semantics: u16,
    pub better: Better,
    pub spread: Spread,
}

impl BaselineMeasurement {
    /// Why this baseline cannot judge values of `metric` (same scenario and
    /// name); `None` when it can.
    pub fn incompatibility(&self, metric: &MetricId) -> Option<String> {
        MetricId {
            scenario: metric.scenario.clone(),
            name: metric.name.clone(),
            unit: self.unit,
            semantics: self.semantics,
            better: Some(self.better),
        }
        .incompatibility(metric)
    }
}

/// The baselines of `store`, by scenario.
pub fn baselines(store: &RunStore) -> Result<BTreeMap<String, Baseline>> {
    store.read(Sidecar::PerfBaselines)
}

/// Make `run` the baseline of each named scenario (all its gated scenarios
/// when none is named). Only a completed run of a clean commit can be one.
pub fn set_baseline(
    store: &RunStore,
    run: &RunSummary,
    scenarios: &[String],
    reason: &str,
    by: &str,
    unix_millis: u64,
) -> Result<Vec<String>> {
    if run.dirty || run.commit.is_none() {
        return Err(format!("run {} was not built from a clean commit", run.id).into());
    }
    if run.state != RunState::Completed {
        return Err(format!("run {} is {}, not completed", run.id, run.state.id()).into());
    }
    let mut by_scenario: BTreeMap<String, BTreeMap<String, BaselineMeasurement>> = BTreeMap::new();
    for sample in &run.samples {
        if !scenarios.is_empty() && !scenarios.contains(&sample.metric.scenario) {
            continue;
        }
        if let (Some(spread), Some(better)) = (sample.spread(), sample.better()) {
            by_scenario
                .entry(sample.metric.scenario.clone())
                .or_default()
                .insert(
                    sample.metric.name.clone(),
                    BaselineMeasurement {
                        unit: sample.metric.unit,
                        semantics: sample.metric.semantics,
                        better,
                        spread,
                    },
                );
        }
    }
    for scenario in scenarios {
        if !by_scenario.contains_key(scenario) {
            return Err(format!("run {} has no gated measurement of {scenario}", run.id).into());
        }
    }
    let mut baselines = baselines(store)?;
    for (scenario, measurements) in &by_scenario {
        baselines.insert(
            scenario.clone(),
            Baseline {
                run: run.id.clone(),
                commit: run.commit.clone(),
                reason: reason.to_owned(),
                by: by.to_owned(),
                unix_millis,
                measurements: measurements.clone(),
            },
        );
    }
    store.write(Sidecar::PerfBaselines, &baselines)?;
    Ok(by_scenario.into_keys().collect())
}

fn short(commit: &Option<String>) -> String {
    commit
        .as_deref()
        .map_or_else(|| "unknown".to_owned(), |c| c[..c.len().min(9)].to_owned())
}

/// Display factor and unit for a measured unit and magnitude.
pub(crate) fn scale(magnitude: f64, unit: MeasurementUnit) -> (f64, String) {
    match unit {
        MeasurementUnit::BitsPerSecond => (1e6, "Mbit/s".into()),
        MeasurementUnit::Microseconds if magnitude.abs() >= 1000.0 => (1000.0, "ms".into()),
        unit => (1.0, unit.to_string()),
    }
}

fn format_spread(spread: &Spread, unit: MeasurementUnit) -> String {
    let (factor, unit) = scale(spread.mean, unit);
    format!(
        "{:.2}±{:.2} {unit} (n={})",
        spread.mean / factor,
        spread.deviation / factor,
        spread.count
    )
}

/// The values of one gated measurement from every clean run of one commit
/// linked with one layout.
struct CommitRow {
    commit: String,
    layout: String,
    runs: String,
    values: Vec<f64>,
    sample: Sample,
}

/// Per-commit summary of the gated measurements of `scenarios` (every gated
/// scenario when empty) in clean runs, oldest first, with each commit
/// compared to the scenario's baseline. Each metric identity is its own
/// section: a name whose unit, semantics or direction changed between
/// commits is never summarized as one series, and a baseline of another
/// identity judges nothing.
pub fn report(
    runs: &[RunSummary],
    scenarios: &[String],
    measurement: Option<&str>,
    baselines: &BTreeMap<String, Baseline>,
) -> String {
    // Metric -> one row per commit, in first-run order.
    let mut rows: BTreeMap<MetricId, Vec<CommitRow>> = BTreeMap::new();
    for run in runs.iter().filter(|run| !run.dirty && run.commit.is_some()) {
        for sample in run.samples.iter().cloned() {
            if !scenarios.is_empty() && !scenarios.contains(&sample.metric.scenario) {
                continue;
            }
            if measurement.is_some_and(|filter| !sample.metric.name.contains(filter)) {
                continue;
            }
            let commit = short(&run.commit);
            let layout = layout(&run.layout_seeds);
            let entries = rows.entry(sample.metric.clone()).or_default();
            match entries
                .iter_mut()
                .find(|entry| entry.commit == commit && entry.layout == layout)
            {
                Some(entry) => {
                    entry.values.extend(&sample.values);
                    entry.runs.push(',');
                    entry.runs.push_str(&run.id);
                }
                None => entries.push(CommitRow {
                    commit,
                    layout,
                    runs: run.id.clone(),
                    values: sample.values.clone(),
                    sample,
                }),
            }
        }
    }
    if rows.is_empty() {
        return "no clean run has a gated measurement of the selection\n".into();
    }
    let mut text = String::new();
    for (metric, entries) in rows {
        let first = &entries[0].sample;
        let Some(gate) = first.gate else {
            continue;
        };
        let direction = match gate.better {
            Better::Higher => "higher is better",
            Better::Lower => "lower is better",
        };
        let (factor, unit) = scale(gate.threshold, metric.unit);
        text.push_str(&format!(
            "{} {} (semantics {}) — gate {:.2} {unit}, {direction}\n",
            metric.scenario,
            metric.name,
            metric.semantics,
            gate.threshold / factor
        ));
        let baseline = baselines
            .get(&metric.scenario)
            .and_then(|baseline| Some((baseline, baseline.measurements.get(&metric.name)?)));
        let incompatible =
            baseline.and_then(|(_, measurement)| measurement.incompatibility(&metric));
        let baseline = baseline.filter(|_| incompatible.is_none());
        if let Some(reason) = &incompatible {
            text.push_str(&format!("  baseline measured another metric: {reason}\n"));
        } else if let Some((baseline, measurement)) = baseline {
            text.push_str(&format!(
                "  baseline {} {}: {} — {}\n",
                short(&baseline.commit),
                baseline.run,
                format_spread(&measurement.spread, measurement.unit),
                baseline.reason
            ));
        } else {
            text.push_str("  no baseline\n");
        }
        text.push_str(&layout_sensitivity(&entries));
        for CommitRow {
            commit,
            layout,
            runs: run_ids,
            values,
            sample,
        } in entries
        {
            let Some(spread) = Spread::of(&values) else {
                continue;
            };
            let verdict = match baseline
                .map(|(_, measurement)| change(&measurement.spread, measurement.better, &spread))
            {
                Some(Change::Regressed) => "  REGRESSED",
                Some(Change::Improved) => "  improved",
                Some(Change::Within) => "  within noise",
                None => "",
            };
            let gate = match sample.gate {
                Some(gate) if gate.better == Better::Higher && spread.mean < gate.threshold => {
                    "  below gate"
                }
                Some(gate) if gate.better == Better::Lower && spread.mean > gate.threshold => {
                    "  above gate"
                }
                _ => "",
            };
            text.push_str(&format!(
                "  {commit:<10} {layout:<10} {}{verdict}{gate}  [{run_ids}]\n",
                format_spread(&spread, sample.metric.unit)
            ));
        }
    }
    text
}

/// For each commit measured with more than one layout, how far the layout
/// means spread compared with the repetitions within one layout: a spread
/// well above the repetition noise means placement, not code, moves the
/// figure, and a single-layout comparison of two commits can mislead.
fn layout_sensitivity(entries: &[CommitRow]) -> String {
    let mut commits: Vec<&str> = Vec::new();
    for entry in entries {
        if !commits.contains(&entry.commit.as_str()) {
            commits.push(&entry.commit);
        }
    }
    let mut text = String::new();
    for commit in commits {
        let layouts = entries
            .iter()
            .filter(|entry| entry.commit == commit)
            .filter_map(|entry| Spread::of(&entry.values).map(|spread| (entry, spread)))
            .collect::<Vec<_>>();
        if layouts.len() < 2 {
            continue;
        }
        let means = layouts
            .iter()
            .map(|(_, spread)| spread.mean)
            .collect::<Vec<_>>();
        let Some(between) = Spread::of(&means) else {
            continue;
        };
        let within = layouts
            .iter()
            .map(|(_, spread)| spread.deviation)
            .sum::<f64>()
            / layouts.len() as f64;
        let unit = layouts[0].0.sample.metric.unit;
        let (factor, unit_name) = scale(between.mean, unit);
        text.push_str(&format!(
            "  {commit:<10} across {} layouts: means {:.2}±{:.2} {unit_name}, repetitions ±{:.2}\n",
            layouts.len(),
            between.mean / factor,
            between.deviation / factor,
            within / factor
        ));
    }
    text
}

/// Every gated measurement of `run` that regressed against its scenario's
/// baseline, or that its baseline cannot judge because it measured another
/// metric (unit, semantics or direction), as report lines; empty when none.
pub fn regressions(run: &RunSummary, baselines: &BTreeMap<String, Baseline>) -> Vec<String> {
    run.samples
        .iter()
        .filter_map(|sample| {
            let baseline = baselines.get(&sample.metric.scenario)?;
            let reference = baseline.measurements.get(&sample.metric.name)?;
            if let Some(reason) = reference.incompatibility(&sample.metric) {
                return Some(format!(
                    "{}: baseline {} measured another metric ({reason}); set a new baseline",
                    sample.metric, baseline.run
                ));
            }
            let spread = sample.spread()?;
            (change(&reference.spread, reference.better, &spread) == Change::Regressed).then(|| {
                format!(
                    "{} {}: {} against baseline {} {}",
                    sample.metric.scenario,
                    sample.metric.name,
                    format_spread(&spread, sample.metric.unit),
                    format_spread(&reference.spread, reference.unit),
                    baseline.run
                )
            })
        })
        .collect()
}

/// Summaries of the runs of `store` started at or after `since_millis`,
/// oldest first. Run directories are named by their start time, so older
/// runs are skipped without reading their bundles; a sealed run's summary
/// is cached in the store's `perf` cache and reused, since its bundle can
/// no longer change.
pub fn summaries_since(store: &RunStore, since_millis: u64) -> Result<Vec<RunSummary>> {
    let cache = store.cache("perf");
    fs::create_dir_all(&cache)?;
    let mut found = Vec::new();
    for name in store.ids_newest_first()? {
        let started = name
            .split('-')
            .next()
            .and_then(|millis| millis.parse::<u64>().ok());
        if started.is_some_and(|started| started < since_millis) {
            continue;
        }
        let cached = cache.join(format!("{name}.json"));
        let reused = fs::read(&cached)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RunSummary>(&bytes).ok())
            .filter(|summary| summary.version == SUMMARY_VERSION);
        let summary = match reused {
            Some(summary) => summary,
            None => {
                let Some(run) = Run::load(&store.run(&name)) else {
                    continue;
                };
                let computed = summary(&run)?;
                if run.is_sealed() {
                    oer_durable::atomic_json(&cached, &computed)?;
                }
                computed
            }
        };
        found.push(summary);
    }
    found.sort_by_key(|run| (run.started_millis, run.id.clone()));
    Ok(found)
}

#[cfg(test)]
mod tests;
