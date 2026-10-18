//! Shared firmware publication for observations and build-only records.
use super::Recipe;
use crate::{Artifacts, LayoutSeed, Result};
use oer_durable::atomic_json;
use oer_hil_run_bundle::build;
use oer_hil_run_bundle::run::FirmwareArchive;
use oer_hil_run_bundle::run::RunSession;
use oer_hil_run_bundle::verify::FirmwareRecipe as _;
use oer_hil_run_bundle_format::build::BuildSubject;
use oer_hil_run_bundle_format::build::BuildSubjectRole;
use oer_hil_run_bundle_format::run::FirmwareArtifact;
use oer_hil_schema::image::ImageClass;
use std::path::{Path, PathBuf};

/// Archive `artifacts` as the `image` firmware of `session` and bind it to
/// the run. Returns the bundle to flash: the build's flash files with the
/// archived application, below the run's target directory, so the run
/// writes exactly the bytes it archived.
pub fn record(
    session: &mut RunSession,
    image: ImageClass,
    artifacts: &Artifacts,
) -> Result<oer_image_bundle::ImageBundle> {
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
    let selection = (
        image,
        bundle.layout_seed,
        &artifacts.features,
        (bundle.chip.as_str(), bundle.rust_target.as_str()),
    );
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
    let embedded_lock = std::path::Path::new("hil/targets")
        .join(&bundle.chip)
        .join("Cargo.lock");
    let effective_embedded_lock = bundle
        .lock(&embedded_lock)
        .ok_or("the bundle has no effective lock of its agent workspace")?;
    let subject = |role, (path, archived): &(PathBuf, build::ArchivedFile)| BuildSubject {
        role,
        path: path.clone(),
        size_bytes: archived.size_bytes,
        sha256: archived.sha256.clone(),
    };
    // The workspace whose lock the runtime resolved against, and the boot
    // flow's own files: build record subjects go application, boot files,
    // runtime ELF.
    let mut locks = Vec::new();
    let mut lock = |name: &str, original: String, filename: &str, source: &Path| -> Result<()> {
        let (path, archived) = archive(source, filename)?;
        locks.push(build::archived_file_material(
            name,
            Path::new(&original),
            path,
            &archived,
        ));
        Ok(())
    };
    let mut subjects = vec![subject(
        BuildSubjectRole::Application,
        &(application_path.clone(), application.clone()),
    )];
    let mut artifact = FirmwareArtifact {
        image,
        replayed_from: None,
        build_id: None,
        build_provenance_path: None,
        application_path,
        application_size_bytes: application.size_bytes,
        application_sha256: application.sha256.clone(),
        runtime_elf_path: Some(runtime_elf_path.clone()),
        runtime_elf_size_bytes: Some(runtime_elf.size_bytes),
        runtime_elf_sha256: runtime_elf.sha256.clone(),
        runtime_bin_path: None,
        runtime_bin_size_bytes: None,
        runtime_bin_sha256: None,
        bootstrap_elf_path: None,
        bootstrap_elf_size_bytes: None,
        bootstrap_elf_sha256: None,
        layout_seed: bundle.layout_seed,
    };
    let runtime_bin = archive(
        &bundle
            .runtime_bin()
            .ok_or("a staged bundle has its runtime")?,
        "runtime.bin",
    )?;
    let bootstrap_elf = archive(
        &bundle
            .bootstrap_elf()
            .ok_or("a staged bundle has its bootstrap")?,
        "bootstrap.elf",
    )?;
    lock(
        "embedded-lock",
        embedded_lock.display().to_string(),
        "effective-Cargo.lock",
        &effective_embedded_lock,
    )?;
    let bootstrap_lock = std::path::Path::new("platform")
        .join(&bundle.chip)
        .join("Cargo.lock");
    lock(
        "bootstrap-lock",
        bootstrap_lock.display().to_string(),
        "bootstrap-Cargo.lock",
        &bundle
            .lock(&bootstrap_lock)
            .ok_or("the bundle has no effective lock of its bootstrap")?,
    )?;
    subjects.push(subject(BuildSubjectRole::BootstrapElf, &bootstrap_elf));
    subjects.push(subject(BuildSubjectRole::RuntimeBin, &runtime_bin));
    artifact.runtime_bin_path = Some(runtime_bin.0);
    artifact.runtime_bin_size_bytes = Some(runtime_bin.1.size_bytes);
    artifact.runtime_bin_sha256 = Some(runtime_bin.1.sha256);
    artifact.bootstrap_elf_path = Some(bootstrap_elf.0);
    artifact.bootstrap_elf_size_bytes = Some(bootstrap_elf.1.size_bytes);
    artifact.bootstrap_elf_sha256 = Some(bootstrap_elf.1.sha256);

    subjects.push(subject(
        BuildSubjectRole::RuntimeElf,
        &(runtime_elf_path, runtime_elf),
    ));
    archive(&bundle.source_inputs(), "source-inputs.json")?;
    let build_id = build::build_id(&subjects);
    let build_provenance_path = firmware_directory.join("build-provenance.json");
    locks.extend(context.snapshot_materials.to_vec());
    let provenance = create_provenance(
        &context.source_root,
        selection,
        build_id.clone(),
        context.source_materials.to_vec(),
        subjects,
        locks,
        artifacts.environment.clone(),
    )?;
    atomic_json(&context.directory.join(&build_provenance_path), &provenance)?;
    artifact.build_id = Some(build_id);
    artifact.build_provenance_path = Some(build_provenance_path);
    Ok((artifact, archived_application))
}

pub(super) fn create_provenance(
    root: &Path,
    selection: (
        ImageClass,
        LayoutSeed,
        &oer_hil_schema::image::FeatureDelta,
        // The chip, its Rust target and how it boots.
        (&str, &str),
    ),
    build_id: String,
    sources: Vec<oer_hil_run_bundle_format::build::SourceMaterial>,
    subjects: Vec<oer_hil_run_bundle_format::build::BuildSubject>,
    effective_locks: Vec<oer_hil_run_bundle_format::build::BuildFileMaterial>,
    environment: oer_hil_run_bundle_format::build::BuildEnvironment,
) -> Result<oer_hil_run_bundle_format::build::BuildProvenance> {
    let (image, layout_seed, features, (chip, rust_target)) = selection;
    let network = oer_hil_image_class::network_on(image, chip);
    // The files the image's build reads beside its sources, from the chip
    // profile: the agent's workspace and stack policy (and the policy it
    // extends), the profile itself and the partition table it names.
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let policy = profile.hil_stack_policy();
    let mut files = vec![
        ("workspace-lock", String::from("Cargo.lock")),
        (
            "embedded-workspace",
            format!("hil/targets/{chip}/Cargo.toml"),
        ),
        ("stack-policy", policy.to_string_lossy().into_owned()),
        ("chip-profile", format!("platform/{chip}/chip.toml")),
    ];
    let policy_text: toml::Table = toml::from_str(&std::fs::read_to_string(root.join(&policy))?)?;
    if let Some(base) = policy_text.get("extends").and_then(toml::Value::as_str) {
        // `extends` is relative to the policy: resolve it lexically.
        let mut resolved = std::path::PathBuf::new();
        for part in policy
            .parent()
            .unwrap_or(Path::new(""))
            .join(base)
            .components()
        {
            match part {
                std::path::Component::ParentDir => {
                    resolved.pop();
                }
                std::path::Component::CurDir => {}
                other => resolved.push(other),
            }
        }
        let base = resolved;
        files.push(("stack-policy-base", base.to_string_lossy().into_owned()));
    }
    if let Some(partitions) = profile.flash.as_ref().map(|flash| &flash.partitions) {
        files.push(("partition-table", partitions.to_string_lossy().into_owned()));
    }
    let mut files = files
        .into_iter()
        .map(|(name, path)| build::build_file_material(root, name, Path::new(&path)))
        .collect::<Result<Vec<_>>>()?;
    files.extend(effective_locks);
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(oer_hil_run_bundle_format::build::BuildProvenance {
        schema: oer_hil_run_bundle_format::build::BUILD_PROVENANCE_SCHEMA,
        build_id,
        build_type: String::from("open-esp-radio-hil-firmware/v1"),
        parameters: oer_hil_run_bundle_format::build::BuildParameters {
            image,
            network: network.map(str::to_owned),
            runtime_profile: image.runtime_profile().to_owned(),
            target: rust_target.to_owned(),
            runtime_features: features.apply(&Recipe.runtime_features(image, network)?),
            layout_seed,
            features: features.clone(),
        },
        source_reconstructable: sources.iter().all(|source| {
            source.rebuild_status
                != oer_hil_run_bundle_format::build::SourceRebuildStatus::Incomplete
        }),
        sources,
        files,
        environment,
        subjects,
        reproducibility: oer_hil_run_bundle_format::build::BuildReproducibility::Unverified,
    })
}
