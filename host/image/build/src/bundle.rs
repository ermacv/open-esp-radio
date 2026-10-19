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
    let flash = profile.flash.clone().ok_or("the chip names no flash map")?;
    // Given boot files are another build's: refuse what is not the chip's
    // before anything is staged.
    if let BootFiles::Given {
        bootloader,
        partition_table,
    } = boot
    {
        let espflash = oer_image_encode::chip(&profile.espflash_chip)?;
        let read = |path: &Path| {
            std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
        };
        oer_image_encode::require_chip_image(&read(bootloader)?, espflash)
            .map_err(|error| format!("bootloader {}: {error}", bootloader.display()))?;
        oer_image_encode::require_partition_table(&read(partition_table)?)
            .map_err(|error| format!("partition table {}: {error}", partition_table.display()))?;
    }
    let staging = crate::exclusion::Staging::begin(output)?;
    let mut bundle = ImageBundle::new(staging.directory(), &profile, flash);
    std::fs::copy(application, bundle.application())?;
    match boot {
        BootFiles::Encode { elf } => {
            crate::staged::encode_boot_files(root, &profile, &std::fs::read(elf)?, &mut bundle)?;
        }
        BootFiles::Given {
            bootloader,
            partition_table,
        } => {
            std::fs::copy(bootloader, bundle.bootloader())?;
            std::fs::copy(partition_table, bundle.partitions())?;
            bundle.boot = Boot::EspIdfBootloader;
        }
    }
    staging.publish(bundle)
}

#[cfg(test)]
mod tests;
