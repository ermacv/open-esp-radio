//! Bundles made around an application encoded earlier.

use std::path::Path;

use oer_image_bundle::ImageBundle;

use crate::{Boot, Result};

/// The boot files of a bundle made around an application encoded before.
pub enum BootFiles<'a> {
    /// A staged image: the bootloader is encoded from an ELF of the
    /// application (its bytes do not depend on the ELF's code), the
    /// partition table from the chip's CSV, and the OTA selection beside them.
    Encode { elf: &'a Path },
    /// A catalog image the ESP-IDF bootloader loads: the bootloader and
    /// partition table of its own build ([`Boot::EspIdfBootloader`]).
    Given {
        bootloader: &'a Path,
        partition_table: &'a Path,
    },
}

/// The bundle published in `output` (built in its staging directory, see
/// [`crate::exclusion::Staging`]) of an `application` image encoded earlier (a
/// run's archive, or a catalog ESP-IDF build) for `chip` of the repository at
/// `root`, with the boot files `boot` names: what a replay or a catalog
/// application flashes, encoded here so a flash still only writes bundle
/// files.
pub fn around(
    root: &Path,
    chip: &str,
    application: &Path,
    boot: BootFiles<'_>,
    output: &Path,
) -> Result<ImageBundle> {
    let profile = crate::profile(root, chip)?;
    let flash = profile
        .facts
        .flash
        .clone()
        .ok_or("the chip names no flash map")?;
    match boot {
        BootFiles::Encode { elf } => {
            let staging = crate::exclusion::Staging::begin(output)?;
            let mut bundle = ImageBundle::new(staging.directory(), &profile, flash);
            std::fs::copy(application, bundle.application())?;
            crate::staged::encode_boot_files(root, &profile, &std::fs::read(elf)?, &mut bundle)?;
            staging.publish(bundle)
        }
        BootFiles::Given {
            bootloader,
            partition_table,
        } => esp_idf(
            &EspIdfChip {
                id: &profile.facts.id,
                rust_target: &profile.facts.rust_target,
                espflash_chip: &profile.facts.espflash_chip,
                flash: &flash,
            },
            application,
            bootloader,
            partition_table,
            output,
        ),
    }
}

/// A chip as the boot files of a catalog ESP-IDF build need it: the chip's
/// identity for the bundle and its flash map.
pub struct EspIdfChip<'a> {
    pub id: &'a str,
    pub rust_target: &'a str,
    /// The chip as `espflash` names it.
    pub espflash_chip: &'a str,
    pub flash: &'a oer_espressif_facts::FlashMap,
}

/// The bundle published in `output` of a catalog `application` with the
/// bootloader and partition table of its own ESP-IDF build ([`Boot::EspIdfBootloader`]).
pub fn esp_idf(
    chip: &EspIdfChip<'_>,
    application: &Path,
    bootloader: &Path,
    partition_table: &Path,
    output: &Path,
) -> Result<ImageBundle> {
    // Given boot files are another build's: refuse what is not the chip's
    // before anything is staged.
    let espflash = crate::encode::chip(chip.espflash_chip)?;
    let read =
        |path: &Path| std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()));
    crate::encode::require_chip_image(&read(bootloader)?, espflash)
        .map_err(|error| format!("bootloader {}: {error}", bootloader.display()))?;
    crate::encode::require_partition_table(&read(partition_table)?)
        .map_err(|error| format!("partition table {}: {error}", partition_table.display()))?;
    let staging = crate::exclusion::Staging::begin(output)?;
    let bundle = ImageBundle::from_parts(
        staging.directory(),
        chip.id,
        chip.rust_target,
        Boot::EspIdfBootloader,
        chip.flash.clone(),
    );
    std::fs::copy(application, bundle.application())?;
    std::fs::copy(bootloader, bundle.bootloader())?;
    std::fs::copy(partition_table, bundle.partitions())?;
    staging.publish(bundle)
}

#[cfg(test)]
mod tests;
