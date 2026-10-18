//! Firmware and replay-material archival for a run session.

use oer_tool_base_durable::atomic_json;
use std::path::{Path, PathBuf};

use crate::Result;
use crate::build;
use crate::run::RunSession;
use oer_tool_data_hil_run_bundle_format::run::FirmwareArtifact;
use oer_tool_data_hil_run_bundle_format::run::FirmwareReplayOrigin;
use oer_tool_data_hil_schema::image::ImageClass;

/// Where a run's firmware is archived: the bundle, the object store and the
/// sources and materials its build provenance cites.
pub struct FirmwareArchive<'a> {
    pub directory: &'a Path,
    pub target_directory: &'a Path,
    pub source_root: PathBuf,
    pub source_materials: &'a [oer_tool_data_hil_run_bundle_format::build::SourceMaterial],
    pub snapshot_materials: &'a [oer_tool_data_hil_run_bundle_format::build::BuildFileMaterial],
}

impl RunSession {
    /// Where the image builder archives the `image` firmware of this run:
    /// the bundle, the object store and the run's frozen sources, which must
    /// be unchanged. Refuses an image the run already has.
    pub fn firmware_archive(&self, image: ImageClass) -> Result<FirmwareArchive<'_>> {
        if self
            .manifest
            .firmware
            .iter()
            .any(|artifact| artifact.image == image)
        {
            return Err("firmware already bound to this run cannot be replaced".into());
        }
        let frozen = self
            .frozen_sources
            .as_ref()
            .ok_or("firmware requires a bound source snapshot")?;
        frozen.verify_unchanged()?;
        Ok(FirmwareArchive {
            directory: &self.directory,
            target_directory: &self.target_directory,
            source_root: frozen.repository(),
            source_materials: &self.source_materials,
            snapshot_materials: &self.snapshot_materials,
        })
    }

    /// Bind the firmware record the image builder archived.
    pub fn bind_firmware(&mut self, artifact: FirmwareArtifact) -> Result<()> {
        if self
            .manifest
            .firmware
            .iter()
            .any(|entry| entry.image == artifact.image)
        {
            return Err("firmware already bound to this run cannot be replaced".into());
        }
        self.manifest.firmware.push(artifact);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)
    }

    pub fn record_replayed_firmware(
        &mut self,
        archived: &crate::verify::ArchivedFirmware,
    ) -> Result<PathBuf> {
        if self
            .manifest
            .firmware
            .iter()
            .any(|artifact| artifact.image == archived.artifact.image)
        {
            return Err("firmware already bound to this run cannot be replaced".into());
        }
        let source = &archived.artifact;
        let firmware_directory = PathBuf::from("firmware").join(source.image.id());
        let application_path = firmware_directory.join("application.bin");
        validate_replayed_source_path(&source.application_path)?;
        archive_expected_file(
            &archived.source_directory.join(&source.application_path),
            &self.directory.join(&application_path),
            &self.target_directory,
            source.application_size_bytes,
            &source.application_sha256,
        )?;

        for subject in source.subjects() {
            import_optional_firmware_subject(
                &archived.source_directory,
                &self.directory,
                &self.target_directory,
                subject,
            )?;
        }

        let mut artifact = source.clone();
        artifact.replayed_from = Some(FirmwareReplayOrigin {
            source_run_id: archived.run_id.clone(),
            source_integrity_sha256: archived.integrity_sha256.clone(),
            firmware_repository: source
                .replayed_from
                .as_ref()
                .map(|origin| origin.firmware_repository.clone())
                .unwrap_or_else(|| archived.repository.clone()),
            source_build_id: source.build_id.clone(),
        });
        if let Some(mut provenance) = archived.build_provenance.clone() {
            for (index, source_material) in provenance.sources.iter_mut().enumerate() {
                let Some(source_path) = source_material.tracked_patch_path.clone() else {
                    continue;
                };
                validate_replayed_source_path(&source_path)?;
                let destination = firmware_directory
                    .join("source-patches")
                    .join(format!("{index:02}.patch"));
                archive_expected_file(
                    &archived.source_directory.join(source_path),
                    &self.directory.join(&destination),
                    &self.target_directory,
                    source_material
                        .tracked_patch_size_bytes
                        .ok_or("replayed source patch has no size")?,
                    source_material
                        .tracked_patch_sha256
                        .as_deref()
                        .ok_or("replayed source patch has no digest")?,
                )?;
                source_material.tracked_patch_path = Some(destination);
            }
            for (index, file) in provenance.files.iter_mut().enumerate() {
                let Some(source_path) = file.archive_path.clone() else {
                    continue;
                };
                validate_replayed_source_path(&source_path)?;
                let destination = if file.name == "embedded-lock" {
                    firmware_directory.join("effective-Cargo.lock")
                } else if file.name.starts_with("source-snapshot-") {
                    firmware_directory.join("source-snapshot").join(&file.path)
                } else {
                    firmware_directory
                        .join("build-materials")
                        .join(format!("{index:02}"))
                };
                archive_expected_file(
                    &archived.source_directory.join(source_path),
                    &self.directory.join(&destination),
                    &self.target_directory,
                    file.size_bytes,
                    &file.sha256,
                )?;
                file.archive_path = Some(destination);
            }
            let provenance_path = firmware_directory.join("build-provenance.json");
            atomic_json(&self.directory.join(&provenance_path), &provenance)?;
            artifact.build_provenance_path = Some(provenance_path);
        }
        self.manifest
            .firmware
            .retain(|entry| entry.image != artifact.image);
        self.manifest.firmware.push(artifact);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        Ok(self.directory.join(application_path))
    }
}

fn archive_expected_file(
    source: &Path,
    destination: &Path,
    target_directory: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<()> {
    let archived = build::archive_content_addressed(
        build::Source::Archived(source),
        destination,
        target_directory,
    )?;
    if archived.size_bytes != expected_size || archived.sha256 != expected_sha256 {
        return Err(format!(
            "replayed firmware input changed after bundle verification: {}",
            source.display()
        )
        .into());
    }
    Ok(())
}

fn import_optional_firmware_subject(
    source_directory: &Path,
    destination_directory: &Path,
    target_directory: &Path,
    subject: oer_tool_data_hil_run_bundle_format::run::SubjectRecord<'_>,
) -> Result<()> {
    match (subject.path, subject.size_bytes, subject.sha256) {
        (None, None, None) => Ok(()),
        (Some(path), Some(size_bytes), Some(sha256)) => {
            validate_replayed_source_path(path)?;
            archive_expected_file(
                &source_directory.join(path),
                &destination_directory.join(path),
                target_directory,
                size_bytes,
                sha256,
            )
        }
        _ => Err("replayed firmware subject has incomplete archive provenance".into()),
    }
}

pub(super) fn validate_replayed_source_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "replayed firmware references an unsafe bundle path: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}
