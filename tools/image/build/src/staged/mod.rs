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

use crate::bundle::{ImageBundle, Staged, files};
use crate::encode;
use crate::encode::Encoding;
use oer_chip_profile::policy::StackPolicy;

use crate::{
    ImageSpec, Result, build_log::BuildLog, exclusion::Staging, instrumentation::Instrumentation,
    strict,
};

use crate::encode::stage_two::pack_runtime;

/// The staged images' MMU page: the bootstrap keeps its text on 64-KiB
/// pages and stage two starts on one.
const MMU_PAGE_SIZE: u32 = 0x1_0000;

pub(crate) fn build(
    spec: &ImageSpec,
    profile: &oer_chip_profile::Profile,
    instrumentation: &Instrumentation,
) -> Result<ImageBundle> {
    let root = spec.root.as_path();
    let target = profile.rust_target.as_str();
    let bootstrap_package = profile.bootstrap_package();
    let flash = profile.flash.clone().ok_or("no flash map")?;
    // Every file goes into the staging directory; the bundle is published
    // only once the whole build succeeded.
    let staging = Staging::begin(&spec.output)?;
    let output = staging.directory().to_owned();
    let mut bundle = ImageBundle::new(&output, profile, flash);
    bundle.purpose = spec.purpose;
    bundle.layout_seed = spec.layout_seed;
    let log = BuildLog::create(&output.join(files::BUILD_LOG))?;
    // The runtime and the bootstrap compile with the image's instrumentation,
    // in its own cache.
    let cache = crate::compile_cache(root, instrumentation);
    let policy = StackPolicy::load(&root.join(&spec.stack_policy))?;
    let mut values = crate::large_values::Report::new(policy.max_move_bytes);
    let mut sizes = crate::sizes::Summary::new(spec, profile)?;
    crate::test_support::refuse(root, &spec.application.package, target)?;

    // Every image uplifts into the same `<target>/firmware` of the
    // checkout's cache: each build holds it until its ELF and dependency information
    // are copied out.
    let release = cache.join(target).join(crate::FIRMWARE_PROFILE);
    let runtime_elf = {
        let _cache = crate::compiler::lock_compile_cache(&cache)?;
        let mut runtime = crate::runtime_command(spec, profile, "build", &policy, instrumentation)?;
        runtime.env("CARGO_TARGET_DIR", &cache);
        sizes.flags("runtime", &runtime, root)?;
        let strict = profile.memory.is_some() && profile.interrupts.is_some();
        let analysis = oer_image_checks::linker::strict::Spec {
            root: spec.root.clone(),
            chip: spec.chip.clone(),
            stack_policy: spec.stack_policy.clone(),
        };
        let specification = strict::configure(&mut runtime, &analysis, &output, strict)?;
        crate::cargo::ensure_fetched(root)?;
        let mut build = |invalidate| -> Result<()> {
            if invalidate {
                let mut clean = crate::cargo::command();
                clean
                    .current_dir(root)
                    .args([
                        "clean",
                        "--locked",
                        "--profile",
                        crate::FIRMWARE_PROFILE,
                        "--target",
                        target,
                        "-p",
                        &spec.application.package,
                    ])
                    .env("CARGO_TARGET_DIR", &cache);
                log.run(
                    &mut clean,
                    "invalidate the runtime link after a fingerprint mismatch",
                )?;
            }
            let _slot = crate::exclusion::slot(&crate::host_build_root()?.join("tokens"))?;
            values = crate::large_values::build(
                &mut runtime,
                &log,
                root,
                &cache,
                target,
                crate::FIRMWARE_PROFILE,
                "runtime",
                &output,
            )?;
            Ok(())
        };
        if strict {
            strict::build_with_retry(&mut build, || {
                strict::restore_if_matching(
                    &output,
                    &spec.output,
                    &release.join(&spec.application.binary),
                    &specification,
                    &crate::compiler::linker_target(root).join("debug/oer-image-linker"),
                )
            })?;
        } else {
            build(false)?;
        }
        let elf = crate::snapshot(
            &release.join(&spec.application.binary),
            &output,
            files::RUNTIME_ELF,
        )?;
        {
            use oer_image_checks::linker::strict::{FRAGMENT, PLACEMENT, SPEC_FILE};
            for file in [FRAGMENT, PLACEMENT, SPEC_FILE] {
                if !output.join(file).is_file() {
                    return Err(format!("the runtime linker did not produce {file}").into());
                }
                bundle.reports.push(file.to_owned());
            }
        }
        elf
    };
    let runtime_bin = output.join(files::RUNTIME_BIN);
    let mut objcopy = oer_os_toolchain::command(oer_os_toolchain::Tool::LlvmObjcopy)?;
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
        if let Some(measured) = &outcome.sizes {
            sizes.elfs.insert("runtime".into(), measured.clone());
        }
        crate::apply_outcome(checks, crate::CheckedElf::Runtime, outcome, &mut bundle)?;
    }

    // The bootstrap embeds the runtime with `include_bytes!`; a stable path
    // that keeps its timestamp while its bytes are unchanged leaves the
    // bootstrap fresh.
    let bootstrap_cache = crate::compiler::lock_compile_cache(&cache)?;
    let embedded = std::path::absolute(cache.join("stage-two-runtime.bin"))?;
    crate::replace_if_changed(&runtime_bin, &embedded)?;
    let mut bootstrap = crate::cargo::command();
    bootstrap
        .current_dir(root)
        .args(["build", "--locked", "-p", &bootstrap_package])
        .args(["--profile", crate::FIRMWARE_PROFILE, "--target", target])
        .env("CARGO_TARGET_DIR", &cache)
        .env("CARGO_INCREMENTAL", "0")
        .env("PSRAM_RUNTIME_BIN", &embedded);
    for (name, _) in &spec.runtime_environment {
        bootstrap.env_remove(name);
    }
    crate::zeroed_inputs(&mut bootstrap, profile);
    crate::compiler::configure(
        &mut bootstrap,
        &crate::compiler::of_policy(
            &policy,
            instrumentation,
            root,
            &crate::linker_cache(root),
            target,
        ),
    )?;
    sizes.flags("bootstrap", &bootstrap, root)?;
    values.append(crate::large_values::build(
        &mut bootstrap,
        &log,
        root,
        &cache,
        target,
        crate::FIRMWARE_PROFILE,
        "bootstrap",
        &output,
    )?);
    let bootstrap_elf = crate::snapshot(
        &release.join(&bootstrap_package),
        &output,
        files::BOOTSTRAP_ELF,
    )?;
    drop(bootstrap_cache);
    match values.publish(&output) {
        Ok(()) => bundle
            .reports
            .extend(["large-values.json".into(), "large-values.txt".into()]),
        Err(error) => bundle
            .warnings
            .push(format!("large-values report: {error}")),
    }
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
        if let Some(measured) = &outcome.sizes {
            sizes.elfs.insert("bootstrap".into(), measured.clone());
        }
        crate::apply_outcome(checks, crate::CheckedElf::Bootstrap, outcome, &mut bundle)?;
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

    sizes.publish(&output)?;
    bundle.reports.push(crate::sizes::FILE.into());
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
