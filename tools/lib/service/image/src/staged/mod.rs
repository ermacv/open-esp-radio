//! The staged pipeline: the ESP-IDF bootloader loads the platform bootstrap,
//! which embeds the packed stage-two runtime and stages it into PSRAM.
//!
//! The runtime is compiled and checked as the caller requests; it is
//! flattened and packed with its checksum, embedded in the bootstrap, whose
//! own stack is checked with the runtime's; the bootstrap ELF is then encoded
//! as the application of the chip's partition table with the flash map's
//! application settings, and the bootloader in the mode the ROM reads it in,
//! the partition tables and the OTA selection of the first slot are encoded
//! beside it.

use std::path::Path;

use oer_image_bundle::{ImageBundle, Lock, Staged, files};
use oer_image_encode as encode;
use oer_image_encode::Encoding;
use oer_image_policy::StackPolicy;

use crate::{
    ImageSpec, Result,
    build_log::BuildLog,
    exclusion::{BuildLock, Staging},
    source_inputs,
};

use oer_image_encode::stage_two::pack_runtime;

/// The staged images' MMU page: the bootstrap keeps its text on 64-KiB
/// pages and stage two starts on one.
const MMU_PAGE_SIZE: u32 = 0x1_0000;

pub(crate) fn build(spec: &ImageSpec, profile: &oer_chip_profile::Profile) -> Result<ImageBundle> {
    let root = spec.root.as_path();
    let target = profile.rust_target.as_str();
    let bootstrap_package = profile.bootstrap_package();
    let platform = profile.platform_workspace(root);
    let flash = profile.flash.clone().ok_or("no flash map")?;
    // Every file goes into the staging directory; the bundle is published
    // only once the whole build succeeded.
    let staging = Staging::begin(&spec.output)?;
    let output = staging.directory().to_owned();
    let mut bundle = ImageBundle::new(&output, profile, flash);
    bundle.layout_seed = spec.layout_seed;
    let log = BuildLog::create(&output.join(files::BUILD_LOG))?;
    let cache = crate::compile_cache()?;
    let policy = StackPolicy::load(&root.join(&spec.stack_policy))?;
    // Private copies of both committed catalogs: patched networks and local
    // overrides resolve into them, never into the source tree.
    let workspace = root.join(&spec.application.workspace);
    let runtime_lock = BuildLock::prepare(&workspace, &output.join("locks/runtime"))?;
    let bootstrap_lock = BuildLock::prepare(&platform, &output.join("locks/bootstrap"))?;

    // Every image uplifts into the same `<target>/release` of the shared
    // cache: each build holds it until its ELF and dependency information
    // are copied out.
    let release = cache.join(target).join("release");
    let (runtime_elf, mut compiled) = {
        let _cache = oer_toolchain::image::lock_compile_cache(&cache)?;
        let mut runtime = crate::runtime_command(spec, profile, "build", &policy)?;
        runtime.env("CARGO_TARGET_DIR", &cache);
        runtime_lock.configure(&mut runtime);
        if spec.overrides.is_empty() {
            crate::cargo::ensure_fetched(root, &workspace.join("Cargo.toml"), |command| {
                runtime_lock.configure(command)
            })?;
        }
        {
            let _slot = crate::exclusion::slot(&crate::host_build_root()?.join("tokens"))?;
            log.run(&mut runtime, "build the stage-two runtime")?;
        }
        let elf = crate::snapshot(
            &release.join(&spec.application.binary),
            &output,
            files::RUNTIME_ELF,
        )?;
        let compiled =
            source_inputs::collect(root, &[(&release, spec.application.binary.as_str())])?;
        (elf, compiled)
    };
    // Local overrides deliberately resolve path packages.
    if spec.overrides.is_empty() {
        runtime_lock.validate()?;
    }

    let runtime_bin = output.join(files::RUNTIME_BIN);
    let mut objcopy = oer_toolchain::command(oer_toolchain::Tool::LlvmObjcopy)?;
    objcopy
        .args(["-O", "binary"])
        .arg(&runtime_elf)
        .arg(&runtime_bin);
    log.run(&mut objcopy, "flatten the stage-two runtime")?;
    let staged = profile
        .staged
        .as_ref()
        .ok_or("a staged chip's profile names its [staged] contract")?;
    let crc = pack_runtime(&staged.stage_two, &runtime_bin)
        .map_err(|error| log.failed("runtime packing", error))?;
    eprintln!("runtime_crc32={crc:08x}");
    if let Some(checks) = &spec.checks {
        let outcome = checks
            .runtime(&crate::CheckInput {
                root,
                profile,
                policy: &policy,
                elf: &runtime_elf,
                flat: Some(&runtime_bin),
                output: &output,
            })
            .map_err(|error| log.failed("runtime checks", error))?;
        crate::apply_outcome(
            checks.as_ref(),
            crate::CheckedElf::Runtime,
            outcome,
            &mut bundle,
        )?;
    }

    // The bootstrap embeds the runtime with `include_bytes!`; a stable path
    // that keeps its timestamp while its bytes are unchanged leaves the
    // bootstrap fresh.
    let bootstrap_cache = oer_toolchain::image::lock_compile_cache(&cache)?;
    let embedded = std::path::absolute(cache.join("stage-two-runtime.bin"))?;
    crate::replace_if_changed(&runtime_bin, &embedded)?;
    let mut bootstrap = crate::cargo::command();
    bootstrap
        .current_dir(root)
        .args(["build", "--manifest-path"])
        .arg(platform.join("Cargo.toml"))
        .args(["-p", &bootstrap_package, "--release", "--target", target])
        .env("CARGO_TARGET_DIR", &cache)
        .env("CARGO_INCREMENTAL", "0")
        .env("PSRAM_RUNTIME_BIN", &embedded);
    if spec.overrides.esp_hal.is_none() {
        bootstrap.arg("--locked");
    }
    bootstrap_lock.configure(&mut bootstrap);
    spec.overrides.apply(&mut bootstrap);
    crate::zeroed_inputs(&mut bootstrap, profile);
    oer_toolchain::image::configure(
        &mut bootstrap,
        &policy.image_compiler(root, &crate::linker_cache()?, target),
    )?;
    if spec.overrides.esp_hal.is_none() {
        crate::cargo::ensure_fetched(root, &platform.join("Cargo.toml"), |command| {
            bootstrap_lock.configure(command)
        })?;
    }
    log.run(&mut bootstrap, "build the Flash/SRAM bootstrap")?;
    let bootstrap_elf = crate::snapshot(
        &release.join(&bootstrap_package),
        &output,
        files::BOOTSTRAP_ELF,
    )?;
    compiled.extend(source_inputs::collect(
        root,
        &[(&release, bootstrap_package.as_str())],
    )?);
    drop(bootstrap_cache);
    if let Some(checks) = &spec.checks {
        let outcome = checks
            .bootstrap(&crate::CheckInput {
                root,
                profile,
                policy: &policy,
                elf: &bootstrap_elf,
                flat: None,
                output: &output,
            })
            .map_err(|error| log.failed("bootstrap checks", error))?;
        crate::apply_outcome(
            checks.as_ref(),
            crate::CheckedElf::Bootstrap,
            outcome,
            &mut bundle,
        )?;
    }

    let elf = std::fs::read(&bootstrap_elf)?;
    let application = encode_application(root, profile, &elf, &bundle)
        .map_err(|error| log.failed("encode the ESP application image", error))?;
    std::fs::write(bundle.application(), application)?;
    encode_boot_files(root, profile, &elf, &mut bundle)
        .map_err(|error| log.failed("encode the boot files", error))?;
    bundle.staged = Some(Staged {
        bootstrap_package: bootstrap_package.clone(),
    });

    for (lock, committed, file) in [
        (
            &runtime_lock,
            &spec.application.workspace,
            files::RUNTIME_LOCK,
        ),
        (
            &bootstrap_lock,
            &platform.strip_prefix(root)?.to_owned(),
            files::BOOTSTRAP_LOCK,
        ),
    ] {
        std::fs::copy(lock.path(), output.join(file))?;
        bundle.locks.push(Lock {
            committed: committed.join("Cargo.lock"),
            file: file.to_owned(),
        });
    }

    let map = bundle.flash.clone();
    let mut reads = vec![
        spec.stack_policy.clone(),
        Path::new("platform").join(&profile.id).join("chip.toml"),
        Path::new("platform").join(&profile.id).join("stack.toml"),
    ];
    if let Some(coverage) = &policy.coverage_policy {
        reads.push(lexical(coverage.strip_prefix(root)?));
    }
    reads.push(
        Path::new("verification")
            .join(&profile.id)
            .join("artifacts.toml"),
    );
    reads.extend(profile.rom.iter().filter_map(|rom| rom.summaries.clone()));
    reads.push(map.partitions.clone());
    reads.extend(spec.reads.iter().cloned());
    let platform_relative = platform.strip_prefix(root)?.to_owned();
    let mut inputs = source_inputs::configuration(
        root,
        &compiled,
        &[spec.application.workspace.as_path(), &platform_relative],
        &reads
            .iter()
            .map(std::path::PathBuf::as_path)
            .collect::<Vec<_>>(),
    )?;
    inputs.extend(compiled);
    inputs.extend(spec.builder_inputs.iter().cloned());
    source_inputs::write(&output, &inputs)?;
    // The private lock copies end with the build.
    drop((runtime_lock, bootstrap_lock));
    staging.publish(bundle)
}

/// The QIO application image of the bootstrap ELF `elf` for the partition
/// at the flash map's application offset, audited against that partition.
fn encode_application(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: &[u8],
    bundle: &ImageBundle,
) -> Result<Vec<u8>> {
    let table = partitions(root, &bundle.flash)?;
    let partition = table
        .iter()
        .find(|partition| partition.application && partition.offset == bundle.flash.application)
        .ok_or_else(|| {
            format!(
                "the partition table has no application partition at {:#x}",
                bundle.flash.application
            )
        })?;
    let encoded = encode::encode(
        elf,
        encode::chip(&profile.espflash_chip)?,
        Encoding::application(&bundle.flash, MMU_PAGE_SIZE)?,
        Some(&root.join(&bundle.flash.partitions)),
        bundle.flash.partition_table,
        Some(&partition.name),
    )?;
    audit_application_image(&encoded.application, partition.size)?;
    Ok(encoded.application)
}

/// Encode the staged boot's bootloader (in the mode the ROM reads it in,
/// from any ELF of the application), the partition tables and the OTA
/// selection of the first slot into `bundle`.
pub(crate) fn encode_boot_files(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: &[u8],
    bundle: &mut ImageBundle,
) -> Result<()> {
    let flash = bundle.flash.clone();
    let table = partitions(root, &flash)?;
    let otadata = table
        .iter()
        .find(|partition| partition.otadata)
        .ok_or("the partition table has no OTA data partition")?;
    if flash.otadata != otadata.offset {
        return Err(format!(
            "the flash map puts the OTA data at {:#x}, the partition table at {:#x}",
            flash.otadata, otadata.offset
        )
        .into());
    }
    let csv = root.join(&flash.partitions);
    let rom = encode::encode(
        elf,
        encode::chip(&profile.espflash_chip)?,
        Encoding::bootloader(&flash, MMU_PAGE_SIZE)?,
        Some(&csv),
        flash.partition_table,
        None,
    )?;
    std::fs::write(
        bundle.bootloader(),
        encode::rom_bootloader(&rom.bootloader)?,
    )?;
    std::fs::write(bundle.partitions(), &rom.partition_table)?;
    std::fs::write(bundle.path(files::OTADATA), encode::ota_selector_image(0))?;
    bundle.otadata = true;
    Ok(())
}

/// `path` with its `..` components resolved against the components before
/// them, as a repository-relative input path names the file.
fn lexical(path: &Path) -> std::path::PathBuf {
    let mut resolved = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

/// The partitions of the flash map's partition table at `root`.
pub fn partitions(root: &Path, flash: &crate::FlashMap) -> Result<Vec<encode::Partition>> {
    encode::partitions(&root.join(&flash.partitions))
}

/// Audit an encoded application image: its ESP-IDF app descriptor and the
/// 64-KiB MMU page size, warning when it nearly fills `capacity`, the bytes
/// of its partition.
pub fn audit_application_image(bytes: &[u8], capacity: u32) -> Result<()> {
    const APP_DESC_OFFSET: usize = 0x20;
    const APP_DESC_MMU_PAGE_LOG2_OFFSET: usize = 180;
    let end = APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET + 1;
    if bytes.len() < end
        || bytes[APP_DESC_OFFSET..APP_DESC_OFFSET + 4] != 0xabcd_5432_u32.to_le_bytes()
        || bytes[APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET] != 16
    {
        return Err("ESP application image has an invalid app descriptor or MMU page size".into());
    }
    if bytes.len() > capacity as usize {
        return Err(format!(
            "the application image ({} bytes) exceeds its partition ({capacity} bytes)",
            bytes.len()
        )
        .into());
    }
    if let Some(warning) = encode::partition_budget_warning(bytes.len() as u64, u64::from(capacity))
    {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
