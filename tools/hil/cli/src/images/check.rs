//! `cargo hil images check`: build HIL firmware image classes the way
//! `cargo hil image build` does,
//! with the link-time stack, placement and application audits, and report
//! every class's outcome instead of stopping at the first failure.

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use oer_hil_model::schema::image::ImageClass;

use crate::Result;
use oer_os_process::Checkout;

/// How far each class is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Depth {
    /// The full image build and its audits.
    Build,
    /// Check the runtime with the class's build configuration.
    Check(oer_image_build::RuntimeCheck),
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
            class.build_features(oer_hil_model::image_class::NETWORK_FEATURE)
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
        if oer_hil_run::image::builds_on(root, chip, class)? {
            served.push(class);
        }
    }
    Ok(served)
}

/// Served classes whose runtime declares instrumentation, as the builder
/// reads it. No class or package names are maintained separately for CI.
pub fn instrumented(root: &std::path::Path, chip: &str) -> Result<Vec<ImageClass>> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let mut selected = Vec::new();
    for class in served(root, chip, &[])? {
        let recipe = profile
            .hil_image(class.id())
            .expect("a served class has a recipe");
        if oer_image_build::instrumentation::Instrumentation::declared(root, &recipe.package)?
            .classes()
            .next()
            .is_some()
        {
            selected.push(class);
        }
    }
    Ok(selected)
}

/// Classes built at once when `--jobs` is not given. Once the seed classes
/// built the shared dependencies, most of a class's build is its final
/// crate's fat LTO, which runs on one core and needs about 1.5 GB; half the
/// cores, at most eight, keeps both the cores and the memory busy.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).clamp(1, 8))
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
/// the `packages` with changed chip code.
pub fn reached(root: &std::path::Path, chip: &str, packages: &[String]) -> Result<Vec<ImageClass>> {
    let packages: BTreeSet<&str> = packages.iter().map(String::as_str).collect();
    let mut graphs = Vec::new();
    for class in served(root, chip, &[])? {
        graphs.push((class, oer_hil_run::image::packages(root, chip, class)?));
    }
    let classes = image_classes(&packages, &graphs);
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
                    let outcome = build_one(ctx, chip, class, index, total, depth);
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
        let reviewed = oer_image_checks::interrupts::rom_summaries(&ctx.root, &profile)?;
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
            Depth::Check(oer_image_build::RuntimeCheck::Types) => "type-checking",
            Depth::Check(oer_image_build::RuntimeCheck::Clippy) => "linting",
        },
        class.id()
    );
    let started = Instant::now();
    let result = match depth {
        Depth::Build => {
            oer_hil_run::image::build(&ctx.root, chip, class, None, &Default::default())
                .map(|artifacts| artifacts.bundle.rom_summaries.clone())
        }
        Depth::Check(check) => {
            oer_hil_run::image::check(&ctx.root, chip, class, check).map(|()| BTreeSet::new())
        }
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
