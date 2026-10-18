//! Which runs the store keeps: the prune rule and the size budget.
//!
//! Pins and the rule keep what agents cite, replay and compare.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle::store::Notes;
use oer_hil_run_bundle::store::Sidecar;
use oer_hil_run_bundle_format::run::RunState;

use crate::{
    Result,
    run::{Run, Unreadable},
};

/// The prune rule.
#[derive(Debug)]
pub struct Retention {
    /// Every run younger than this is kept.
    pub keep_days: u64,
    /// The newest failed runs kept per scenario and image class.
    pub keep_failed: usize,
}

/// Why each run is kept; runs without a reason may be deleted.
pub fn retained(
    runs: &[Run],
    rule: &Retention,
    now_millis: u64,
    pinned: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut keep = BTreeMap::new();
    let recent = now_millis.saturating_sub(rule.keep_days * 24 * 3600 * 1000);
    for run in runs {
        let id = run.id().to_owned();
        if pinned.contains(&id) {
            keep.insert(id, String::from("pinned"));
        } else if run.started_millis() >= recent {
            keep.insert(id, format!("younger than {} days", rule.keep_days));
        } else if run.state() == RunState::Running && !run.abandoned {
            keep.insert(id, String::from("in progress"));
        } else if run.outcome().is_none() && !run.abandoned {
            keep.insert(id, String::from("incomplete"));
        }
        for source in run.bundle.replayed_runs().unwrap_or_default() {
            keep.entry(source)
                .or_insert_with(|| format!("firmware replayed by {}", run.id()));
        }
    }
    let mut passed = BTreeMap::new();
    let mut failed: BTreeMap<(String, String), Vec<&Run>> = BTreeMap::new();
    for run in runs.iter().rev() {
        for scenario in run.scenarios() {
            let key = (scenario.scenario.clone(), scenario.image.id().to_owned());
            if scenario.outcome.is_passed() {
                passed.entry(key).or_insert(run.id().to_owned());
            } else {
                failed.entry(key).or_default().push(run);
            }
        }
    }
    for ((scenario, image), run) in passed {
        keep.entry(run)
            .or_insert_with(|| format!("latest pass of {scenario} [{image}]"));
    }
    for ((scenario, image), runs) in failed {
        for run in runs.into_iter().take(rule.keep_failed) {
            keep.entry(run.id().to_owned())
                .or_insert_with(|| format!("recent failure of {scenario} [{image}]"));
        }
    }
    keep
}

/// A run the size budget weighs: the bytes only it holds, and whether only
/// its age keeps it.
#[derive(Clone, Copy, Debug)]
pub struct Weighed<'a> {
    pub id: &'a str,
    pub bytes: u64,
    pub deletable: bool,
}

/// The runs to delete, oldest first, so that a store of these `runs`
/// (oldest first) fits `budget` bytes. Only a run only its age keeps is
/// deleted.
pub fn over_budget<'a>(runs: &[Weighed<'a>], budget: u64) -> Vec<&'a str> {
    let mut total: u64 = runs.iter().map(|run| run.bytes).sum();
    let mut deleted = Vec::new();
    for run in runs {
        if total <= budget {
            break;
        }
        if !run.deletable {
            continue;
        }
        total = total.saturating_sub(run.bytes);
        deleted.push(run.id);
    }
    deleted
}

/// Bytes only this directory holds: files without other hard links.
pub fn exclusive_bytes(directory: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    let mut total = 0;
    let mut pending = vec![directory.to_owned()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.nlink() == 1 {
                total += metadata.len();
            }
        }
    }
    total
}

/// What a prune deleted, or would delete.
#[derive(Debug, Default)]
pub struct Pruned {
    /// The deleted runs, oldest first, with the bytes only they held.
    pub removed: Vec<(Run, u64)>,
    /// How many runs a rule keeps.
    pub kept: usize,
    /// How many runs the store held.
    pub total: usize,
    /// How many observer builds no kept run names were deleted.
    pub observers: usize,
    /// The source objects and captures that were deleted.
    pub sources: crate::runs::CollectedSources,
    /// The deleted runs this build cannot read, with the bytes only they
    /// held: those of an older schema, and with `unreadable` those of the
    /// current one.
    pub removed_unreadable: Vec<(Unreadable, u64)>,
    /// The runs this build cannot read that stay, with why: pinned,
    /// of a newer or unknown schema, of an older one but younger than the
    /// rule's days, or of the current one without `unreadable`.
    pub unreadable_kept: Vec<(Unreadable, String)>,
}

impl Pruned {
    pub fn freed(&self) -> u64 {
        self.removed.iter().map(|(_, bytes)| bytes).sum::<u64>()
            + self
                .removed_unreadable
                .iter()
                .map(|(_, bytes)| bytes)
                .sum::<u64>()
    }
}

/// Delete (with `apply`; else only list) the runs of `store` that no rule
/// keeps: not pinned (by hand, by an archive selection or by an unpublished
/// evidence archive version), not replayed, not younger than the rule, not
/// the latest pass or a recent failure of a scenario. With a `budget` in
/// bytes, a store over it also loses its oldest runs that only their age
/// kept.
///
/// A run this build cannot read is deleted when its schema is older than
/// this build's and it is older than the rule's days, and, with
/// `unreadable`, when its schema is this build's; pinned runs stay. A run of
/// a newer or unknown schema always stays: the store is shared by every
/// checkout, and a newer branch's runs are only unreadable to this one.
pub fn prune(
    store: &RunStore,
    rule: &Retention,
    budget: Option<u64>,
    apply: bool,
    unreadable: bool,
    captures: &crate::runs::Captures,
) -> Result<Pruned> {
    let all = Run::all(store)?;
    // From reading the pins until the last deletion, no pin can land: a pin
    // taken meanwhile (`cargo hil archive ref`, `runs pin`) waits, then
    // checks under the lock that its run still exists and fails if not.
    let mut pins_lock = Some(store.lock_notes(Sidecar::Pins, oer_process::lock::Mode::Shared)?);
    let now = oer_durable::unix_millis();
    let pinned = store
        .read::<Notes>(Sidecar::Pins)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    let keep = retained(&all, rule, now, &pinned);
    let unreadable_runs = Run::unreadable(store)?;
    let recent = now.saturating_sub(rule.keep_days * 24 * 3600 * 1000);
    let mut pruned = Pruned {
        total: all.len() + unreadable_runs.len(),
        ..Pruned::default()
    };
    for run in unreadable_runs {
        let kept = if pinned.contains(&run.id) {
            Some(String::from("pinned"))
        } else if run.older_schema() {
            (run.started_millis >= recent)
                .then(|| format!("older schema, younger than {} days", rule.keep_days))
        } else if run.current_schema() {
            (!unreadable).then(|| String::from("this build's schema; --unreadable deletes it"))
        } else if run.schema.is_some() {
            Some(String::from("newer schema, a newer branch's"))
        } else {
            Some(String::from("unknown schema"))
        };
        match kept {
            Some(reason) => pruned.unreadable_kept.push((run, reason)),
            None => {
                let bytes = exclusive_bytes(&run.directory);
                pruned.removed_unreadable.push((run, bytes));
            }
        }
    }
    let (remaining, deleted): (Vec<Run>, Vec<Run>) =
        all.into_iter().partition(|run| keep.contains_key(run.id()));
    for run in deleted {
        let bytes = exclusive_bytes(run.directory());
        pruned.removed.push((run, bytes));
    }
    if let Some(budget) = budget {
        // Every run that stays counts, readable or not; a readable one only
        // its age keeps and an older-schema one only its age keeps go,
        // oldest first, until the store fits.
        let sizes = remaining
            .iter()
            .map(|run| (run.id().to_owned(), exclusive_bytes(run.directory())))
            .chain(
                pruned
                    .unreadable_kept
                    .iter()
                    .map(|(run, _)| (run.id.clone(), exclusive_bytes(&run.directory))),
            )
            .collect::<BTreeMap<_, _>>();
        let kept = retained(
            &remaining,
            &Retention {
                keep_days: 0,
                keep_failed: rule.keep_failed,
            },
            now,
            &pinned,
        );
        let mut weighed = remaining
            .iter()
            .map(|run| {
                (
                    run.started_millis(),
                    Weighed {
                        id: run.id(),
                        bytes: sizes[run.id()],
                        deletable: !kept.contains_key(run.id()),
                    },
                )
            })
            .chain(pruned.unreadable_kept.iter().map(|(run, _)| {
                (
                    run.started_millis,
                    Weighed {
                        id: &run.id,
                        bytes: sizes[&run.id],
                        deletable: run.older_schema() && !pinned.contains(&run.id),
                    },
                )
            }))
            .collect::<Vec<_>>();
        weighed.sort_by_key(|(started, run)| (*started, run.id));
        let weighed = weighed.into_iter().map(|(_, run)| run).collect::<Vec<_>>();
        let over = over_budget(&weighed, budget)
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for run in remaining {
            if over.contains(run.id()) {
                let bytes = sizes[run.id()];
                pruned.removed.push((run, bytes));
            } else {
                pruned.kept += 1;
            }
        }
        for (run, reason) in std::mem::take(&mut pruned.unreadable_kept) {
            if over.contains(&run.id) {
                let bytes = sizes[&run.id];
                pruned.removed_unreadable.push((run, bytes));
            } else {
                pruned.unreadable_kept.push((run, reason));
            }
        }
    } else {
        pruned.kept = remaining.len();
    }
    pruned.kept += pruned.unreadable_kept.len();
    if apply {
        for (run, _) in &pruned.removed {
            fs::remove_dir_all(run.directory())?;
        }
        for (run, _) in &pruned.removed_unreadable {
            fs::remove_dir_all(&run.directory)?;
        }
        // The runs are gone: pins may land again while the shared objects
        // the deleted runs alone named are collected.
        pins_lock.take();
        pruned.observers = crate::runs::collect_observers(store)?;
        pruned.sources = crate::runs::collect_sources(store, captures)?;
    }
    drop(pins_lock);
    Ok(pruned)
}

#[cfg(test)]
mod tests;
