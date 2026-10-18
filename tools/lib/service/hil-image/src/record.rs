//! The image builder's records for run evidence: build-only records, the
//! firmware a run flashes and the recipe a replay checks them against.
//!
//! Build-only records are immutable build subjects without fabricated
//! hardware observations.
use crate::{Artifacts, Result, chip_profile, frozen::build_slots};
use oer_tool_base_durable::atomic_json;
use oer_tool_data_hil_run_bundle_format::run::FirmwareArtifact;
use oer_tool_data_hil_run_bundle_format::run::RepositoryProvenance;
use oer_tool_data_hil_schema::image::ImageClass;
use oer_tool_service_hil_run_bundle::build;
use oer_tool_service_hil_run_bundle::replay::FirmwareRecipe;
use oer_tool_service_hil_run_bundle::run::FirmwareArchive;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
struct Record {
    schema: u16,
    kind: &'static str,
    target: String,
    repository: RepositoryProvenance,
    firmware: Vec<FirmwareArtifact>,
}

/// Publish atomically after construction. The content identity covers the entire
/// inventory; build records contain no scenario, outcome or repetition count.
pub fn publish(
    root: &Path,
    snapshot: &Path,
    objects: &Path,
    class: ImageClass,
    artifacts: &Artifacts,
) -> Result<PathBuf> {
    let chip = artifacts.bundle.chip.clone();
    let target = root.join("target/hil").join(&chip);
    let builds = target.join("builds");
    fs::create_dir_all(&builds)?;
    let staging = tempfile::Builder::new()
        .prefix(".build-")
        .tempdir_in(&builds)?;
    let directory = staging.path();
    let (frozen, sources, materials) =
        build::archive_snapshot(snapshot, objects, directory, &target, &build_slots(&chip)?)?;
    frozen.verify_unchanged()?;
    let (artifact, _) = firmware::archive(
        &FirmwareArchive {
            directory,
            target_directory: &target,
            source_root: frozen.repository(),
            source_materials: &sources,
            snapshot_materials: &materials,
        },
        class,
        artifacts,
    )?;
    let source = sources
        .first()
        .filter(|s| s.name == "repository")
        .ok_or("build snapshot has no primary repository")?;
    atomic_json(
        &directory.join("build.json"),
        &Record {
            schema: 1,
            kind: "open-esp-radio-build",
            target: chip,
            repository: RepositoryProvenance {
                commit: source.commit.clone(),
                dirty: source.dirty,
                workspace_sha256: source.workspace_sha256.clone(),
            },
            firmware: vec![artifact],
        },
    )?;
    let inventory = oer_tool_data_hil_run_bundle_format::run::inventory(directory)?;
    let id = oer_tool_base_durable::sha256_bytes(&serde_json::to_vec(&inventory)?);
    let destination = builds.join(id);
    if destination.exists() {
        if !fs::symlink_metadata(&destination)?.file_type().is_dir()
            || oer_tool_data_hil_run_bundle_format::run::inventory(&destination)? != inventory
        {
            return Err("published build record does not match its identity".into());
        }
        return Ok(destination);
    }
    fs::rename(directory, &destination)?;
    Ok(destination)
}

pub mod firmware;

/// The builder's recipe: the Rust target of a chip's images and the runtime
/// features a class builds with for its network integration.
pub struct Recipe;

impl FirmwareRecipe for Recipe {
    fn rust_target(&self, chip: &str) -> oer_tool_service_hil_run_bundle::Result<String> {
        Ok(chip_profile(chip)?.rust_target)
    }

    fn runtime_features(
        &self,
        image: ImageClass,
        network: Option<&str>,
    ) -> oer_tool_service_hil_run_bundle::Result<String> {
        Ok(match network {
            Some(oer_tool_data_hil_image_class::NETWORK) => {
                image.build_features(oer_tool_data_hil_image_class::NETWORK_FEATURE)
            }
            Some(other) => {
                return Err(format!(
                    "unknown network integration `{other}` (expected {})",
                    oer_tool_data_hil_image_class::NETWORK
                )
                .into());
            }
            // An ESP-IDF application links no network integration.
            None => image.runtime_features().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests;
