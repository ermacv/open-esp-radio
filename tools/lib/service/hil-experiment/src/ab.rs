//! A/B experiments: two variants of the firmware on the same scenarios,
//! with enough repetitions to tell an effect from noise.
//!
//! A variant is a repository revision plus local checkouts that replace its
//! pinned dependencies (the `ESP_HAL_ROOT` root). Each arm's revision is checked out in the
//! experiment's worktree below `target/hil/ab/<id>/` and captured with its
//! overrides into a source snapshot, which the experiment's runner builds:
//! both arms therefore need the checkout's HIL protocol version.
//!
//! The experiment runs in rounds: both arms on one layout seed. For every
//! layout seed, round 0 is the **preparation**: it builds each arm's images
//! (A, then B), each run under a lease of its own, and warms the stand up;
//! its runs are recorded with the preparation phase and never paired. The
//! **measured** rounds `1..=repetitions` replay those exact images, each
//! round holding one whole-stand lease for both arms, so drift of the air
//! and the calibrations falls on both arms of a round alike, and other
//! owners' shorter work goes between rounds. `--repetitions N` is the
//! number of measured rounds, so N pairs per layout seed.
//!
//! Which arm runs first in a measured round is balanced and seeded: the
//! measured rounds of a layout seed come in consecutive pairs, one AB and
//! one BA, and which of the two leads is drawn from
//! `oer_stats::balanced_swaps` with the experiment's order seed (given, or
//! the experiment id) mixed with the layout seed. Warm-up, temperature drift and recovery order therefore
//! fall on both arms alike instead of always on B. The order seed is in the
//! report, and every run records its round (layout seed, index, order,
//! order seed) in its manifest with its arm and variant
//! ([`oer_hil_run_bundle_format::experiment`]).
//!
//! Every run is launched through [`crate::launch::launch_run`] as a job of
//! its own and records no evidence. The report compares, per metric, the
//! two arms' run means paired by measured round, after checking that every
//! run belongs to this one experiment ([`oer_hil_analysis::arms`]), and is
//! written to `ab-report.json`.

use std::{
    collections::BTreeMap,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_analysis::{
    Run,
    arms::{self, MeasurementComparison, ValidatedExperiment},
};
use oer_hil_run_bundle::RunId;
use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle_format::experiment::Arm;
use oer_hil_run_bundle_format::experiment::DependencyOverride;
use oer_hil_run_bundle_format::experiment::Experiment;
use oer_hil_run_bundle_format::experiment::Order;
use oer_hil_run_bundle_format::experiment::Phase;
use oer_hil_run_bundle_format::experiment::Round;
use oer_hil_run_bundle_format::experiment::Variant;
use oer_hil_schema::dependency::Dependency;
use oer_hil_schema::image::FeatureDelta;
use oer_hil_schema::run::Outcome;
use oer_process::git::Worktree;
use serde::Serialize;

use crate::{
    Result,
    job::Running,
    launch::{Launch, Runner, launch_run},
};

const REPORT_SCHEMA: u16 = 3;

/// A variant as written on the command line, before its revision and
/// checkouts are resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantSpec {
    pub revision: String,
    pub overrides: Vec<(Dependency, PathBuf)>,
    /// Runtime features added to or removed from each image class's own.
    pub features: FeatureDelta,
}

impl std::str::FromStr for VariantSpec {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        let mut revision = None;
        let mut overrides = Vec::new();
        let mut features = None;
        for part in text
            .split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            if let Some(value) = part.strip_prefix("rev=") {
                if revision.replace(value.to_owned()).is_some() {
                    return Err(format!("`{text}` names two revisions"));
                }
            } else if let Some(value) = part.strip_prefix("override:") {
                let (dependency, path) = value
                    .split_once('=')
                    .ok_or_else(|| format!("`{part}` is not override:<dependency>=<path>"))?;
                let dependency: Dependency = dependency.parse()?;
                if overrides.iter().any(|(known, _)| *known == dependency) {
                    return Err(format!("`{text}` overrides {} twice", dependency.id()));
                }
                overrides.push((dependency, PathBuf::from(path)));
            } else if let Some(value) = part.strip_prefix("features=") {
                if features.replace(value.parse::<FeatureDelta>()?).is_some() {
                    return Err(format!("`{text}` names features twice"));
                }
            } else {
                return Err(format!(
                    "`{part}` is neither rev=<revision>, override:<dependency>=<path> nor \
                     features=+f,-g"
                ));
            }
        }
        Ok(Self {
            revision: revision.unwrap_or_else(|| String::from("HEAD")),
            overrides,
            features: features.unwrap_or_default(),
        })
    }
}

/// What one experiment compares.
#[derive(Clone, Debug)]
pub struct Spec {
    pub a: VariantSpec,
    pub b: VariantSpec,
    pub scenarios: Vec<String>,
    /// Measured rounds per layout seed, after its preparation round: the
    /// pairs each layout seed contributes.
    pub repetitions: u32,
    /// Layout seeds 1..=K, each built and run for both arms.
    pub layout_seeds: u32,
    /// The runner arguments naming the boards every run uses.
    pub boards: Vec<String>,
    /// The seed of the rounds' order; `None` takes the experiment id.
    pub order_seed: Option<u64>,
}

/// One run of an arm.
#[derive(Clone, Debug, Serialize)]
pub struct ArmRun {
    pub arm: Arm,
    /// The round it ran in: layout seed, index, order and order seed.
    pub round: Round,
    pub run: String,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema: u16,
    pub id: String,
    pub a: Variant,
    pub b: Variant,
    pub scenarios: Vec<String>,
    pub repetitions: u32,
    pub layout_seeds: u32,
    /// The seed every round's order was drawn from.
    pub order_seed: u64,
    pub runs: Vec<ArmRun>,
    pub comparisons: Vec<MeasurementComparison>,
}

/// A finished experiment: its report, where it is written, and every run
/// it created with its outcome.
pub struct Finished {
    pub report: Report,
    pub path: PathBuf,
    pub runs: Vec<(RunId, Option<Outcome>)>,
}

/// Run the experiment `spec` in the checkout at `checkout` with `runner`
/// for `owner`.
pub fn run(checkout: &Path, owner: &str, runner: &Runner, spec: &Spec) -> Result<Finished> {
    if spec.repetitions == 0 || spec.layout_seeds == 0 {
        return Err("--repetitions and --layout-seeds must be at least 1".into());
    }
    let started = oer_durable::unix_millis();
    let id = started.to_string();
    let order_seed = spec.order_seed.unwrap_or(started);
    let directory = checkout.join("target/hil/ab").join(&id);
    std::fs::create_dir_all(&directory)?;
    let current =
        crate::revision::messages_lock(checkout).ok_or("this checkout names no HIL wire")?;
    let arms = [(Arm::A, &spec.a), (Arm::B, &spec.b)]
        .into_iter()
        .map(|(arm, variant)| prepare(checkout, &directory, arm, variant, &current))
        .collect::<Result<Vec<_>>>()?;
    let prepared = |arm: Arm| match arm {
        Arm::A => &arms[0],
        Arm::B => &arms[1],
    };
    let mut report = Report {
        schema: REPORT_SCHEMA,
        id: id.clone(),
        a: arms[0].variant.clone(),
        b: arms[1].variant.clone(),
        scenarios: spec.scenarios.clone(),
        repetitions: spec.repetitions,
        layout_seeds: spec.layout_seeds,
        order_seed,
        runs: Vec::new(),
        comparisons: Vec::new(),
    };
    let path = directory.join("ab-report.json");
    let seeds = (1..=spec.layout_seeds)
        .filter_map(NonZeroU32::new)
        .collect::<Vec<_>>();
    let schedule = seeds
        .iter()
        .map(|seed| (*seed, orders(order_seed, *seed, spec.repetitions)))
        .collect::<BTreeMap<_, _>>();
    // Measured round `index` (1..=repetitions) takes the order at
    // `index - 1`; the preparation round runs A, then B.
    let round_of = |seed: NonZeroU32, index: u32| round(&schedule[&seed], seed, index, order_seed);
    let session = Session {
        checkout,
        owner,
        id: &id,
        directory: &directory,
        runner,
        boards: &spec.boards,
        store: RunStore::shared()?,
    };
    let mut loaded = Vec::new();
    // The preparation round of each seed builds each arm's images, each run
    // under a lease of its own; no analysis pairs it.
    let mut first = BTreeMap::new();
    for seed in &seeds {
        let round = round_of(*seed, 0);
        for arm in round.order.arms() {
            let run = prepared(arm).run(
                &session,
                &spec.scenarios,
                &Firmware::Build(*seed),
                round,
                None,
            )?;
            first.insert((arm, *seed), run.clone());
            record(&session.store, &mut report, &mut loaded, arm, round, &run)?;
            write_report(&path, &report)?;
        }
    }
    // The measured rounds replay those images. One round, both arms of one
    // seed in the round's order, holds the stand under one grant, so drift
    // falls on both arms alike; between rounds the lease is released and
    // shorter work of owners with a higher balance goes first. Every round
    // leases the same work, so the arbiter estimates the next round from
    // the rounds before it.
    let arbiter = oer_stand_arbiter::Arbiter::open()?;
    let work = round_work(&spec.scenarios);
    for index in 1..=spec.repetitions {
        for seed in &seeds {
            let round = round_of(*seed, index);
            let grant = arbiter.acquire(&oer_stand_arbiter::Request {
                owner: owner.to_owned(),
                work: work.clone(),
                scenarios: spec.scenarios.clone(),
                run: None,
                claims: vec![oer_stand_claims::Claim::stand()],
            })?;
            for arm in round.order.arms() {
                let built = &first[&(arm, *seed)];
                for (_, scenarios) in classes_of(&loaded, built) {
                    let run = prepared(arm).run(
                        &session,
                        &scenarios,
                        &Firmware::Replay(built.clone()),
                        round,
                        Some(&grant),
                    )?;
                    record(&session.store, &mut report, &mut loaded, arm, round, &run)?;
                    write_report(&path, &report)?;
                }
            }
            drop(grant);
        }
    }
    let experiment =
        ValidatedExperiment::new(&loaded.iter().map(|(_, run)| run).collect::<Vec<_>>())?;
    if experiment.id() != id {
        return Err(format!("the runs record experiment {}, not {id}", experiment.id()).into());
    }
    report.comparisons = arms::compare(&experiment)?;
    write_report(&path, &report)?;
    for arm in arms {
        if let Err(error) = arm.worktree.remove() {
            eprintln!("hil: the arm's worktree was not removed: {error}");
        }
    }
    Ok(Finished {
        runs: loaded
            .iter()
            .map(|(_, run)| (RunId::new(run.id()), run.outcome()))
            .collect(),
        report,
        path,
    })
}

/// Round `index` of `layout_seed`: the preparation for 0, which runs A then
/// B, else the measured round whose order is `orders[index - 1]`.
fn round(orders: &[Order], layout_seed: NonZeroU32, index: u32, order_seed: u64) -> Round {
    Round {
        layout_seed: layout_seed.get(),
        index,
        phase: Phase::of(index),
        order: match index.checked_sub(1) {
            None => Order::Ab,
            Some(measured) => orders[measured as usize],
        },
        order_seed,
    }
}

/// The order of each of the `rounds` measured rounds of `layout_seed`:
/// balanced AB/BA pairs drawn from `order_seed` mixed with the layout seed,
/// so every layout seed has its own reproducible sequence.
fn orders(order_seed: u64, layout_seed: NonZeroU32, rounds: u32) -> Vec<Order> {
    let stream = order_seed ^ u64::from(layout_seed.get()).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    oer_stats::balanced_swaps(stream, rounds as usize)
        .into_iter()
        .map(Order::swapped)
        .collect()
}

/// The arbiter work of one replay round: the same for every round of an
/// experiment on these scenarios, so each round's estimate is the rounds'
/// own duration.
fn round_work(scenarios: &[String]) -> String {
    format!("ab round {}", scenarios.join(" "))
}

/// Add the run `id` of `arm` to the report and to the loaded runs.
fn record(
    store: &RunStore,
    report: &mut Report,
    loaded: &mut Vec<(Arm, Run)>,
    arm: Arm,
    round: Round,
    id: &RunId,
) -> Result<()> {
    let run = Run::open(store, id.as_str())?;
    report.runs.push(ArmRun {
        arm,
        round,
        run: id.to_string(),
        outcome: run.outcome().map(|outcome| outcome.id().to_owned()),
    });
    loaded.push((arm, run));
    Ok(())
}

/// The scenarios of the run `id`, grouped by the image class each ran on:
/// a replay flashes one class.
fn classes_of(loaded: &[(Arm, Run)], id: &RunId) -> BTreeMap<String, Vec<String>> {
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some((_, run)) = loaded.iter().find(|(_, run)| run.id() == id.as_str()) {
        for scenario in run.scenarios() {
            classes
                .entry(scenario.image.id().to_owned())
                .or_default()
                .push(scenario.scenario.clone());
        }
    }
    classes
}

/// How a run gets its firmware.
#[derive(Clone, Debug)]
enum Firmware {
    /// Build from the arm's snapshot with this layout seed.
    Build(NonZeroU32),
    /// Replay the images an earlier run of the arm archived.
    Replay(RunId),
}

/// What every run of one experiment shares.
struct Session<'a> {
    checkout: &'a Path,
    owner: &'a str,
    id: &'a str,
    directory: &'a Path,
    /// The runner every round runs, fixed when the experiment started.
    runner: &'a Runner,
    boards: &'a [String],
    store: RunStore,
}

struct PreparedArm {
    arm: Arm,
    variant: Variant,
    worktree: Worktree,
    snapshot: PathBuf,
}

/// Check the arm's revision out, resolve its overrides and capture its
/// sources.
fn prepare(
    checkout: &Path,
    directory: &Path,
    arm: Arm,
    spec: &VariantSpec,
    current: &str,
) -> Result<PreparedArm> {
    let commit = oer_process::git::commit(checkout, &spec.revision)?;
    let worktree = Worktree::detached(checkout, &directory.join(format!("arm-{arm}")), &commit)?;
    if crate::revision::messages_lock(worktree.path()).as_deref() != Some(current) {
        return Err(format!(
            "arm {arm} ({}) speaks another HIL wire than this checkout (its \
             hil/protocol/messages.lock differs); an A/B comparison runs both arms on this \
             checkout's runner",
            spec.revision
        )
        .into());
    }
    let mut overrides = Vec::new();
    for (dependency, path) in &spec.overrides {
        let path = path.canonicalize().map_err(|error| {
            format!(
                "override {} at {}: {error}",
                dependency.id(),
                path.display()
            )
        })?;
        overrides.push(DependencyOverride {
            dependency: *dependency,
            commit: oer_process::git::text(&path, ["rev-parse", "HEAD"])?,
            dirty: !oer_process::git::text(&path, ["status", "--porcelain"])?.is_empty(),
            path,
        });
    }
    let snapshot = oer_hil_source_snapshot::capture_with_overrides(
        worktree.path(),
        &[],
        false,
        &overrides
            .iter()
            .map(|entry| (entry.dependency, entry.path.clone()))
            .collect::<Vec<_>>(),
        &oer_hil_image::source_snapshot_store()?,
        &oer_hil_image::source_objects()?,
    )?;
    Ok(PreparedArm {
        arm,
        variant: Variant {
            commit,
            overrides,
            features: spec.features.clone(),
        },
        worktree,
        snapshot: snapshot.directory().to_owned(),
    })
}

/// The runner's argument for a variant's feature delta, none for an empty
/// one: one `--features=<delta>` word, since a delta that starts with a
/// removal (`-phy-rx-hot-sram`) would otherwise parse as an option of its own.
fn features_argument(features: &FeatureDelta) -> Option<String> {
    (!features.is_empty()).then(|| format!("--features={features}"))
}

impl PreparedArm {
    /// One run of this arm in `round`, a job of its own, under `grant`
    /// when the experiment holds the stand; the created run's id.
    fn run(
        &self,
        session: &Session<'_>,
        scenarios: &[String],
        firmware: &Firmware,
        round: Round,
        grant: Option<&oer_stand_arbiter::Grant>,
    ) -> Result<RunId> {
        let mut arguments = vec![String::from("run")];
        arguments.extend(scenarios.iter().cloned());
        arguments.extend(session.boards.iter().cloned());
        match firmware {
            Firmware::Build(seed) => {
                arguments.push(String::from("--source-snapshot"));
                arguments.push(self.snapshot.to_string_lossy().into_owned());
                arguments.extend([String::from("--layout-seed"), seed.to_string()]);
                arguments.extend(features_argument(&self.variant.features));
            }
            Firmware::Replay(run) => {
                arguments.extend([String::from("--firmware-from"), run.to_string()]);
            }
        }
        let experiment = Experiment {
            id: session.id.to_owned(),
            arm: self.arm,
            variant: self.variant.clone(),
            round,
        };
        let arguments = arguments
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let mut job = Running::new(session.checkout, session.owner, &arguments)?;
        let mut launch = Launch::new(session.checkout, session.runner)
            .args(arguments)
            .env(oer_stand_owners::OWNER_ENV, session.owner)
            .env(
                oer_hil_run_bundle_format::experiment::EXPERIMENT_ENV,
                serde_json::to_string(&experiment)?,
            )
            .log(session.directory.join(format!(
                "run-{}-{}.log",
                self.arm,
                oer_durable::unix_millis()
            )));
        let mut context = match grant {
            Some(grant) => grant.context()?,
            None => oer_process::Context::current()?.clone(),
        };
        context.set(oer_stand_owners::OWNER_KEY, session.owner);
        context.set(oer_stand_arbiter::jobs::JOB_KEY, job.id());
        launch = launch.context(context);
        let launched = launch_run(&launch)?;
        job.finish_with(&session.store, &launched.runs)?;
        launched.run()
    }
}

pub fn summary(report: &Report) -> String {
    let mut text = format!(
        "ab {}: A {} vs B {}, {} runs, order seed {}\n",
        report.id,
        describe(&report.a),
        describe(&report.b),
        report.runs.len(),
        report.order_seed
    );
    for entry in &report.comparisons {
        let Some(c) = entry.comparison.compared() else {
            text.push_str(&format!(
                "  {} {}: no pairs ({} unpaired measured rounds): nothing compared\n",
                entry.metric.scenario, entry.metric.name, entry.unpaired_rounds
            ));
            continue;
        };
        text.push_str(&format!(
            "  {} {}: A {:.3} ±{:.3} (n={}), B {:.3} ±{:.3} (n={}) {}; paired B−A {:+.3} ±{:.3} \
             ({} pairs, {} unpaired): {}{}\n",
            entry.metric.scenario,
            entry.metric.name,
            c.a.mean,
            c.a.deviation,
            c.a.count,
            c.b.mean,
            c.b.deviation,
            c.b.count,
            entry.metric.unit,
            c.difference,
            c.interval,
            entry.pairs.len(),
            entry.unpaired_rounds,
            c.verdict,
            if entry.better.is_none() {
                " (ungated)"
            } else {
                ""
            }
        ));
    }
    text
}

fn describe(variant: &Variant) -> String {
    let mut text = variant.commit[..12.min(variant.commit.len())].to_owned();
    for entry in &variant.overrides {
        text.push_str(&format!(
            " +{}@{}{}",
            entry.dependency.id(),
            &entry.commit[..12.min(entry.commit.len())],
            if entry.dirty { "+" } else { "" }
        ));
    }
    text
}

fn write_report(path: &Path, report: &Report) -> Result<()> {
    oer_durable::atomic_json(path, report)
}

#[cfg(test)]
mod tests;
