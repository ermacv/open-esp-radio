//! Firmware and replay-material archival for a run session.

use oer_os_durable::atomic_json;
use std::path::{Path, PathBuf};

use crate::Result;
use crate::build;
use crate::run::RunSession;
use oer_hil_run_bundle_format::run::FirmwareArtifact;
use oer_hil_run_bundle_format::run::FirmwareReplayOrigin;
use oer_hil_schema::image::ImageClass;

/// Where a run's firmware is archived: the bundle and the object store.
pub struct FirmwareArchive<'a> {
    pub directory: &'a Path,
    pub target_directory: &'a Path,
}

impl RunSession {
    /// Where the image builder archives the `image` firmware of this run:
    /// the bundle and the object store. Refuses an image the run already
    /// has.
    pub fn firmware_archive(&self, image: ImageClass) -> Result<FirmwareArchive<'_>> {
        if self
            .manifest
            .firmware
            .iter()
            .any(|artifact| artifact.image == image)
        {
            return Err("firmware already bound to this run cannot be replaced".into());
        }
        Ok(FirmwareArchive {
            directory: &self.directory,
            target_directory: &self.target_directory,
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
        archived: &crate::replay::ArchivedFirmware,
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
        // A replay of a replay names the run that built the image.
        artifact.replayed_from =
            Some(
                source
                    .replayed_from
                    .clone()
                    .unwrap_or_else(|| FirmwareReplayOrigin {
                        source_run_id: archived.run_id.clone(),
                    }),
            );
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
    subject: oer_hil_run_bundle_format::run::SubjectRecord<'_>,
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
