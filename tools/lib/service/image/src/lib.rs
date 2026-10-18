//! The one image pipeline: `build(ImageSpec) -> ImageBundle`.
//!
//! Every firmware image of the repository, each a HIL image class, is built
//! here by the [`staged`] pipeline: the ESP-IDF bootloader loads the platform
//! bootstrap, which stages the packed runtime into PSRAM. [`esp_idf`] builds
//! the firmware catalog's ESP-IDF images (vendor references and peers) and
//! bundles them with their own bootloader.
//!
//! The pipeline compiles with the image compiler of `oer-toolchain` under the
//! image's stack policy (its move limit and stack reserves), run the
//! [`Checks`] the caller requests (the binary analyses of
//! `oer-image-checks`: stacks, interrupts, placement), and encode
//! the complete flash contents at build time with the `espflash` library:
//! the application, the bootloader, the partition table and the OTA
//! selection, at the offsets of the chip's [`FlashMap`]. A flash only writes
//! a bundle's files (`ImageBundle::segments`). Each bundle records the
//! repository files it was built from (`source-inputs.json`), with the
//! builder sources the caller names.

pub mod build_log;
pub mod bundle;
pub mod cargo;
pub mod esp_idf;
pub mod exclusion;
pub mod source_inputs;
pub mod staged;

use std::{
    collections::BTreeSet,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

pub use build_log::{BuildLog, BuildStepFailed};

pub use cargo::{LAYOUT_SEED_ENV, Overrides};
pub use oer_chip_profile::{Boot, FlashMap};
use oer_image_policy::StackPolicy;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The packages of the pipeline itself: a caller that records its builder
/// sources starts from their closure. The image linker is a root of its
/// own: no package depends on it, every image links with it.
pub const PIPELINE: [&str; 2] = ["oer-image", "oer-image-linker"];

/// What a check of one of an image's ELF files reads.
pub struct CheckInput<'a> {
    /// The tree the image is built from.
    pub root: &'a Path,
    pub profile: &'a oer_chip_profile::Profile,
    pub policy: &'a StackPolicy,
    pub elf: &'a Path,
    /// The staged runtime's flattened image, beside its ELF.
    pub flat: Option<&'a Path>,
    /// The bundle's directory, where reports go.
    pub output: &'a Path,
}

pub use oer_image_bundle::{CheckRecord, CheckResult, CheckedElf};

/// What the checks passed with.
#[derive(Clone, Debug, Default)]
pub struct CheckOutcome {
    /// Every requested check, by name, with its result. A requested check
    /// without a result, or with one that does not pass
    /// ([`CheckResult::passes`]), fails the build.
    pub results: Vec<(String, CheckResult)>,
    /// The reports they wrote into the bundle, by file name.
    pub reports: Vec<String>,
    /// The reviewed ROM summaries an analysis applied.
    pub rom_summaries: BTreeSet<String>,
    /// Partial or conditional results they passed with.
    pub warnings: Vec<String>,
}

/// Checks of an image's ELF files that a caller requests: the binary
/// analyses of `oer-image-checks`. A build without them only compiles,
/// links and encodes.
pub trait Checks: Send + Sync {
    /// The names of the checks requested of `elf`; each gets a result in
    /// the outcome of [`Checks::runtime`] or [`Checks::bootstrap`].
    fn requested(&self, elf: CheckedElf) -> Vec<String>;
    /// Check the runtime (application) ELF.
    fn runtime(&self, input: &CheckInput<'_>) -> Result<CheckOutcome>;
    /// Check a staged boot's bootstrap ELF.
    fn bootstrap(&self, input: &CheckInput<'_>) -> Result<CheckOutcome>;
}

/// What to build.
pub struct ImageSpec {
    /// The tree the image is built from: a checkout or a source snapshot.
    pub root: PathBuf,
    /// The chip id; its profile names the boot kind and the flash map.
    pub chip: String,
    pub application: Application,
    /// The image's stack policy (`stack.toml`), relative to `root`.
    pub stack_policy: PathBuf,
    /// The seed the runtime's code and read-only data are shuffled by;
    /// `None` keeps the linker's natural order.
    pub layout_seed: Option<NonZeroU32>,
    /// Local checkouts that replace pinned dependencies; a build with any
    /// resolves through its private lock copy without `--locked`.
    pub overrides: Overrides,
    /// The sources of the code that drives the build (a HIL image builder
    /// records its dependency closure), relative to `root`: they join the
    /// source inputs.
    pub builder_inputs: BTreeSet<PathBuf>,
    /// Further repository files the build reads, relative to `root`.
    pub reads: Vec<PathBuf>,
    /// The bundle's directory; every build of an image has its own. The
    /// compile cache is the host's one [`compile_cache`].
    pub output: PathBuf,
    /// The checks to run; `None` runs none.
    pub checks: Option<Box<dyn Checks>>,
}

/// The application crate of an image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Application {
    /// Its Cargo workspace, relative to the root; the workspace's committed
    /// `Cargo.lock` is the build's catalog.
    pub workspace: PathBuf,
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

/// Build `spec`: the staged pipeline of its chip.
pub fn build(spec: &ImageSpec) -> Result<oer_image_bundle::ImageBundle> {
    let profile = profile(&spec.root, &spec.chip)?;
    staged::build(spec, &profile)
}

/// Type-check the runtime of `spec` against the committed pins with the
/// image build's target, features and compiler configuration, without code
/// generation, in its compile cache. Lints that need monomorphization
/// (`large_assignments`) and the link-time gates still need [`build`].
pub fn type_check(spec: &ImageSpec) -> Result<()> {
    // A type check makes no ELF: a requested check would pass unexamined.
    if spec.checks.is_some() {
        return Err("a type check builds no image; its checks need a build".into());
    }
    let profile = profile(&spec.root, &spec.chip)?;
    let policy = StackPolicy::load(&spec.root.join(&spec.stack_policy))?;
    let lock = exclusion::BuildLock::prepare(
        &spec.root.join(&spec.application.workspace),
        &spec.output.join("locks/check"),
    )?;
    let cache = compile_cache()?;
    // The image linker the flags name is the shared one.
    let _cache = oer_toolchain::image::lock_compile_cache(&cache)?;
    let mut command = runtime_command(spec, &profile, "check", &policy)?;
    command.env("CARGO_TARGET_DIR", &cache);
    lock.configure(&mut command);
    let manifest = spec
        .root
        .join(&spec.application.workspace)
        .join("Cargo.toml");
    cargo::ensure_fetched(&spec.root, &manifest, |command| lock.configure(command))?;
    // Through the shared runner, so a caller's output policy (xtask logs
    // child output by default) applies to the type check too.
    oer_process::run(&mut command).map_err(|error| {
        format!(
            "the {} runtime does not type-check: {error}",
            spec.application.package
        )
    })?;
    lock.validate()
}

/// The profile of `chip` at `root`, which must name its flash map.
pub fn profile(root: &Path, chip: &str) -> Result<oer_chip_profile::Profile> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    if profile.flash.is_none() {
        return Err(
            format!("platform/{chip}/chip.toml names no [flash] map for its images").into(),
        );
    }
    Ok(profile)
}

/// The Cargo `subcommand` (`build` or `check`) of `spec`'s runtime with the
/// image compiler of `policy`: the application package for the chip's
/// target, its features alone, `--locked` unless a dependency is
/// overridden.
fn runtime_command(
    spec: &ImageSpec,
    profile: &oer_chip_profile::Profile,
    subcommand: &str,
    policy: &StackPolicy,
) -> Result<std::process::Command> {
    let application = &spec.application;
    let mut command = cargo::command();
    command
        .current_dir(&spec.root)
        .arg(subcommand)
        .arg("--manifest-path")
        .arg(spec.root.join(&application.workspace).join("Cargo.toml"))
        .args(["-p", &application.package, "--bin", &application.binary])
        .args(["--release", "--target", &profile.rust_target])
        .args(application.feature_arguments())
        .env("CARGO_INCREMENTAL", "0");
    if spec.overrides.is_empty() {
        command.arg("--locked");
    }
    spec.overrides.apply(&mut command);
    if let Some(seed) = spec.layout_seed {
        command.env(LAYOUT_SEED_ENV, seed.to_string());
    }
    zeroed_inputs(&mut command, profile);
    oer_toolchain::image::configure(
        &mut command,
        &policy.image_compiler(&spec.root, &linker_cache()?, &profile.rust_target),
    )?;
    Ok(command)
}

/// Hand the image linker the input sections the chip's staged boot zeroes,
/// which it refuses initialized bytes in.
pub(crate) fn zeroed_inputs(
    command: &mut std::process::Command,
    profile: &oer_chip_profile::Profile,
) {
    command.env(
        oer_toolchain::image::ZEROED_INPUTS_ENV,
        profile.zeroed_inputs(),
    );
}

/// The host build root ([`oer_toolchain::image::host_build_root`]).
pub fn host_build_root() -> Result<PathBuf> {
    // Unit tests build into the workspace's disposable target directory,
    // never into the host's shared build root.
    #[cfg(test)]
    if std::env::var_os(oer_toolchain::image::BUILD_ROOT_ENV).is_none_or(|root| root.is_empty()) {
        return Ok(oer_process::built_root().join("target/test-build-root"));
    }
    oer_toolchain::image::host_build_root()
}

/// The host's one firmware compile cache
/// ([`oer_toolchain::image::compile_cache`]); a build holds
/// [`oer_toolchain::image::lock_compile_cache`] from its Cargo run until its
/// ELF and dependency information are copied out.
pub fn compile_cache() -> Result<PathBuf> {
    Ok(oer_toolchain::image::compile_cache(&host_build_root()?))
}

/// The target directory of the image linker, built once for every image
/// build; its path is part of every image's `RUSTFLAGS`, so it must not
/// differ between builds.
pub(crate) fn linker_cache() -> Result<PathBuf> {
    Ok(oer_toolchain::image::linker_target(&host_build_root()?))
}

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
/// does not pass, fails the build after the results are written to
/// [`files::CHECKS`](oer_image_bundle::files::CHECKS) in its directory.
pub(crate) fn apply_outcome(
    checks: &dyn Checks,
    elf: CheckedElf,
    outcome: CheckOutcome,
    bundle: &mut oer_image_bundle::ImageBundle,
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
            failed.push(format!("{name} ({elf:?}): {result}"));
        }
        bundle.checks.push(CheckRecord {
            elf,
            check: name,
            result,
        });
    }
    bundle.reports.extend(outcome.reports);
    bundle.rom_summaries.extend(outcome.rom_summaries);
    bundle.warnings.extend(outcome.warnings);
    if failed.is_empty() {
        return Ok(());
    }
    oer_durable::atomic_json(
        &bundle.path(oer_image_bundle::files::CHECKS),
        &bundle.checks,
    )?;
    Err(format!("requested checks did not pass: {}", failed.join("; ")).into())
}
