//! Remove rebuildable build caches that no build has used recently, when
//! asked (`cargo hil sweep`); nothing sweeps implicitly. It belongs to
//! HIL: it keeps what queued HIL jobs were fixed with and knows the image
//! classes' build outputs.
//!
//! Incremental session data of every feature set ever built and per-image
//! HIL build caches accumulate in a checkout's `target/` until the disk
//! fills. Only caches Cargo recreates are removed, only in this checkout,
//! never run bundles, evidence or archives, never a directory whose Cargo build lock is held and never what
//! a queued HIL job was fixed with. Another checkout's `target/` is its
//! owner's to sweep.
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::Result;

/// How old unused caches must be before they are removed.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub incremental: Duration,
    pub image_caches: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            incremental: Duration::from_secs(24 * 3600),
            image_caches: Duration::from_secs(3 * 24 * 3600),
        }
    }
}

/// One removable cache directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub reason: String,
}

/// The files that queued or running HIL jobs were fixed with, and the
/// source snapshot each names with `--source-snapshot`, which it reads when
/// it starts.
pub(crate) fn held_by_jobs() -> Result<Vec<PathBuf>> {
    Ok(oer_stand_arbiter::Arbiter::open()?
        .jobs()
        .unfinished()
        .into_iter()
        .flat_map(|job| {
            let snapshot = named_snapshot(&job.command).map(|path| job.checkout.join(path));
            job.fixed.into_iter().chain(snapshot)
        })
        .collect())
}

/// The value of `--source-snapshot` in the `cargo hil` arguments `command`.
fn named_snapshot(command: &[String]) -> Option<&str> {
    let mut arguments = command.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--source-snapshot" {
            return arguments.next().map(String::as_str);
        }
        if let Some(value) = argument.strip_prefix("--source-snapshot=") {
            return Some(value);
        }
    }
    None
}

/// `found` without the candidates that hold or lie inside a `held` path.
fn unheld(found: Vec<Candidate>, held: &[PathBuf]) -> Vec<Candidate> {
    found
        .into_iter()
        .filter(|candidate| {
            !held
                .iter()
                .any(|path| path.starts_with(&candidate.path) || candidate.path.starts_with(path))
        })
        .collect()
}

fn age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    now.duration_since(modified).ok()
}

/// Whether a Cargo build currently holds a lock inside `directory`, within
/// three levels (`<dir>/<triple>/<profile>/.cargo-lock`).
fn build_running(directory: &Path) -> bool {
    fn visit(directory: &Path, depth: usize) -> bool {
        let lock = directory.join(".cargo-lock");
        if lock.is_file()
            && !matches!(
                oer_process::lock::FileLock::try_acquire(&lock, oer_process::lock::Mode::Shared),
                Ok(Some(_))
            )
        {
            return true;
        }
        depth > 0
            && fs::read_dir(directory)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    entry.file_type().is_ok_and(|kind| kind.is_dir())
                        && visit(&entry.path(), depth - 1)
                })
    }
    visit(directory, 3)
}

/// Incremental crate directories older than the policy, below every
/// `incremental` directory of `target` up to four levels deep, skipping a
/// profile whose build lock is held.
fn incremental(target: &Path, policy: Policy, now: SystemTime, found: &mut Vec<Candidate>) {
    fn visit(
        directory: &Path,
        depth: usize,
        policy: Policy,
        now: SystemTime,
        found: &mut Vec<Candidate>,
    ) {
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            if entry.file_name() == "incremental" {
                if build_running(directory) {
                    continue;
                }
                for crate_directory in fs::read_dir(&path).into_iter().flatten().flatten() {
                    let crate_path = crate_directory.path();
                    if crate_path.is_dir()
                        && age(&crate_path, now).is_some_and(|age| age > policy.incremental)
                    {
                        found.push(Candidate {
                            path: crate_path,
                            reason: "incremental data unused".into(),
                        });
                    }
                }
            } else if depth > 0 {
                visit(&path, depth - 1, policy, now, found);
            }
        }
    }
    visit(target, 4, policy, now, found);
}

/// Whether `name`, a directory of `target/hil/<chip>`, is an image class's
/// build output (named after a class's runtime profile) or type-check
/// output (`check-<class>-<network>`).
fn class_output(name: &str) -> bool {
    name.starts_with("check-")
        || oer_hil_schema::image::ImageClass::ALL
            .iter()
            .any(|class| name.starts_with(&format!("{}-", class.runtime_profile())))
}

/// Per-image HIL build outputs of every chip in `chips`, unused for the
/// policy's age and not building: the class outputs of `target/hil/<chip>`.
/// The compile cache is the image pipeline's one shared cache.
fn image_caches(
    target: &Path,
    chips: &[String],
    policy: Policy,
    now: SystemTime,
    found: &mut Vec<Candidate>,
) {
    for chip in chips {
        let directory = target.join("hil").join(chip);
        for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || build_running(&path) || !class_output(&name) {
                continue;
            }
            if age(&path, now).is_some_and(|age| age > policy.image_caches) {
                found.push(Candidate {
                    path,
                    reason: "image cache unused".into(),
                });
            }
        }
    }
}

/// Every removable cache of the checkout at `root` whose chips are `chips`.
pub fn candidates(
    root: &Path,
    chips: &[String],
    policy: Policy,
    now: SystemTime,
) -> Vec<Candidate> {
    let target = root.join("target");
    let mut found = Vec::new();
    incremental(&target, policy, now, &mut found);
    image_caches(&target, chips, policy, now, &mut found);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_dir() {
        fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| size(&entry.path()))
            .sum()
    } else {
        metadata.len()
    }
}

/// List, or with `apply` remove, the caches of the checkout at `root`;
/// returns the bytes that were (or would be) freed.
pub fn run(root: &Path, policy: Policy, apply: bool) -> Result<u64> {
    let chips: Vec<String> = oer_chip_profile::Profile::all(root)?
        .into_iter()
        .map(|profile| profile.id)
        .collect();
    let found = unheld(
        candidates(root, &chips, policy, SystemTime::now()),
        &held_by_jobs()?,
    );
    let mut bytes = 0;
    for candidate in &found {
        bytes += size(&candidate.path);
        if apply {
            if let Err(error) = fs::remove_dir_all(&candidate.path) {
                eprintln!("sweep: cannot remove {}: {error}", candidate.path.display());
            }
        } else if found.len() <= 20 {
            println!("  {} ({})", candidate.path.display(), candidate.reason);
        }
    }
    println!(
        "{} {}: {} caches, {:.1} GiB",
        if apply { "removed" } else { "would remove" },
        root.display(),
        found.len(),
        bytes as f64 / (1u64 << 30) as f64
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(path: &Path, days: u64) {
        let time = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
        fs::File::open(path).unwrap().set_modified(time).unwrap();
    }

    #[test]
    fn only_old_caches_are_candidates_and_bundles_are_kept() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let old = target.join("debug/incremental/crate-old");
        let fresh = target.join("debug/incremental/crate-fresh");
        let stale =
            target.join("hil/chip-a/psram-code-psram-data-psram-stack-correctness-owned-xarxa");
        let live =
            target.join("hil/chip-a/psram-code-psram-data-psram-stack-performance-owned-xarxa");
        let runs = target.join("hil/chip-a/runs.before-shared-store");
        let peer = target.join("hil/chip-b/check-system-watchdog-owned-xarxa");
        let replay = target.join("hil/chip-b/replay");
        let runners = target.join("hil/runners");
        for directory in [&old, &fresh, &stale, &live, &runs, &peer, &replay, &runners] {
            fs::create_dir_all(directory).unwrap();
        }
        for directory in [&old, &stale, &peer] {
            aged(directory, 10);
        }
        for directory in [&runs, &replay, &runners] {
            aged(directory, 30);
        }
        let chips = ["chip-b", "chip-a"].map(String::from);
        let found = candidates(root.path(), &chips, Policy::default(), SystemTime::now());
        let paths = found.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths, [old, stale, peer]);
    }

    #[test]
    fn files_a_queued_job_was_fixed_with_are_kept_with_what_holds_them() {
        let candidate = |path: &str| Candidate {
            path: PathBuf::from(path),
            reason: String::from("idle"),
        };
        let held = [PathBuf::from("/c/target/hil/jobs/cli/ab/oer-hil-cli")];
        assert_eq!(
            unheld(
                vec![
                    candidate("/c/target"),
                    candidate("/c/target/hil/jobs/cli/ab/oer-hil-cli"),
                    candidate("/c/target/debug/incremental/x"),
                    candidate("/d/target"),
                ],
                &held,
            ),
            [
                candidate("/c/target/debug/incremental/x"),
                candidate("/d/target")
            ]
        );
    }

    #[test]
    fn a_held_build_lock_protects_its_caches() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("target/debug");
        let old = profile.join("incremental/crate-old");
        fs::create_dir_all(&old).unwrap();
        aged(&old, 10);
        let lock = oer_process::lock::FileLock::acquire(
            &profile.join(".cargo-lock"),
            oer_process::lock::Mode::Exclusive,
        )
        .unwrap();
        assert!(candidates(root.path(), &[], Policy::default(), SystemTime::now()).is_empty());
        drop(lock);
        assert_eq!(
            candidates(root.path(), &[], Policy::default(), SystemTime::now()).len(),
            1
        );
    }

    /// A job names the snapshot it reads with `--source-snapshot`; a sweep
    /// and source collection keep it as if the job were fixed with it.
    #[test]
    fn a_job_holds_the_snapshot_it_names() {
        let command = |arguments: &[&str]| -> Vec<String> {
            arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect()
        };
        assert_eq!(
            named_snapshot(&command(&[
                "run",
                "smoke",
                "--source-snapshot",
                "/s/schema-2/ab"
            ])),
            Some("/s/schema-2/ab")
        );
        assert_eq!(
            named_snapshot(&command(&["run", "--source-snapshot=rel/cd", "smoke"])),
            Some("rel/cd")
        );
        assert_eq!(named_snapshot(&command(&["run", "smoke"])), None);
    }
}
