//! HIL images: what each image class builds for each chip, through the one
//! image pipeline (`oer-image`), from the live tree or a frozen source
//! snapshot, and the firmware and build records the builder hands to run
//! evidence.
//!
//! This crate owns what is HIL's own about an image: the class's features
//! and network integration, the agent package of each chip, the HIL stack
//! policies, the radio observers' placement, where class builds write their
//! bundles, and the records. Every build compiles in the pipeline's one
//! shared compile cache (`oer_image::compile_cache`). Compiling, gating and encoding are
//! the pipeline's.

use std::{
    error::Error,
    path::{Path, PathBuf},
};

use serde::Serialize;

use oer_hil_image_class::NETWORK;
use oer_hil_schema::image::{FeatureDelta, ImageClass};
use oer_image::{ImageSpec, Overrides};
use oer_image_bundle::ImageBundle;
use oer_image_check_interrupts::Required;
use oer_image_checks::Gates;

pub mod builder;
pub mod frozen;
pub mod record;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Version of the `image build` artifact report on stdout.
const ARTIFACT_REPORT_SCHEMA: u16 = 4;

/// The packages whose code builds a HIL image: the pipeline's, its checks
/// and this one, which turns a class into an image spec. Their closure joins
/// every HIL image's source inputs.
const BUILDERS: [&str; 4] = [
    oer_image::PIPELINE[0],
    oer_image::PIPELINE[1],
    "oer-image-checks",
    "oer-hil-image",
];

/// The report `cargo hil image build` prints: the bundle and what passed.
#[derive(Serialize)]
pub struct ArtifactReport<'a> {
    schema: u16,
    image_class: &'a str,
    chip: &'a str,
    target: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<&'a str>,
    profile: &'a str,
    bundle: String,
    runtime_elf: String,
    application_image: String,
    application_sha256: String,
    reports: Vec<String>,
    warnings: &'a [String],
}

/// The report of `class` built as `artifacts`.
pub fn artifact_report(class: ImageClass, artifacts: &Artifacts) -> Result<ArtifactReport<'_>> {
    let bundle = &artifacts.bundle;
    Ok(ArtifactReport {
        schema: ARTIFACT_REPORT_SCHEMA,
        image_class: class.id(),
        chip: &bundle.chip,
        target: &bundle.rust_target,
        network: oer_hil_image_class::network_on(class, &bundle.chip),
        profile: class.runtime_profile(),
        bundle: bundle.directory.display().to_string(),
        runtime_elf: bundle.runtime_elf().display().to_string(),
        application_image: bundle.application().display().to_string(),
        application_sha256: oer_durable::sha256_file(&bundle.application())?,
        reports: bundle
            .reports
            .iter()
            .map(|report| bundle.path(report).display().to_string())
            .collect(),
        warnings: &bundle.warnings,
    })
}

/// A HIL image: the pipeline's bundle and what the run records beside it.
#[derive(Clone, Debug)]
pub struct Artifacts {
    pub bundle: ImageBundle,
    /// Runtime features added to or removed from the class's own.
    pub features: FeatureDelta,
    /// Host tools that produced this build, recorded in its provenance.
    pub environment: oer_hil_run_bundle_format::build::BuildEnvironment,
}

/// The profile of `chip` in the repository this runner was built from.
pub fn chip_profile(chip: &str) -> Result<oer_chip_profile::Profile> {
    oer_chip_profile::Profile::load(&oer_process::built_root(), chip)
        .map_err(|error| error.to_string().into())
}

/// Whether the runner builds and flashes `class` for `chip`: the chip's
/// runtime declares every feature of the class.
pub fn builds_on(_root: &Path, chip: &str, class: ImageClass) -> Result<bool> {
    Ok(oer_hil_image_class::enabled_features_on(class, chip).is_some())
}

/// Whether `chip`'s HIL agent at `root` builds `class`: the agent declares
/// every feature the class selects.
pub fn serves(root: &Path, chip: &str, class: ImageClass) -> Result<bool> {
    let profile = oer_chip_profile::Profile::load(root, chip).map_err(|error| error.to_string())?;
    let path = profile.hil_agent_manifest(root);
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    let declared = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} declares no features", path.display()))?;
    Ok(class
        .runtime_features()
        .split(',')
        .all(|feature| declared.contains_key(feature)))
}

/// The chips, sorted, the runner builds and flashes every one of `classes`
/// for.
pub fn chips_building(root: &Path, classes: &[ImageClass]) -> Result<Vec<String>> {
    let mut chips = Vec::new();
    for chip in oer_chip_profile::supported(root).map_err(|error| error.to_string())? {
        let mut builds = true;
        for class in classes {
            builds &= builds_on(root, &chip, *class)?;
        }
        if builds {
            chips.push(chip);
        }
    }
    Ok(chips)
}

/// The chip to build `classes` for: `requested`, which must build every
/// one of them, or else the one chip that does.
pub fn chip_for(root: &Path, classes: &[ImageClass], requested: Option<&str>) -> Result<String> {
    if let Some(chip) = requested {
        for class in classes {
            if !builds_on(root, chip, *class)? {
                return Err(format!("{chip} does not build {}", class.id()).into());
            }
        }
        return Ok(chip.to_owned());
    }
    match chips_building(root, classes)?.as_slice() {
        [chip] => Ok(chip.clone()),
        [] => Err("no chip builds every requested class".into()),
        chips => Err(format!(
            "{} build every requested class; name one with --chip",
            chips.join(", ")
        )
        .into()),
    }
}

/// The seed of a runtime image's link order: `None` keeps the linker's
/// natural order, a seed shuffles ordinary code and read-only data by it.
pub type LayoutSeed = Option<std::num::NonZeroU32>;

/// How a run builds its images from the current sources.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentBuild {
    pub layout_seed: LayoutSeed,
    /// Runtime features added to or removed from each class's own; empty
    /// outside an experiment.
    pub features: FeatureDelta,
}

/// The image spec of `class` for `chip`, from the tree at `root`.
pub(crate) fn spec(
    root: &Path,
    chip: &str,
    class: ImageClass,
    (layout_seed, features): (LayoutSeed, &FeatureDelta),
    overrides: Overrides,
    output: PathBuf,
) -> Result<ImageSpec> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let workspace = profile.hil_agent_workspace(root);
    let class_features = oer_hil_image_class::build_features_on(class, chip);
    let package = profile.hil_agent_package();
    Ok(ImageSpec {
        root: root.to_owned(),
        chip: chip.to_owned(),
        application: oer_image::Application {
            workspace: workspace.strip_prefix(root)?.to_owned(),
            binary: package.clone(),
            package,
            features: features
                .apply(&class_features)
                .split(',')
                .filter(|feature| !feature.is_empty())
                .map(str::to_owned)
                .collect(),
            default_features: false,
        },
        stack_policy: profile.hil_stack_policy(),
        layout_seed,
        overrides,
        builder_inputs: builder::closure(root, &BUILDERS)?,
        reads: Vec::new(),
        output,
        // HIL images always request every check; each applies where the
        // chip's data names what it checks.
        // A diagnostic image's interrupt stacks may be `partial + ?`, with
        // their holes named.
        checks: Some(Box::new(Gates::all(if class.diagnostic() {
            Required::Partial
        } else {
            Required::Proven
        }))),
    })
}

/// Build `class` for `chip` from the live tree at `root`, with the caller's
/// local overrides, into the class's directory below `target/hil/<chip>`.
pub fn build(
    root: &Path,
    chip: &str,
    class: ImageClass,
    layout_seed: LayoutSeed,
    features: &FeatureDelta,
) -> Result<Artifacts> {
    let output = root.join("target/hil").join(chip).join(format!(
        "{}-{}-{}{}{}",
        class.runtime_profile(),
        class.id(),
        NETWORK,
        seed_suffix(layout_seed),
        features.suffix()
    ));
    let spec = spec(
        root,
        chip,
        class,
        (layout_seed, features),
        Overrides::from_environment()?,
        output,
    )?;
    built(&spec, features)
}

/// Run the pipeline for `spec` and keep the HIL record's context.
pub(crate) fn built(spec: &ImageSpec, features: &FeatureDelta) -> Result<Artifacts> {
    let bundle = oer_image::build(spec)?;
    for warning in &bundle.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(Artifacts {
        bundle,
        features: features.clone(),
        environment: oer_hil_run_bundle::build::capture_environment(),
    })
}

/// Type-check the runtime of `class` for `chip` against the committed
/// pins, with the image build's target, features and compiler
/// configuration, in the image pipeline's shared compile cache. A pre-push check:
/// lints that need monomorphization (`large_assignments`) and the link-time
/// gates still need `cargo hil image build`.
pub fn check(root: &Path, chip: &str, class: ImageClass) -> Result<()> {
    let output =
        root.join("target/hil")
            .join(chip)
            .join(format!("check-{}-{}", class.id(), NETWORK));
    let mut spec = spec(
        root,
        chip,
        class,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        output,
    )?;
    // A type check makes no image for the class's gates to examine; they
    // run on `cargo hil image build`.
    spec.checks = None;
    oer_image::type_check(&spec)
        .map_err(|error| format!("the {} runtime: {error}", class.id()).into())
}

/// The names of the packages `class`'s runtime for `chip` compiles: the
/// agent's normal dependency graph for the chip target with the class's
/// features, as [`check`] type-checks it.
pub fn packages(
    root: &Path,
    chip: &str,
    class: ImageClass,
) -> Result<std::collections::BTreeSet<String>> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let features = oer_hil_image_class::build_features_on(class, chip);
    let mut command = oer_process::command(oer_toolchain::cargo_program());
    command
        .current_dir(root)
        .args(["tree", "--manifest-path"])
        .arg(profile.hil_agent_workspace(root).join("Cargo.toml"))
        .args([
            "-p",
            &profile.hil_agent_package(),
            "--target",
            &profile.rust_target,
            "--locked",
        ])
        .args(["--no-default-features", "--features", &features])
        .args(["-e", "normal", "--prefix", "none"]);
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree of the {} runtime failed: {}",
            class.id(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(tree_packages(&String::from_utf8_lossy(&output.stdout)))
}

/// Package names from `cargo tree --prefix none` lines (`name vX.Y.Z ...`).
fn tree_packages(tree: &str) -> std::collections::BTreeSet<String> {
    tree.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// Where the host keeps the source snapshots its builds read.
pub fn source_snapshot_store() -> Result<PathBuf> {
    Ok(oer_image::host_build_root()?.join("source-snapshots"))
}

/// The source files of every snapshot, by their SHA-256: beside the shared
/// run store's runs, which keep naming them after a build cache is cleared.
pub fn source_objects() -> Result<PathBuf> {
    Ok(oer_hil_run_bundle::RunStore::shared()?.sources())
}

/// The host build root of `chip` images.
pub fn chip_build_root(chip: &str) -> Result<PathBuf> {
    Ok(oer_image::host_build_root()?.join(chip))
}

/// The artifact directory suffix of a seeded build, so a seed never reuses
/// another layout's artifacts.
pub(crate) fn seed_suffix(layout_seed: LayoutSeed) -> String {
    layout_seed.map_or_else(String::new, |seed| format!("-seed{seed}"))
}

/// Fails when the root lock or a chip's HIL agent workspace pulls in a
/// package the chip's profile forbids its HIL agent (`[hil]
/// forbidden-packages`): the source-only radio graph.
pub fn ensure_vendor_dependencies_absent(root: &Path) -> Result<()> {
    for profile in oer_chip_profile::Profile::all(root)? {
        let Some(hil) = &profile.hil else {
            continue;
        };
        let workspace = profile.hil_agent_workspace(root);
        for path in [
            root.join("Cargo.lock"),
            workspace.join("Cargo.toml"),
            workspace.join("Cargo.lock"),
        ] {
            let contents = std::fs::read_to_string(&path)?;
            for package in &hil.forbidden_packages {
                let forbidden = format!("name = \"{package}\"");
                if contents.contains(&forbidden) {
                    return Err(format!(
                        "{} pulls `{forbidden}` into the source-only graph",
                        path.display()
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

/// The failed build step behind `error`, when a step failed.
pub fn failed_step<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a oer_image::BuildStepFailed> {
    let mut cause = Some(error);
    while let Some(current) = cause {
        if let Some(failed) = current.downcast_ref::<oer_image::BuildStepFailed>() {
            return Some(failed);
        }
        cause = current.source();
    }
    None
}

/// Build `classes` at `base` in a detached worktree and in this checkout,
/// then compare each pair; fails unless every pair is equivalent.
pub fn compare_images(
    root: &Path,
    chip: &str,
    base: &str,
    classes: &[ImageClass],
    review: &oer_image_compare::Review,
) -> Result<()> {
    let worktree =
        oer_process::git::Worktree::detached(root, &root.join("target/compare/base"), base)?;
    let worktree = worktree.path();
    let mut equivalent = true;
    for &class in classes {
        // The image pipeline builds the worktree's sources with its own
        // builder: the comparison is of the sources alone.
        let [base_elf, current_elf] = [worktree, root].map(|root| {
            build(root, chip, class, None, &Default::default())
                .map(|artifacts| artifacts.bundle.runtime_elf())
        });
        let comparison = oer_image_compare::compare_elf(&base_elf?, &current_elf?, review)?;
        equivalent &= comparison.equivalent(&review.allowed);
    }
    if equivalent {
        println!("images equivalent modulo placement against {base}");
        Ok(())
    } else {
        Err(format!("images differ from {base} beyond placement").into())
    }
}

#[cfg(test)]
mod tests;
