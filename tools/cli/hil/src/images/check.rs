//! `cargo hil images check`: build HIL firmware image classes the way
//! `cargo hil image build` does,
//! with the link-time stack, placement and application audits, and report
//! every class's outcome instead of stopping at the first failure.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use oer_hil_schema::image::ImageClass;
use oer_hil_source_snapshot::FrozenSources;

use crate::Result;
use oer_process::Checkout;

/// How far each class is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Depth {
    /// The full image build and its audits.
    Build,
    /// A `cargo check` of the runtime with the class's features only.
    TypeCheck,
}

/// One class's result.
#[derive(Debug)]
pub struct Outcome {
    pub class: ImageClass,
    pub elapsed: Duration,
    pub failure: Option<String>,
    /// The ROM summaries the class's image applied.
    pub summaries: BTreeSet<String>,
}

/// Every class with the runtime features its image builds with, and how to
/// type-check one.
pub fn list() -> String {
    let mut text = String::new();
    for class in ImageClass::ALL {
        text.push_str(&format!(
            "{:<36} {}\n",
            class.id(),
            class.build_features(oer_hil_image_class::NETWORK_FEATURE)
        ));
    }
    text.push_str("type-check one class: cargo hil images check --class <class> --type-check\n");
    text
}

/// The classes to take: `selected`, or every class when it is empty, in
/// [`ImageClass::ALL`] order without repeats.
pub fn classes(selected: &[ImageClass]) -> Vec<ImageClass> {
    ImageClass::ALL
        .into_iter()
        .filter(|class| selected.is_empty() || selected.contains(class))
        .collect()
}

/// The chips whose HIL agent builds a class of `classes` (every class when
/// it is empty): `requested`, or every supported chip.
pub fn chips(
    root: &std::path::Path,
    requested: Option<&str>,
    selected: &[ImageClass],
) -> Result<Vec<String>> {
    let candidates = match requested {
        Some(chip) => vec![chip.to_owned()],
        None => oer_chip_profile::supported(root)?,
    };
    let mut chips = Vec::new();
    for chip in candidates {
        if !served(root, &chip, selected)?.is_empty() {
            chips.push(chip);
        }
    }
    Ok(chips)
}

/// The classes of [`classes`]`(selected)` that `chip`'s HIL agent builds.
pub fn served(
    root: &std::path::Path,
    chip: &str,
    selected: &[ImageClass],
) -> Result<Vec<ImageClass>> {
    let mut served = Vec::new();
    for class in classes(selected) {
        if oer_hil_image::builds_on(root, chip, class)? {
            served.push(class);
        }
    }
    Ok(served)
}

/// Classes built at once when `--jobs` is not given. Once the seed classes
/// built the shared dependencies, most of a class's build is its final
/// crate's fat LTO, which runs on one core and needs about 1.5 GB; half the
/// cores, at most eight, keeps both the cores and the memory busy.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).clamp(1, 8))
}

/// The classes whose image a change of `changed` (repository-relative
/// paths) can alter or whose audits it can change: those whose last build in
/// the host build root read a changed file, as its `source-inputs.json`
/// records (sources, workspace and Cargo configuration, policies and the
/// image builder itself), and every class that has no complete record yet.
pub fn affected(chip: &str, changed: &[PathBuf]) -> Result<Vec<ImageClass>> {
    Ok(affected_in(&oer_hil_image::chip_build_root(chip)?, changed))
}

fn affected_in(builds: &std::path::Path, changed: &[PathBuf]) -> Vec<ImageClass> {
    ImageClass::ALL
        .into_iter()
        .filter(|class| match last_inputs(builds, *class) {
            Some(inputs) => changed.iter().any(|path| inputs.contains(path)),
            None => true,
        })
        .collect()
}

/// Every input the newest build of `class` in the host build root recorded,
/// or `None` when it has none or an older record that listed only the
/// compiled sources.
fn last_inputs(
    builds: &std::path::Path,
    class: ImageClass,
) -> Option<std::collections::BTreeSet<PathBuf>> {
    #[derive(serde::Deserialize)]
    struct SourceInputs {
        schema: u32,
        files: Vec<PathBuf>,
    }
    let name = format!("{}-{}", class.id(), oer_hil_image_class::NETWORK);
    let newest = std::fs::read_dir(builds.join("snapshot-builds"))
        .ok()?
        .flatten()
        .map(|snapshot| snapshot.path().join(&name).join("source-inputs.json"))
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .max()?;
    let inputs: SourceInputs = serde_json::from_slice(&std::fs::read(newest.1).ok()?).ok()?;
    (inputs.schema == oer_image::source_inputs::SCHEMA).then(|| inputs.files.into_iter().collect())
}

/// The image classes to type-check for chip code in `changed` packages:
/// every class whose image's package graph (`graphs`) compiles one of them,
/// in catalog order.
pub fn image_classes(
    changed: &BTreeSet<&str>,
    graphs: &[(ImageClass, BTreeSet<String>)],
) -> Vec<ImageClass> {
    graphs
        .iter()
        .filter(|(_, packages)| {
            packages
                .iter()
                .any(|package| changed.contains(package.as_str()))
        })
        .map(|(class, _)| *class)
        .collect()
}

/// The classes a change reaches: every class whose image compiles one of
/// the `packages` with changed chip code and, with
/// `changed` files, every class whose last build read one of them.
pub fn reached(
    root: &std::path::Path,
    chip: &str,
    packages: &[String],
    changed: &[PathBuf],
) -> Result<Vec<ImageClass>> {
    let packages: BTreeSet<&str> = packages.iter().map(String::as_str).collect();
    let mut graphs = Vec::new();
    for class in served(root, chip, &[])? {
        graphs.push((class, oer_hil_image::packages(root, chip, class)?));
    }
    let mut classes = image_classes(&packages, &graphs);
    if !changed.is_empty() {
        for class in affected(chip, changed)? {
            if !classes.contains(&class) {
                classes.push(class);
            }
        }
    }
    println!(
        "images check: {} package(s) with chip code; {} image class(es)",
        packages.len(),
        classes.len()
    );
    Ok(classes)
}

/// Classes waiting to build, and the outcomes of those that did.
#[derive(Default)]
struct Queue {
    ready: std::collections::VecDeque<(usize, ImageClass)>,
    outcomes: Vec<(usize, Outcome)>,
}

/// Builds or type-checks every selected class, up to `jobs` at once, and
/// reports the outcomes in catalog order. Every class compiles in the
/// image pipeline's one shared compile cache, so a dependency is compiled
/// once for all of them; the cache serializes their Cargo runs.
pub fn run(
    ctx: &Checkout,
    chip: &str,
    selected: &[ImageClass],
    depth: Depth,
    jobs: usize,
) -> Result<()> {
    // A full build compiles a snapshot of this checkout in one of the host's
    // build slots, whose paths and caches every checkout shares.
    let frozen = match depth {
        Depth::Build => {
            let snapshot = oer_hil_source_snapshot::capture(
                &ctx.root,
                &[],
                true,
                &oer_hil_image::source_snapshot_store()?,
                &oer_hil_image::source_objects()?,
            )?;
            Some(FrozenSources::open_in_free_workspace(
                snapshot.directory(),
                &oer_hil_image::source_objects()?,
                &oer_hil_image::frozen::build_slots(chip)?,
            )?)
        }
        Depth::TypeCheck => None,
    };
    let frozen = frozen.as_ref();
    let classes: Vec<(usize, ImageClass)> = served(&ctx.root, chip, selected)?
        .into_iter()
        .enumerate()
        .collect();
    let total = classes.len();
    let mut queue = Queue::default();
    queue.ready.extend(classes.iter().copied());
    let queue = std::sync::Mutex::new(queue);
    let lock = || {
        queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, total.max(1)) {
            scope.spawn(|| {
                loop {
                    let next = lock().ready.pop_front();
                    let Some((index, class)) = next else {
                        break;
                    };
                    let outcome = build_one(ctx, chip, frozen, class, index, total, depth);
                    lock().outcomes.push((index, outcome));
                }
            });
        }
    });
    let mut outcomes = queue
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .outcomes;
    outcomes.sort_by_key(|(index, _)| *index);
    let outcomes: Vec<Outcome> = outcomes.into_iter().map(|(_, outcome)| outcome).collect();
    let profile = oer_chip_profile::Profile::load(&ctx.root, chip)?;
    print!("{}", summary(&outcomes));
    verdict(&outcomes)?;
    // Only the whole catalog, built, shows which summaries no image uses.
    if selected.is_empty() && depth == Depth::Build {
        let reviewed = oer_image_check_interrupts::rom_summaries(&ctx.root, &profile)?;
        stale_summaries(&reviewed, &outcomes)?;
    }
    Ok(())
}

/// Fails naming every reviewed ROM summary no class's image applied: a
/// summary of a ROM function no image reaches is stale.
fn stale_summaries(reviewed: &[String], outcomes: &[Outcome]) -> Result<()> {
    let applied: BTreeSet<&String> = outcomes
        .iter()
        .flat_map(|outcome| &outcome.summaries)
        .collect();
    let stale: Vec<&str> = reviewed
        .iter()
        .filter(|name| !applied.contains(name))
        .map(String::as_str)
        .collect();
    if stale.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "no image class reaches the ROM functions of these summaries; remove them: {}",
            stale.join(", ")
        )
        .into())
    }
}

/// Builds or type-checks `class`, the `index`th of `total`.
fn build_one(
    ctx: &Checkout,
    chip: &str,
    frozen: Option<&FrozenSources>,
    class: ImageClass,
    index: usize,
    total: usize,
    depth: Depth,
) -> Outcome {
    println!(
        "check firmware: [{}/{total}] {} {}",
        index + 1,
        match depth {
            Depth::Build => "building",
            Depth::TypeCheck => "type-checking",
        },
        class.id()
    );
    let started = Instant::now();
    let result = match depth {
        Depth::Build => match frozen {
            Some(frozen) => {
                oer_hil_image::frozen::build(frozen, chip, class, None, &Default::default())
                    .map(|artifacts| artifacts.bundle.rom_summaries.clone())
            }
            None => Err("a full build needs the frozen sources".into()),
        },

        Depth::TypeCheck => oer_hil_image::check(&ctx.root, chip, class).map(|()| BTreeSet::new()),
    };
    let (summaries, failure) = match result {
        Ok(summaries) => (summaries, None),
        Err(error) => (BTreeSet::new(), Some(error.to_string())),
    };
    let outcome = Outcome {
        class,
        elapsed: started.elapsed(),
        failure,
        summaries,
    };
    println!(
        "check firmware: {} {} ({}s)",
        class.id(),
        if outcome.failure.is_some() {
            "failed"
        } else {
            "passed"
        },
        outcome.elapsed.as_secs()
    );
    outcome
}

pub fn summary(outcomes: &[Outcome]) -> String {
    let mut text = String::new();
    for outcome in outcomes {
        let status = if outcome.failure.is_some() {
            "FAIL"
        } else {
            "PASS"
        };
        text.push_str(&format!(
            "{status} {:<36} {:>5}s",
            outcome.class.id(),
            outcome.elapsed.as_secs()
        ));
        if let Some(failure) = &outcome.failure {
            let first = failure.lines().next().unwrap_or_default();
            text.push_str(&format!("  {first}"));
        }
        text.push('\n');
    }
    let failed = outcomes.iter().filter(|o| o.failure.is_some()).count();
    text.push_str(&format!(
        "check firmware: {} passed, {failed} failed\n",
        outcomes.len() - failed
    ));
    text
}

/// Fails naming every failed class.
pub fn verdict(outcomes: &[Outcome]) -> Result<()> {
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_some())
        .map(|outcome| outcome.class.id())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("HIL firmware failed for {}", failed.join(", ")).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_no_class_applied_is_stale() {
        let reviewed = ["memset".to_owned(), "memcpy".to_owned()];
        let mut first = outcome(ImageClass::SystemWatchdog, None);
        first.summaries.insert("memset".to_owned());
        let second = outcome(ImageClass::SystemPanicReset, None);
        let error = stale_summaries(&reviewed, &[first, second])
            .unwrap_err()
            .to_string();
        assert!(error.ends_with("remove them: memcpy"), "{error}");
        let mut both = outcome(ImageClass::SystemPanicReset, None);
        both.summaries
            .extend(["memset".to_owned(), "memcpy".to_owned()]);
        assert!(stale_summaries(&reviewed, &[both]).is_ok());
    }

    fn outcome(class: ImageClass, failure: Option<&str>) -> Outcome {
        Outcome {
            class,
            elapsed: Duration::from_secs(3),
            failure: failure.map(str::to_owned),
            summaries: BTreeSet::new(),
        }
    }

    #[test]
    fn a_change_affects_the_classes_whose_last_build_compiled_it() {
        let builds = tempfile::tempdir().unwrap();
        let record = |class: ImageClass, files: &[&str]| {
            let directory = builds.path().join("snapshot-builds/abc").join(format!(
                "{}-{}",
                class.id(),
                oer_hil_image_class::NETWORK
            ));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("source-inputs.json"),
                serde_json::json!({
                    "schema": oer_image::source_inputs::SCHEMA,
                    "files": files,
                })
                .to_string(),
            )
            .unwrap();
        };
        for class in ImageClass::ALL {
            record(class, &["crates/common/src/lib.rs"]);
        }
        let [wifi, other] = [ImageClass::SystemPanicReset, ImageClass::SystemWatchdog];
        record(
            wifi,
            &["crates/common/src/lib.rs", "crates/wifi/src/lib.rs"],
        );
        let changed = |paths: &[&str]| -> Vec<ImageClass> {
            affected_in(
                builds.path(),
                &paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
            )
        };
        assert_eq!(changed(&["crates/wifi/src/lib.rs"]), [wifi]);
        assert!(changed(&["docs/guide.md"]).is_empty());
        assert_eq!(
            changed(&["crates/common/src/lib.rs"]).len(),
            ImageClass::ALL.len()
        );
        assert!(!changed(&["crates/wifi/src/lib.rs"]).contains(&other));
        // Configuration and builder files are inputs like any other: a
        // class is rebuilt when its record names one.
        record(
            wifi,
            &[
                "crates/wifi/src/lib.rs",
                "hil/targets/chip-a/stack.toml",
                "tools/image/pipeline/src/lib.rs",
            ],
        );
        assert_eq!(changed(&["tools/image/pipeline/src/lib.rs"]), [wifi]);
        for unread in ["Cargo.lock", "hil/host/runner/src/main.rs", "docs/guide.md"] {
            assert!(changed(&[unread]).is_empty(), "{unread}");
        }
        // A record that lists only the compiled sources is incomplete.
        let old = builds.path().join("snapshot-builds/abc").join(format!(
            "{}-{}",
            wifi.id(),
            oer_hil_image_class::NETWORK
        ));
        std::fs::write(
            old.join("source-inputs.json"),
            serde_json::json!({"schema": 1, "files": ["crates/wifi/src/lib.rs"]}).to_string(),
        )
        .unwrap();
        assert_eq!(changed(&["docs/guide.md"]), [wifi]);
        std::fs::remove_dir_all(builds.path().join("snapshot-builds/abc").join(format!(
            "{}-{}",
            other.id(),
            oer_hil_image_class::NETWORK
        )))
        .unwrap();
        assert!(
            changed(&["docs/guide.md"]).contains(&other),
            "no record: build it"
        );
    }

    #[test]
    fn the_listing_names_every_class_with_its_features() {
        let listing = list();
        for class in ImageClass::ALL {
            let line = listing
                .lines()
                .find(|line| line.split_whitespace().next() == Some(class.id()))
                .unwrap_or_else(|| panic!("{} is not listed", class.id()));
            assert!(line.contains(class.runtime_features()), "{line}");
        }
        assert!(listing.contains("--type-check"));
    }

    #[test]
    fn no_selection_takes_every_class() {
        assert_eq!(classes(&[]), ImageClass::ALL.to_vec());
    }

    #[test]
    fn selection_keeps_catalog_order_without_repeats() {
        let first = ImageClass::ALL[0];
        let last = ImageClass::ALL[ImageClass::ALL.len() - 1];
        assert_eq!(classes(&[last, first, last]), vec![first, last]);
    }

    #[test]
    fn a_failure_does_not_hide_the_other_classes() {
        let [first, second, third] = [ImageClass::ALL[0], ImageClass::ALL[1], ImageClass::ALL[2]];
        let outcomes = [
            outcome(first, None),
            outcome(second, Some("link failed\nmore detail")),
            outcome(third, Some("stack audit")),
        ];
        let text = summary(&outcomes);
        assert!(text.contains(&format!("PASS {}", first.id())));
        assert!(text.contains("link failed"));
        assert!(!text.contains("more detail"));
        assert!(text.contains("1 passed, 2 failed"));
        assert_eq!(
            verdict(&outcomes).unwrap_err().to_string(),
            format!("HIL firmware failed for {}, {}", second.id(), third.id())
        );
    }

    #[test]
    fn all_passing_classes_pass() {
        assert!(verdict(&[outcome(ImageClass::ALL[0], None)]).is_ok());
    }

    #[test]
    fn chip_code_type_checks_each_class_that_compiles_it() {
        let graph = |packages: &[&str]| packages.iter().map(|p| (*p).to_owned()).collect();
        let graphs = vec![
            (
                ImageClass::SystemWatchdog,
                graph(&["agent", "oer-watchdog"]),
            ),
            (
                ImageClass::SystemPanicReset,
                graph(&["agent", "oer-system"]),
            ),
            (ImageClass::BootSmoke, graph(&["agent", "oer-watchdog"])),
        ];
        assert_eq!(
            image_classes(&BTreeSet::from(["oer-watchdog"]), &graphs),
            [ImageClass::SystemWatchdog, ImageClass::BootSmoke]
        );
        assert_eq!(
            image_classes(&BTreeSet::from(["oer-system"]), &graphs),
            [ImageClass::SystemPanicReset]
        );
        assert!(image_classes(&BTreeSet::from(["oer-other"]), &graphs).is_empty());
    }
}
