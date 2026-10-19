//! Shared firmware publication for observations and build-only records:
//! the image's files by their SHA-256, and the commit and dirty state of the
//! tree it was built from.
use crate::image::{Artifacts, Result};
use oer_hil_model::schema::image::ImageClass;
use oer_hil_runs::build;
use oer_hil_runs::format::run::FirmwareArtifact;
use oer_hil_runs::run::FirmwareArchive;
use oer_hil_runs::run::RunSession;
use std::path::{Path, PathBuf};

/// Archive `artifacts` as the `image` firmware of `session` and bind it to
/// the run. Returns the bundle to flash: the build's flash files with the
/// archived application, below the run's target directory, so the run
/// writes exactly the bytes it archived.
pub fn record(
    session: &mut RunSession,
    image: ImageClass,
    artifacts: &Artifacts,
) -> Result<oer_image_build::bundle::ImageBundle> {
    let context = session.firmware_archive(image)?;
    let flash = context
        .target_directory
        .join("flash")
        .join(session.id())
        .join(image.id());
    let (artifact, application) = archive(&context, image, artifacts)?;
    let bundle = artifacts.bundle.with_application(&application, &flash)?;
    session.bind_firmware(artifact)?;
    Ok(bundle)
}

pub(super) fn archive(
    context: &FirmwareArchive<'_>,
    image: ImageClass,
    artifacts: &Artifacts,
) -> Result<(FirmwareArtifact, PathBuf)> {
    let bundle = &artifacts.bundle;
    let firmware_directory = PathBuf::from("firmware").join(image.id());
    let archive = |source: &Path, file: &str| -> Result<(PathBuf, build::ArchivedFile)> {
        let path = firmware_directory.join(file);
        let archived = build::archive_content_addressed(
            build::Source::Built(source),
            &context.directory.join(&path),
            context.target_directory,
        )?;
        Ok((path, archived))
    };
    let (application_path, application) = archive(&bundle.application(), "application.bin")?;
    let archived_application = context.directory.join(&application_path);
    let (runtime_elf_path, runtime_elf) = archive(&bundle.runtime_elf(), "runtime.elf")?;
    let (runtime_bin_path, runtime_bin) = archive(
        &bundle
            .runtime_bin()
            .ok_or("a staged bundle has its runtime")?,
        "runtime.bin",
    )?;
    let (bootstrap_elf_path, bootstrap_elf) = archive(
        &bundle
            .bootstrap_elf()
            .ok_or("a staged bundle has its bootstrap")?,
        "bootstrap.elf",
    )?;
    let artifact = FirmwareArtifact {
        image,
        replayed_from: None,
        repository: artifacts.repository.clone(),
        application_path,
        application_size_bytes: application.size_bytes,
        application_sha256: application.sha256,
        runtime_elf_path: Some(runtime_elf_path),
        runtime_elf_size_bytes: Some(runtime_elf.size_bytes),
        runtime_elf_sha256: runtime_elf.sha256,
        runtime_bin_path: Some(runtime_bin_path),
        runtime_bin_size_bytes: Some(runtime_bin.size_bytes),
        runtime_bin_sha256: Some(runtime_bin.sha256),
        bootstrap_elf_path: Some(bootstrap_elf_path),
        bootstrap_elf_size_bytes: Some(bootstrap_elf.size_bytes),
        bootstrap_elf_sha256: Some(bootstrap_elf.sha256),
        layout_seed: bundle.layout_seed,
    };
    Ok((artifact, archived_application))
}
