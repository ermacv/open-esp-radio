//! Offline integrity verification for immutable HIL run bundles.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use crate::Result;
use crate::build::build_id;
use crate::run::sha256_file;
use oer_tool_data_hil_run_bundle_format::build::BUILD_PROVENANCE_SCHEMA;
use oer_tool_data_hil_run_bundle_format::build::BuildProvenance;
use oer_tool_data_hil_run_bundle_format::build::BuildReproducibility;
use oer_tool_data_hil_run_bundle_format::build::BuildSubject;
use oer_tool_data_hil_run_bundle_format::build::BuildSubjectRole;
use oer_tool_data_hil_run_bundle_format::build::SourceMaterial;
use oer_tool_data_hil_run_bundle_format::build::SourceRebuildStatus;
use oer_tool_data_hil_run_bundle_format::lab::LabProvenance;
use oer_tool_data_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_tool_data_hil_run_bundle_format::run::RunManifest;
use oer_tool_data_hil_run_bundle_format::run::RunState;
use oer_tool_data_hil_run_bundle_format::run::SuiteResult;
use oer_tool_data_hil_run_bundle_format::run::validation::read_json;
use oer_tool_data_hil_run_bundle_format::run::validation::validate_manifest;
use oer_tool_data_hil_run_bundle_format::run::validation::validate_suite;

#[derive(Debug, Serialize)]
pub struct VerificationCompletion {
    pub schema: u16,
    /// The chips of the verified runs.
    pub chips: BTreeSet<String>,
    pub status: &'static str,
    pub runs: usize,
    pub attachments: usize,
    pub firmware_artifacts: usize,
    pub verified_run_ids: Vec<String>,
}

/// What the image builder derives from a build's recorded selection.
/// Verification checks each build record against it, so a record cannot
/// claim a target or features its image class does not build with.
pub trait FirmwareRecipe {
    /// The Rust target `chip` images build for.
    fn rust_target(&self, chip: &str) -> Result<String>;

    /// The runtime features `image` builds with before any feature delta,
    /// for the recorded network integration, or none for an image that
    /// links no network integration.
    fn runtime_features(
        &self,
        image: oer_tool_data_hil_schema::image::ImageClass,
        network: Option<&str>,
    ) -> Result<String>;
}

#[derive(Debug, Serialize)]
pub struct ArchivedFirmware {
    pub run_id: String,
    /// The chip the source run's image was built for.
    pub target: String,
    pub image: oer_tool_data_hil_schema::image::ImageClass,
    pub application_path: PathBuf,
    pub application_sha256: String,
    pub build_id: Option<String>,
    pub(super) source_directory: PathBuf,
    pub(super) integrity_sha256: String,
    pub(super) repository: oer_tool_data_hil_run_bundle_format::run::RepositoryProvenance,
    pub(super) artifact: oer_tool_data_hil_run_bundle_format::run::FirmwareArtifact,
    pub(super) build_provenance: Option<BuildProvenance>,
}

/// Verify the run `run_id`, or every run, of the checkout at `root`; with
/// `chip`, only that chip's runs, and `run_id` must be one of them.
pub fn verify(
    root: &Path,
    chip: Option<&str>,
    run_id: Option<&str>,
    recipe: &dyn FirmwareRecipe,
) -> Result<VerificationCompletion> {
    verify_at(
        &root.join(crate::store::CHECKOUT_RUNS),
        chip,
        run_id,
        recipe,
    )
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
    image: oer_tool_data_hil_schema::image::ImageClass,
    recipe: &dyn FirmwareRecipe,
) -> Result<ArchivedFirmware> {
    verify(root, Some(target), Some(run_id), recipe)?;
    let run_directory = runs_directory(&root.join(crate::store::CHECKOUT_RUNS))?.join(run_id);
    let manifest: RunManifest = read_json(&run_directory.join("manifest.json"))?;
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
    let build_provenance = artifact
        .build_provenance_path
        .as_ref()
        .map(|path| read_json(&run_directory.join(path)))
        .transpose()?;
    Ok(ArchivedFirmware {
        run_id: run_id.to_owned(),
        target: manifest.target.clone(),
        image,
        application_path: run_directory.join(&artifact.application_path),
        application_sha256: artifact.application_sha256.clone(),
        build_id: artifact.build_id.clone(),
        source_directory: run_directory.clone(),
        integrity_sha256: sha256_file(&run_directory.join("integrity.json"))?,
        repository: manifest.repository,
        artifact: artifact.clone(),
        build_provenance,
    })
}

pub fn verify_at(
    runs: &Path,
    chip: Option<&str>,
    run_id: Option<&str>,
    recipe: &dyn FirmwareRecipe,
) -> Result<VerificationCompletion> {
    let runs_directory = runs_directory(runs)?;
    let run_directories = select_run_directories(&runs_directory, run_id)?;
    let mut attachments = 0;
    let mut firmware_artifacts = 0;
    let mut verified_run_ids = Vec::with_capacity(run_directories.len());
    let mut chips = BTreeSet::new();

    for run_directory in run_directories {
        let manifest_path = run_directory.join("manifest.json");
        require_regular_file(&manifest_path)?;
        let manifest: RunManifest = read_json(&manifest_path)?;
        // Every chip's runs share the store: a chip filter skips the others,
        // and refuses a named run of another chip.
        if let Some(chip) = chip
            && run_id.is_none()
            && manifest.target != chip
        {
            continue;
        }
        let target = chip.unwrap_or(&manifest.target).to_owned();
        validate_manifest(&manifest, &target, &run_directory)?;
        if manifest.state == RunState::Running {
            return Err(format!(
                "HIL run `{}` is still running and has no immutable integrity seal",
                manifest.run_id
            )
            .into());
        }
        validate_lab_provenance(&run_directory, &manifest)?;
        validate_firmware(&run_directory, &manifest, recipe)?;
        firmware_artifacts += manifest.firmware.len();

        if manifest.state == RunState::Completed {
            let suite_path = run_directory.join("suite.json");
            require_regular_file(&suite_path)?;
            let suite: SuiteResult = read_json(&suite_path)?;
            validate_suite(&suite, &manifest)?;
            attachments += validate_attachments(&run_directory, &suite)?;
        }
        validate_integrity_index(&run_directory, &manifest)?;
        validate_observer(&runs_directory, &manifest)?;
        chips.insert(target);
        verified_run_ids.push(manifest.run_id);
    }
    validate_observer_store(&runs_directory)?;

    Ok(VerificationCompletion {
        schema: RUN_SCHEMA,
        chips,
        status: "verified",
        runs: verified_run_ids.len(),
        attachments,
        firmware_artifacts,
        verified_run_ids,
    })
}

/// A run's observer record is a reference whose build is present in the
/// store and hashes to its name.
fn validate_observer(runs_directory: &Path, manifest: &RunManifest) -> Result<()> {
    let Some(record) = manifest.observer() else {
        return Ok(());
    };
    let store = crate::store::RunStore::of_runs(runs_directory)?;
    oer_tool_data_hil_run_bundle_format::observer::store::attach(record, store.observers())
        .map_err(|error| {
            format!(
                "HIL run `{}` has an invalid observer record: {error}",
                manifest.run_id
            )
        })?;
    Ok(())
}

/// Every stored observer build is named by the digest of its bytes.
fn validate_observer_store(runs_directory: &Path) -> Result<()> {
    use oer_tool_data_hil_run_bundle_format::observer::store as observer_store;
    let directory = crate::store::RunStore::of_runs(runs_directory)?
        .observers()
        .join(observer_store::DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        let digest = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|name| observer_store::is_digest(name))
            .ok_or_else(|| format!("unexpected observer store entry {}", path.display()))?;
        if oer_tool_base_durable::sha256_bytes(&fs::read(&path)?) != digest {
            return Err(format!(
                "observer build {} does not hash to its name",
                path.display()
            )
            .into());
        }
    }
    Ok(())
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
    if provenance.scope == oer_tool_data_hil_run_bundle_format::lab::ObservationScope::System {
        let plan: oer_tool_data_hil_run_bundle_format::run::RunPlan =
            read_json(&run_directory.join("plan.json"))?;
        let selected: Vec<_> = plan
            .entries
            .iter()
            .filter(|entry| {
                entry.disposition
                    == oer_tool_data_hil_run_bundle_format::run::PlanDisposition::Selected
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
            let scenario = oer_tool_data_hil_scenario_catalog::Header::from_snapshot(&fs::read(
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

fn validate_integrity_index(run_directory: &Path, manifest: &RunManifest) -> Result<()> {
    oer_tool_data_hil_run_bundle_format::run::integrity::verify(run_directory, &manifest.run_id)
        .map(drop)
}

fn select_run_directories(runs_directory: &Path, run_id: Option<&str>) -> Result<Vec<PathBuf>> {
    require_directory(runs_directory)?;
    if let Some(run_id) = run_id {
        if !is_single_normal_component(Path::new(run_id)) {
            return Err(format!("invalid HIL run ID `{run_id}`").into());
        }
        let directory = runs_directory.join(run_id);
        require_directory(&directory)?;
        return Ok(vec![directory]);
    }

    let mut entries = fs::read_dir(runs_directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut directories = Vec::with_capacity(entries.len());
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            return Err(format!(
                "HIL runs directory contains a non-directory entry: {}",
                entry.path().display()
            )
            .into());
        }
        directories.push(entry.path());
    }
    Ok(directories)
}

fn validate_firmware(
    run_directory: &Path,
    manifest: &RunManifest,
    recipe: &dyn FirmwareRecipe,
) -> Result<()> {
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
        match (&artifact.build_id, &artifact.build_provenance_path) {
            (None, None) => {}
            (Some(build_id), Some(path)) => {
                let expected = PathBuf::from("firmware")
                    .join(artifact.image.id())
                    .join("build-provenance.json");
                if path != &expected || !paths.insert(path.clone()) {
                    return Err(format!(
                        "HIL run `{}` has non-canonical build provenance for `{}`",
                        manifest.run_id,
                        artifact.image.id()
                    )
                    .into());
                }
                validate_build_provenance(
                    run_directory,
                    manifest,
                    artifact,
                    build_id,
                    path,
                    recipe,
                )?;
            }
            _ => {
                return Err(format!(
                    "HIL run `{}` has incomplete build provenance reference",
                    manifest.run_id
                )
                .into());
            }
        }
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

fn validate_build_provenance(
    run_directory: &Path,
    manifest: &RunManifest,
    artifact: &oer_tool_data_hil_run_bundle_format::run::FirmwareArtifact,
    expected_build_id: &str,
    path: &Path,
    recipe: &dyn FirmwareRecipe,
) -> Result<()> {
    validate_relative_path(path, "build provenance")?;
    require_regular_file_below(run_directory, path)?;
    let provenance: BuildProvenance = read_json(&run_directory.join(path))?;
    if provenance.schema != BUILD_PROVENANCE_SCHEMA
        || provenance.build_id != expected_build_id
        || provenance.build_id != build_id(&provenance.subjects)
        || provenance.build_type != "open-esp-radio-hil-firmware/v1"
        || provenance.parameters.image != artifact.image
        || provenance.parameters.runtime_profile != artifact.image.runtime_profile()
        || provenance.parameters.runtime_features
            != provenance
                .parameters
                .features
                .apply(&recipe.runtime_features(
                    artifact.image,
                    provenance.parameters.network.as_deref(),
                )?)
        || provenance.parameters.target != recipe.rust_target(&manifest.target)?
        // The manifest's seed is the one the build record says the image
        // was linked with, so a run cannot claim another layout.
        || provenance.parameters.layout_seed != artifact.layout_seed
        || provenance.reproducibility != BuildReproducibility::Unverified
    {
        return Err(format!(
            "HIL run `{}` has build provenance inconsistent with `{}`",
            manifest.run_id,
            artifact.image.id()
        )
        .into());
    }
    // The build record lists the application, the subjects of the image's
    // boot flow and the runtime ELF, in that order.
    let subject = |role, file: &str| -> Result<BuildSubject> {
        let record = artifact
            .subjects()
            .into_iter()
            .find(|subject| subject.file == file)
            .ok_or("unknown firmware subject")?;
        Ok(BuildSubject {
            role,
            path: record
                .path
                .cloned()
                .ok_or_else(|| format!("build provenance requires an archived {}", record.kind))?,
            size_bytes: record
                .size_bytes
                .ok_or_else(|| format!("build provenance requires a {} size", record.kind))?,
            sha256: record
                .sha256
                .ok_or_else(|| format!("build provenance requires a {} digest", record.kind))?
                .to_owned(),
        })
    };
    let mut expected_subjects = vec![BuildSubject {
        role: BuildSubjectRole::Application,
        path: artifact.application_path.clone(),
        size_bytes: artifact.application_size_bytes,
        sha256: artifact.application_sha256.clone(),
    }];
    expected_subjects.push(subject(BuildSubjectRole::BootstrapElf, "bootstrap.elf")?);
    expected_subjects.push(subject(BuildSubjectRole::RuntimeBin, "runtime.bin")?);
    expected_subjects.push(subject(BuildSubjectRole::RuntimeElf, "runtime.elf")?);
    if provenance.subjects != expected_subjects || provenance.sources.is_empty() {
        return Err(format!(
            "HIL run `{}` has inconsistent build subjects or no source materials",
            manifest.run_id
        )
        .into());
    }
    for subject in &provenance.subjects {
        verify_indexed_file(
            run_directory,
            &subject.path,
            subject.size_bytes,
            &subject.sha256,
            "build subject",
        )?;
    }
    let mut source_names = BTreeSet::new();
    for source in &provenance.sources {
        if source.name.is_empty() || !source_names.insert(&source.name) {
            return Err(format!(
                "HIL run `{}` has an invalid or duplicate source material",
                manifest.run_id
            )
            .into());
        }
        validate_source_material(run_directory, manifest, source)?;
    }
    let primary = &provenance.sources[0];
    let expected_repository = artifact
        .replayed_from
        .as_ref()
        .map(|origin| &origin.firmware_repository)
        .unwrap_or(&manifest.repository);
    if primary.name != "repository"
        || primary.commit != expected_repository.commit
        || primary.dirty != expected_repository.dirty
        || primary.workspace_sha256 != expected_repository.workspace_sha256
    {
        return Err(format!(
            "HIL run `{}` has primary source material inconsistent with its manifest",
            manifest.run_id
        )
        .into());
    }
    let source_reconstructable = provenance
        .sources
        .iter()
        .all(|source| source.rebuild_status != SourceRebuildStatus::Incomplete);
    if provenance.source_reconstructable != source_reconstructable {
        return Err(format!(
            "HIL run `{}` has inconsistent source reconstructability",
            manifest.run_id
        )
        .into());
    }
    let mut file_names = BTreeSet::new();
    let mut file_paths = BTreeSet::new();
    for file in &provenance.files {
        validate_relative_path(&file.path, "build file material")?;
        validate_sha256(&file.sha256, "build file material", &file.name)?;
        if file.name.is_empty() || !file_names.insert(&file.name) || !file_paths.insert(&file.path)
        {
            return Err(format!(
                "HIL run `{}` has an invalid or duplicate build file material",
                manifest.run_id
            )
            .into());
        }
        if let Some(archive_path) = &file.archive_path {
            verify_indexed_file(
                run_directory,
                archive_path,
                file.size_bytes,
                &file.sha256,
                "archived build file material",
            )?;
        }
    }
    let expected_lock_archive = PathBuf::from("firmware")
        .join(artifact.image.id())
        .join("effective-Cargo.lock");
    validate_snapshot_materials(run_directory, &provenance)?;
    if provenance
        .files
        .iter()
        .find(|file| file.name == "embedded-lock")
        .and_then(|file| {
            file.archive_path
                .as_ref()
                .filter(|path| *path == &expected_lock_archive)
        })
        .is_none()
    {
        return Err(format!(
            "HIL run `{}` does not archive its effective embedded lock file",
            manifest.run_id
        )
        .into());
    }
    let mut tools = BTreeSet::new();
    if provenance.environment.cargo_incremental != "0"
        || provenance.environment.tools.iter().any(|tool| {
            tool.name.is_empty() || tool.program.is_empty() || !tools.insert(&tool.name)
        })
    {
        return Err(format!(
            "HIL run `{}` has invalid build environment provenance",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

fn validate_replay_origin(
    manifest: &RunManifest,
    artifact: &oer_tool_data_hil_run_bundle_format::run::FirmwareArtifact,
) -> Result<()> {
    let Some(origin) = &artifact.replayed_from else {
        return Ok(());
    };
    if origin.source_run_id == manifest.run_id
        || !is_single_normal_component(Path::new(&origin.source_run_id))
        || origin.source_build_id != artifact.build_id
    {
        return Err(format!(
            "HIL run `{}` has inconsistent replay origin for `{}`",
            manifest.run_id,
            artifact.image.id()
        )
        .into());
    }
    validate_sha256(
        &origin.source_integrity_sha256,
        "source run integrity",
        &origin.source_run_id,
    )
}

fn validate_source_material(
    run_directory: &Path,
    manifest: &RunManifest,
    source: &SourceMaterial,
) -> Result<()> {
    match (
        source.tracked_patch_path.as_deref(),
        source.tracked_patch_size_bytes,
        source.tracked_patch_sha256.as_deref(),
    ) {
        (None, None, None) => {}
        (Some(path), Some(size_bytes), Some(sha256)) => verify_indexed_file(
            run_directory,
            path,
            size_bytes,
            sha256,
            "tracked source patch",
        )?,
        _ => {
            return Err(format!(
                "HIL run `{}` has incomplete tracked source patch provenance",
                manifest.run_id
            )
            .into());
        }
    }
    let mut untracked_paths = BTreeSet::new();
    validate_sha256(&source.workspace_sha256, "source workspace", &source.name)?;
    for file in &source.untracked_files {
        validate_relative_path(&file.path, "untracked source identity")?;
        validate_sha256(
            &file.sha256,
            "untracked source identity",
            &file.path.display().to_string(),
        )?;
        if !untracked_paths.insert(&file.path) {
            return Err(format!(
                "HIL run `{}` has a duplicate untracked source path `{}`",
                manifest.run_id,
                file.path.display()
            )
            .into());
        }
    }
    let state_is_consistent = match source.rebuild_status {
        SourceRebuildStatus::SourceSnapshot => {
            source.tracked_patch_path.is_none()
                && source.untracked_files.is_empty()
                && source.limitations.is_empty()
                && !source.commit.is_empty()
        }
        SourceRebuildStatus::CleanCommit => {
            !source.dirty
                && source.tracked_patch_path.is_none()
                && source.untracked_files.is_empty()
                && source.limitations.is_empty()
                && source.remote.is_some()
                && !source.commit.is_empty()
        }
        SourceRebuildStatus::TrackedPatch => {
            source.dirty
                && source.tracked_patch_path.is_some()
                && source.untracked_files.is_empty()
                && source.limitations.is_empty()
                && source.remote.is_some()
                && !source.commit.is_empty()
        }
        SourceRebuildStatus::Incomplete => !source.limitations.is_empty(),
    };
    if !state_is_consistent {
        return Err(format!(
            "HIL run `{}` has inconsistent source rebuild provenance",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

fn validate_snapshot_materials(run: &Path, provenance: &BuildProvenance) -> Result<()> {
    let snapshot_sources = provenance
        .sources
        .iter()
        .filter(|s| s.rebuild_status == SourceRebuildStatus::SourceSnapshot)
        .count();
    let materials = provenance
        .files
        .iter()
        .filter(|f| f.name.starts_with("source-snapshot-"))
        .collect::<Vec<_>>();
    if snapshot_sources == 0 && materials.is_empty() {
        return Ok(());
    }
    if snapshot_sources != provenance.sources.len()
        || materials.len() != crate::build::SNAPSHOT_MATERIALS.len()
    {
        return Err("incomplete source snapshot binding".into());
    }
    let mut directory = None;
    for (name, filename) in crate::build::SNAPSHOT_MATERIALS {
        let file = materials
            .iter()
            .find(|f| f.name == name)
            .ok_or("missing source snapshot material")?;
        let path = file
            .archive_path
            .as_deref()
            .ok_or("source snapshot must be archived")?;
        if file.path != Path::new(filename)
            || path.file_name() != Some(std::ffi::OsStr::new(filename))
        {
            return Err("invalid source snapshot material path".into());
        }
        let parent = path.parent().ok_or("source snapshot has no directory")?;
        if directory
            .replace(parent)
            .is_some_and(|previous| previous != parent)
        {
            return Err("source snapshot materials must share one directory".into());
        }
    }
    let frozen = oer_tool_service_hil_source_snapshot::FrozenSources::open(
        &run.join(directory.unwrap()),
        &oer_tool_data_hil_schema::snapshot::objects_of_run(run)?,
    )?;
    if frozen.sources().len() != provenance.sources.len() {
        return Err("source snapshot roles disagree with provenance".into());
    }
    for (input, source) in frozen.sources().iter().zip(&provenance.sources) {
        if input.name != source.name
            || input.commit != source.commit
            || input.dirty != source.dirty
            || oer_tool_service_hil_source_snapshot::identity(input)? != source.workspace_sha256
        {
            return Err("source snapshot identity disagrees with provenance".into());
        }
    }
    Ok(())
}

fn validate_attachments(run_directory: &Path, suite: &SuiteResult) -> Result<usize> {
    let mut paths = BTreeSet::new();
    let mut count = 0;
    for scenario in &suite.scenarios {
        for repetition in &scenario.repetitions {
            validate_relative_path(&repetition.artifact_directory, "artifact directory")?;
            for attachment in &repetition.attachments {
                if attachment.media_type.is_empty()
                    || !attachment.path.starts_with(&repetition.artifact_directory)
                    || !paths.insert(&attachment.path)
                {
                    return Err(format!(
                        "HIL run `{}` has an invalid or duplicate attachment `{}`",
                        suite.run_id,
                        attachment.path.display()
                    )
                    .into());
                }
                verify_indexed_file(
                    run_directory,
                    &attachment.path,
                    attachment.size_bytes,
                    &attachment.sha256,
                    "attachment",
                )?;
                count += 1;
            }
        }
    }
    Ok(count)
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
        &oer_tool_data_hil_run_bundle_format::archived::stored_path(relative_path),
    )?;
    let oer_tool_data_hil_run_bundle_format::archived::Identity {
        size_bytes: actual_size,
        sha256: actual_sha256,
    } = oer_tool_data_hil_run_bundle_format::archived::archived_identity(&path)?;
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

fn require_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(format!("HIL bundle path is not a directory: {}", path.display()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
