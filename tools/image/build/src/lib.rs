//! The one image pipeline: `build(ImageSpec) -> ImageBundle`.
//!
//! Every firmware image of the repository, each a HIL image class, is built
//! here by the [`staged`] pipeline: the ESP-IDF bootloader loads the platform
//! bootstrap, which stages the packed runtime into PSRAM. [`esp_idf`] builds
//! the firmware catalog's ESP-IDF images (vendor references and peers) and
//! bundles them with their own bootloader.
//!
//! The pipeline compiles with the image compiler of `oer-image-toolchain` under the
//! image's stack policy (its move limit and stack reserves), run the
//! [`Gates`] the caller requests (the binary analyses of
//! `oer-image-checks`: stacks, interrupts, placement), and encode
//! the complete flash contents at build time with the `espflash` library:
//! the application, the bootloader, the partition table and the OTA
//! selection, at the offsets of the chip's [`FlashMap`]. A flash only writes
//! a bundle's files (`ImageBundle::segments`).

#![forbid(unsafe_code)]

mod around;
pub mod build_log;
pub mod cargo;
pub mod compiler;
mod critical_section;
pub mod encode;
pub mod esp_idf;
pub mod exclusion;
mod harts;
pub mod images;
pub mod instrumentation;
mod large_values;
mod manifest;
pub mod panic_budget;
pub mod probe;
pub mod sizes;
pub mod staged;
mod strict;
pub mod task_stack;
mod test_support;

use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

pub use build_log::{BuildLog, BuildStepFailed};

pub use cargo::LAYOUT_SEED_ENV;
use oer_chip_profile::policy::StackPolicy;
pub use oer_espressif_facts::{Boot, FlashMap};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub use around::{BootFiles, around};
use oer_image_bundle::{CheckRecord, CheckResult, CheckedElf};
pub use oer_image_checks::{Check, CheckInput, CheckOutcome, Enforced, Gates};

/// The Cargo profile every image builds with (the root `Cargo.toml`).
pub const FIRMWARE_PROFILE: &str = "firmware";

/// What to build.
pub struct ImageSpec {
    /// Whether this build may be flashed or is only for analysis.
    pub purpose: oer_image_bundle::Purpose,
    /// The tree the image is built from: a checkout or a worktree.
    pub root: PathBuf,
    /// The chip id; its profile names the boot kind and the flash map.
    pub chip: String,
    pub application: Application,
    /// The image's stack policy (`stack.toml`), relative to `root`.
    pub stack_policy: PathBuf,
    /// The seed the runtime's code and read-only data are shuffled by;
    /// `None` keeps the linker's natural order.
    pub layout_seed: Option<NonZeroU32>,
    /// Variables the runtime's Cargo build sets (`Some`) or removes
    /// (`None`); the bootstrap's build removes every one.
    pub runtime_environment: Vec<(String, Option<String>)>,
    /// The bundle's directory; every build of an image has its own. The
    /// compile cache is `root`'s own [`compile_cache`].
    pub output: PathBuf,
    /// The checks to run; `None` runs none.
    pub checks: Option<Gates>,
}

/// The application crate of an image, a member of the root's one
/// workspace, whose `Cargo.lock` every image resolves against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Application {
    pub package: String,
    pub binary: String,
    pub features: Vec<String>,
    pub default_features: bool,
}

impl Application {
    /// Cargo's feature arguments for this application.
    fn feature_arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if !self.default_features {
            arguments.push("--no-default-features".to_owned());
        }
        if !self.features.is_empty() {
            arguments.push("--features".to_owned());
            arguments.push(self.features.join(","));
        }
        arguments
    }
}

/// Build `spec`: the staged pipeline of its chip, with the instrumentation
/// classes and the task-stack declaration its application makes.
pub fn build(spec: &ImageSpec) -> Result<oer_image_bundle::ImageBundle> {
    let profile = profile(&spec.root, &spec.chip)?;
    let instrumentation =
        instrumentation::Instrumentation::declared(&spec.root, &spec.application.package)?;
    let task_stack_proven = task_stack::declared_proven(&spec.root, &spec.application.package)?;
    let critical_section_hart =
        critical_section::declared(&spec.root, &spec.application.package, &profile)?;
    let harts = harts::declared(&spec.root, &spec.application.package, &profile)?;
    let panics = panic_budget::declared(&spec.root, &spec.application.package)?;
    staged::build(
        spec,
        &profile,
        &instrumentation,
        task_stack_proven,
        critical_section_hart,
        &harts,
        &panics,
        staged::cargo,
    )
}

/// A runtime check that produces no image for the link-time gates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeCheck {
    Types,
    Clippy,
}

impl RuntimeCheck {
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::Types => "check",
            Self::Clippy => "clippy",
        }
    }
}

/// Check the runtime of `spec` against the committed pins with the
/// image build's target, features and compiler configuration, without code
/// generation, in its compile cache. Lints that need monomorphization
/// (`large_assignments`) and the link-time gates still need [`build`].
pub fn check(spec: &ImageSpec, check: RuntimeCheck) -> Result<()> {
    // A type check makes no ELF: a requested check would pass unexamined.
    if spec.checks.is_some() {
        return Err("a runtime check builds no image; its gates need a build".into());
    }
    let profile = profile(&spec.root, &spec.chip)?;
    let harts = harts::declared(&spec.root, &spec.application.package, &profile)?;
    let policy = StackPolicy::load(&spec.root.join(&spec.stack_policy))?.for_image(&harts)?;
    let instrumentation =
        instrumentation::Instrumentation::declared(&spec.root, &spec.application.package)?;
    let cache = compile_cache(&spec.root, &instrumentation);
    // The image linker the flags name is this checkout's.
    let _cache = crate::compiler::lock_compile_cache(&cache)?;
    let mut command = runtime_command(
        spec,
        &profile,
        check.subcommand(),
        &policy,
        &instrumentation,
    )?;
    command.env("CARGO_TARGET_DIR", &cache);
    cargo::ensure_fetched(&spec.root)?;
    // Through the shared runner, so a caller's output policy (xtask logs
    // child output by default) applies to the type check too.
    oer_os_process::run(&mut command).map_err(|error| {
        format!(
            "the {} runtime {} failed: {error}",
            spec.application.package,
            check.subcommand()
        )
    })?;
    Ok(())
}

/// The profile of `chip` at `root`, which must name its flash map.
pub fn profile(root: &Path, chip: &str) -> Result<oer_chip_profile::Profile> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    if profile.facts.flash.is_none() {
        return Err(
            format!("platform/{chip}/chip.toml names no [flash] map for its images").into(),
        );
    }
    Ok(profile)
}

/// The Cargo `subcommand` (`build`, `check` or `clippy`) of `spec`'s runtime with the
/// image compiler of `policy` and the classes of `instrumentation`: the
/// application package for the chip's target with the firmware profile, its
/// features alone, against the committed pins.
fn runtime_command(
    spec: &ImageSpec,
    profile: &oer_chip_profile::Profile,
    subcommand: &str,
    policy: &StackPolicy,
    instrumentation: &instrumentation::Instrumentation,
) -> Result<std::process::Command> {
    let application = &spec.application;
    let mut command = cargo::command();
    command
        .current_dir(&spec.root)
        .arg(subcommand)
        .args([
            "--locked",
            "-p",
            &application.package,
            "--bin",
            &application.binary,
        ])
        .args([
            "--profile",
            FIRMWARE_PROFILE,
            "--target",
            &profile.facts.rust_target,
        ])
        .args(application.feature_arguments())
        .env("CARGO_INCREMENTAL", "0");
    for (name, value) in &spec.runtime_environment {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    if let Some(seed) = spec.layout_seed {
        command.env(LAYOUT_SEED_ENV, seed.to_string());
    }
    zeroed_inputs(&mut command, profile);
    crate::compiler::configure(
        &mut command,
        &crate::compiler::of_policy(
            policy,
            instrumentation,
            &spec.root,
            &linker_cache(&spec.root),
            &profile.facts.rust_target,
        ),
    )?;
    Ok(command)
}

/// Hand the image linker the input sections the chip's staged boot zeroes,
/// which it refuses initialized bytes in.
pub(crate) fn zeroed_inputs(
    command: &mut std::process::Command,
    profile: &oer_chip_profile::Profile,
) {
    command.env(crate::compiler::ZEROED_INPUTS_ENV, profile.zeroed_inputs());
}

/// The host's directory of the image build slots
/// ([`crate::compiler::host_build_root`]).
pub fn host_build_root() -> Result<PathBuf> {
    // Unit tests build into the workspace's disposable target directory,
    // never into the host's shared build root.
    #[cfg(test)]
    if std::env::var_os(crate::compiler::BUILD_ROOT_ENV).is_none_or(|root| root.is_empty()) {
        return Ok(oer_os_process::built_root().join("target/test-build-root"));
    }
    crate::compiler::host_build_root()
}

/// The firmware compile cache of the checkout at `root` for images built
/// with `instrumentation` ([`crate::compiler::compile_cache`]); a build
/// holds [`crate::compiler::lock_compile_cache`] from its Cargo run until
/// its ELF and dependency information are copied out.
pub fn compile_cache(root: &Path, instrumentation: &instrumentation::Instrumentation) -> PathBuf {
    crate::compiler::compile_cache(root, instrumentation)
}

/// The target directory of the image linker of the checkout at `root`;
/// its path is part of every image's encoded rustflags.
pub(crate) fn linker_cache(root: &Path) -> PathBuf {
    crate::compiler::linker_target(root)
}

pub use crate::compiler::seed_build_cache;

/// Copy `source` to `target` unless `target` already holds the same bytes, so
/// that an unchanged file keeps the timestamp Cargo compares.
fn replace_if_changed(source: &Path, target: &Path) -> Result<()> {
    let bytes = std::fs::read(source)?;
    if std::fs::read(target).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    let parent = target.parent().ok_or("target has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut staged, &bytes)?;
    staged.persist(target)?;
    Ok(())
}

/// Copy the compiled `source` out of the compile cache, which a later build
/// may overwrite, to `output/name`.
fn snapshot(source: &Path, output: &Path, name: &str) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(format!("the build left no {name}: {}", source.display()).into());
    }
    let destination = output.join(name);
    std::fs::copy(source, &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod tests;

/// Fold the outcome of the checks of `elf` into `bundle`: every requested
/// check's result into its `checks` record, the reports, summaries and
/// warnings beside it. A requested check without a result, or with one that
/// does not pass, fails the build if it is enforced (a warning otherwise),
/// after the results are written to
/// [`files::CHECKS`](oer_image_bundle::files::CHECKS) in its directory. An image
/// that declares its task stacks proven (`enforce_stack`) is also held to its
/// stack check.
pub(crate) fn apply_outcome(
    checks: &Gates,
    elf: CheckedElf,
    outcome: CheckOutcome,
    bundle: &mut oer_image_bundle::ImageBundle,
    enforce_stack: bool,
) -> Result<()> {
    let mut failed = Vec::new();
    for name in checks.requested(elf) {
        let result = outcome
            .results
            .iter()
            .find(|(check, _)| *check == name)
            .map(|(_, result)| result.clone())
            .unwrap_or_else(|| CheckResult::Failed("the check produced no result".to_owned()));
        if let CheckResult::NotApplicable(reason) = &result {
            bundle
                .warnings
                .push(format!("check `{name}` does not apply: {reason}"));
        }
        if !result.passes() {
            if checks.enforced.requires(&name) || (enforce_stack && name == oer_image_checks::STACK)
            {
                failed.push(format!("{name} ({elf:?}): {result}"));
            } else {
                bundle.warnings.push(format!(
                    "check `{name}` ({elf:?}) did not pass (reported only): {result}"
                ));
            }
        }
        bundle.checks.push(CheckRecord {
            elf,
            check: name,
            result,
        });
    }
    bundle.reports.extend(outcome.reports);
    bundle.rom_summaries.extend(outcome.rom_summaries);
    bundle.coverage_reviews.extend(outcome.coverage_reviews);
    bundle.warnings.extend(outcome.warnings);
    if failed.is_empty() {
        return Ok(());
    }
    oer_os_durable::atomic_json(
        &bundle.path(oer_image_bundle::files::CHECKS),
        &bundle.checks,
    )?;
    Err(format!("requested checks did not pass: {}", failed.join("; ")).into())
}
