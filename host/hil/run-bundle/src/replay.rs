//! The archived firmware of an earlier run, checked before a run replays it.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use crate::Result;
use oer_hil_run_bundle_format::lab::LabProvenance;
use oer_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_hil_run_bundle_format::run::RunManifest;
use oer_hil_run_bundle_format::run::RunState;
use oer_hil_run_bundle_format::run::validation::read_json;
use oer_hil_run_bundle_format::run::validation::validate_manifest;

#[derive(Debug, Serialize)]
pub struct ArchivedFirmware {
    pub run_id: String,
    /// The chip the source run's image was built for.
    pub target: String,
    pub image: oer_hil_schema::image::ImageClass,
    pub application_path: PathBuf,
    pub application_sha256: String,
    pub(super) source_directory: PathBuf,
    pub(super) artifact: oer_hil_run_bundle_format::run::FirmwareArtifact,
}

/// The run directory a checkout's `runs` resolves to. A checkout links it to
/// the run store shared by every checkout; the link itself is followed,
/// while links inside bundles stay refused.
fn runs_directory(runs: &Path) -> Result<std::path::PathBuf> {
    if fs::symlink_metadata(runs).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(fs::canonicalize(runs)?);
    }
    Ok(runs.to_owned())
}

pub fn archived_firmware(
    root: &Path,
    target: &str,
    run_id: &str,
    image: oer_hil_schema::image::ImageClass,
) -> Result<ArchivedFirmware> {
    let runs = root.join(crate::store::CHECKOUT_RUNS);
    let manifest = validate_source(&runs, target, run_id)?;
    let run_directory = runs_directory(&runs)?.join(run_id);
    let artifact = manifest
        .firmware
        .iter()
        .find(|artifact| artifact.image == image)
        .ok_or_else(|| {
            format!(
                "HIL run `{run_id}` has no archived `{}` firmware",
                image.id()
            )
        })?;
    Ok(ArchivedFirmware {
        run_id: run_id.to_owned(),
        target: manifest.target.clone(),
        image,
        application_path: run_directory.join(&artifact.application_path),
        application_sha256: artifact.application_sha256.clone(),
        source_directory: run_directory.clone(),
        artifact: artifact.clone(),
    })
}

/// Check the run `run_id` of `chip` below `runs` before its firmware is
/// replayed: its manifest, laboratory provenance and every firmware
/// artifact's files against their recorded digests.
fn validate_source(runs: &Path, chip: &str, run_id: &str) -> Result<RunManifest> {
    let run_directory = runs_directory(runs)?.join(run_id);
    let manifest_path = run_directory.join("manifest.json");
    require_regular_file(&manifest_path)?;
    let manifest: RunManifest = read_json(&manifest_path)?;
    validate_manifest(&manifest, chip, &run_directory)?;
    if manifest.state == RunState::Running {
        return Err(format!("HIL run `{}` is still running", manifest.run_id).into());
    }
    validate_lab_provenance(&run_directory, &manifest)?;
    validate_firmware(&run_directory, &manifest)?;
    Ok(manifest)
}

fn validate_lab_provenance(run_directory: &Path, manifest: &RunManifest) -> Result<()> {
    let Some(path) = manifest.lab_provenance_path.as_deref() else {
        // Schema-2 bundles created before lab provenance remain valid history.
        return Ok(());
    };
    let expected = Path::new("lab-provenance.json");
    if path != expected {
        return Err(format!(
            "HIL run `{}` has a non-canonical lab provenance path `{}`",
            manifest.run_id,
            path.display()
        )
        .into());
    }
    require_regular_file_below(run_directory, path)?;
    let provenance: LabProvenance = read_json(&run_directory.join(path))?;
    if provenance.scope == oer_hil_run_bundle_format::lab::ObservationScope::System {
        let plan: oer_hil_run_bundle_format::run::RunPlan =
            read_json(&run_directory.join("plan.json"))?;
        let selected: Vec<_> = plan
            .entries
            .iter()
            .filter(|entry| {
                entry.disposition == oer_hil_run_bundle_format::run::PlanDisposition::Selected
            })
            .collect();
        if plan.run_id != manifest.run_id
            || plan.schema != RUN_SCHEMA
            || selected.is_empty()
            || selected
                .iter()
                .any(|entry| entry.requirements.is_none_or(|required| required.network()))
        {
            return Err(
                "system-only lab provenance requires an explicit plan with no network dependencies"
                    .into(),
            );
        }
        for entry in selected {
            let snapshot = PathBuf::from("scenarios")
                .join(&entry.scenario)
                .join("scenario.json");
            validate_relative_path(&snapshot, "scenario snapshot")?;
            require_regular_file_below(run_directory, &snapshot)?;
            let scenario = oer_hil_scenario_catalog::Header::from_snapshot(&fs::read(
                run_directory.join(snapshot),
            )?)?;
            if scenario.id != entry.scenario || scenario.repetitions != entry.repetitions {
                return Err(
                    "system-only lab provenance disagrees with the selected scenario snapshot"
                        .into(),
                );
            }
        }
    }
    provenance.validate_binding(
        &manifest.cell.cell_id,
        &manifest.cell.device_id,
        manifest.started_unix_millis,
        manifest.finished_unix_millis,
    )
}

fn validate_firmware(run_directory: &Path, manifest: &RunManifest) -> Result<()> {
    let mut images = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for artifact in &manifest.firmware {
        validate_replay_origin(manifest, artifact)?;
        let expected_path = PathBuf::from("firmware")
            .join(artifact.image.id())
            .join("application.bin");
        if artifact.application_path != expected_path
            || !images.insert(artifact.image.id())
            || !paths.insert(artifact.application_path.clone())
        {
            return Err(format!(
                "HIL run `{}` has non-canonical firmware provenance for `{}`",
                manifest.run_id,
                artifact.image.id()
            )
            .into());
        }
        let required = artifact.required_subjects();
        for subject in artifact.subjects() {
            // A staged image's digests are recorded even for an older bundle
            // that did not archive the files.
            let wanted = subject.file == "runtime.elf" || required.contains(&subject.file);
            match (wanted, subject.sha256) {
                (true, Some(sha256)) => validate_sha256(sha256, subject.kind, &manifest.run_id)?,
                (false, None) => {}
                (true, None) | (false, Some(_)) => {
                    return Err(format!(
                        "HIL run `{}` records the wrong subjects for its image `{}`: {}",
                        manifest.run_id,
                        artifact.image.id(),
                        subject.kind
                    )
                    .into());
                }
            }
            validate_optional_firmware_file(
                run_directory,
                &mut paths,
                subject.path.map(PathBuf::as_path),
                subject.size_bytes,
                subject.sha256.unwrap_or_default(),
                &PathBuf::from("firmware")
                    .join(artifact.image.id())
                    .join(subject.file),
                subject.kind,
            )?;
        }
        verify_indexed_file(
            run_directory,
            &artifact.application_path,
            artifact.application_size_bytes,
            &artifact.application_sha256,
            "firmware application",
        )?;
    }
    Ok(())
}

fn validate_optional_firmware_file(
    run_directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
    path: Option<&Path>,
    size_bytes: Option<u64>,
    sha256: &str,
    expected_path: &Path,
    kind: &str,
) -> Result<()> {
    match (path, size_bytes) {
        (None, None) => Ok(()),
        (Some(path), Some(size_bytes))
            if path == expected_path && paths.insert(path.to_owned()) =>
        {
            verify_indexed_file(run_directory, path, size_bytes, sha256, kind)
        }
        _ => Err(format!(
            "HIL {kind} has incomplete, duplicate or non-canonical archive provenance"
        )
        .into()),
    }
}

fn validate_replay_origin(
    manifest: &RunManifest,
    artifact: &oer_hil_run_bundle_format::run::FirmwareArtifact,
) -> Result<()> {
    let Some(origin) = &artifact.replayed_from else {
        return Ok(());
    };
    if origin.source_run_id == manifest.run_id
        || !is_single_normal_component(Path::new(&origin.source_run_id))
    {
        return Err(format!(
            "HIL run `{}` has inconsistent replay origin for `{}`",
            manifest.run_id,
            artifact.image.id()
        )
        .into());
    }
    Ok(())
}

fn verify_indexed_file(
    run_directory: &Path,
    relative_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    kind: &str,
) -> Result<()> {
    validate_relative_path(relative_path, kind)?;
    validate_sha256(expected_sha256, kind, &relative_path.display().to_string())?;
    let path = run_directory.join(relative_path);
    // A compressed file is checked by its uncompressed bytes.
    require_regular_file_below(
        run_directory,
        &oer_hil_run_bundle_format::archived::stored_path(relative_path),
    )?;
    let oer_hil_run_bundle_format::archived::Identity {
        size_bytes: actual_size,
        sha256: actual_sha256,
    } = oer_hil_run_bundle_format::archived::archived_identity(&path)?;
    if actual_size != expected_size {
        return Err(format!(
            "HIL {kind} `{}` has size {actual_size}, expected {expected_size}",
            path.display()
        )
        .into());
    }
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "HIL {kind} `{}` has SHA-256 {actual_sha256}, expected {expected_sha256}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn validate_relative_path(path: &Path, kind: &str) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "HIL {kind} path is not a safe relative path: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn is_single_normal_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn validate_sha256(value: &str, kind: &str, owner: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("HIL {kind} `{owner}` has an invalid SHA-256 digest").into());
    }
    Ok(())
}

fn require_regular_file_below(root: &Path, relative_path: &Path) -> Result<()> {
    let mut current = root.to_owned();
    let component_count = relative_path.components().count();
    for (index, component) in relative_path.components().enumerate() {
        let Component::Normal(component) = component else {
            return Err(format!(
                "HIL file path is not a safe relative path: {}",
                relative_path.display()
            )
            .into());
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        let last = index + 1 == component_count;
        if (last && !metadata.file_type().is_file()) || (!last && !metadata.file_type().is_dir()) {
            return Err(format!(
                "HIL bundle path has an unexpected file type: {}",
                current.display()
            )
            .into());
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(format!("HIL bundle path is not a regular file: {}", path.display()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
