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

use std::path::{Path, PathBuf};

use crate::encode;
use crate::encode::Encoding;
use oer_chip_profile::policy::StackPolicy;
use oer_image_bundle::{ImageBundle, Staged, files};

use crate::{
    ImageSpec, Result, build_log::BuildLog, exclusion::Staging, instrumentation::Instrumentation,
};

mod compile;
pub(crate) use compile::cargo;

pub(crate) struct CompileInput<'a> {
    spec: &'a ImageSpec,
    profile: &'a oer_chip_profile::Profile,
    instrumentation: &'a Instrumentation,
    policy: &'a StackPolicy,
    task_stack_proven: bool,
    critical_section_hart: u32,
    harts: &'a [u32],
    panics: &'a oer_image_checks::panics::Contract,
    output: &'a Path,
    log: &'a BuildLog,
}

pub(crate) struct Compiled {
    runtime_elf: PathBuf,
    /// The packed runtime's payload CRC-32.
    runtime_crc32: u32,
    bootstrap_elf: PathBuf,
    values: crate::large_values::Report,
    sizes: crate::sizes::Summary,
}

/// The staged images' MMU page: bootstrap flash sources and stage two
/// use 64-KiB pages.
const MMU_PAGE_SIZE: u32 = 0x1_0000;
const LOADER_REPORT: &str = "bootstrap-loader.txt";

pub(crate) fn build(
    spec: &ImageSpec,
    profile: &oer_chip_profile::Profile,
    instrumentation: &Instrumentation,
    task_stack_proven: bool,
    critical_section_hart: u32,
    harts: &[u32],
    panics: &oer_image_checks::panics::Contract,
    compile: impl FnOnce(&CompileInput<'_>, &mut ImageBundle) -> Result<Compiled>,
) -> Result<ImageBundle> {
    let root = spec.root.as_path();
    let bootstrap_package = profile.bootstrap_package();
    let flash = profile.facts.flash.clone().ok_or("no flash map")?;
    // Every file goes into the staging directory; the bundle is published
    // only once the whole build succeeded.
    let staging = Staging::begin(&spec.output)?;
    let output = staging.directory().to_owned();
    let mut bundle = ImageBundle::new(&output, profile, flash);
    bundle.purpose = spec.purpose;
    bundle.layout_seed = spec.layout_seed;
    let log = BuildLog::create(&output.join(files::BUILD_LOG))?;
    // The image's declared harts resolve the policy's launch templates.
    let policy = StackPolicy::load(&root.join(&spec.stack_policy))?.for_image(harts)?;
    if task_stack_proven {
        // One source of the size: the bound, not also the policy.
        policy.proven_task_stacks()?;
    }
    let Compiled {
        runtime_elf,
        runtime_crc32,
        bootstrap_elf,
        mut values,
        mut sizes,
    } = compile(
        &CompileInput {
            spec,
            profile,
            instrumentation,
            policy: &policy,
            task_stack_proven,
            critical_section_hart,
            harts,
            panics,
            output: &output,
            log: &log,
        },
        &mut bundle,
    )?;
    // Cache-off placement is a prerequisite for executing a bootstrap, not
    // a reported-only product gate. Audit even a reused ELF and a run/probe.
    let memory = profile
        .facts
        .memory
        .as_ref()
        .ok_or("a staged bootstrap needs its SRAM map")?;
    let placement =
        oer_image_checks::placement::bootstrap(memory.sram, memory.flash_xip, &bootstrap_elf)
            .map_err(|error| log.failed("bootstrap cache-off placement", error))?;
    std::fs::write(
        output.join(oer_image_checks::placement::BOOTSTRAP_REPORT),
        placement,
    )?;
    bundle
        .reports
        .push(oer_image_checks::placement::BOOTSTRAP_REPORT.into());
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
                task_stack_proven: false,
                critical_section_hart,
                stage_two_elf: Some(&runtime_elf),
                panics: Some(panics),
            })
            .map_err(|error| log.failed("bootstrap checks", error))?;
        if let Some(measured) = &outcome.sizes {
            sizes.elfs.insert("bootstrap".into(), measured.clone());
        }
        crate::apply_outcome(
            checks,
            crate::CheckedElf::Bootstrap,
            outcome,
            &mut bundle,
            false,
        )?;
    }

    let elf = std::fs::read(&bootstrap_elf)?;
    let (application, loader_report) = encode_application(root, profile, &elf, &bundle)
        .map_err(|error| log.failed("encode the ESP application image", error))?;
    if let Some(report) = loader_report {
        std::fs::write(output.join(LOADER_REPORT), report)?;
        bundle.reports.push(LOADER_REPORT.into());
    }
    std::fs::write(bundle.application(), application)?;
    encode_boot_files(root, profile, &elf, &mut bundle)
        .map_err(|error| log.failed("encode the boot files", error))?;
    bundle.staged = Some(Staged {
        bootstrap_package: bootstrap_package.clone(),
        runtime_crc32,
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
) -> Result<(Vec<u8>, Option<String>)> {
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
        encode::chip(&profile.facts.espflash_chip)?,
        Encoding::application(&bundle.flash, MMU_PAGE_SIZE)?,
        Some(&root.join(&bundle.flash.partitions)),
        bundle.flash.partition_table,
        Some(&partition.name),
    )?;
    audit_application_image(&encoded.application, partition.size)?;
    let report = audit_loader_mappings(profile, &encoded.application)?;
    Ok((encoded.application, report))
}

/// A loader's segment-count contract is independent of ELF permissions and
/// cannot be downgraded by a diagnostic gate. Check the actual encoded image.
fn audit_loader_mappings(
    profile: &oer_chip_profile::Profile,
    bytes: &[u8],
) -> Result<Option<String>> {
    let Some(expected) = profile
        .facts
        .boot_chain
        .as_ref()
        .and_then(|chain| chain.mapped_segments)
    else {
        return Ok(None);
    };
    let memory = profile
        .facts
        .memory
        .as_ref()
        .ok_or("mapped loader segments need the flash map")?;
    let found = encode::mapped_segment_count(bytes, memory.flash_xip)?;
    if found != usize::from(expected.get()) {
        return Err(format!("bootstrap loader requires {expected} mapped ESP segments, found {found}; flash code is forbidden").into());
    }
    Ok(Some(format!(
        "bootstrap loader mapped-segments={found} expected={expected}\nresult=PASS\n"
    )))
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
        encode::chip(&profile.facts.espflash_chip)?,
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
