//! The images this runner flashes into its device under test: the bundles a
//! build or a run's archive hands it, written by the flash operation under
//! the run's lock of the board.

use std::path::Path;

use oer_device_lock::DeviceAccess;
use oer_hil_schema::image::ImageClass;
use oer_image::bundle::BootFiles;
use oer_image_bundle::ImageBundle;

use crate::Result;

/// What the journal records of an image the runner flashes.
pub(crate) struct Flash<'a> {
    /// The image class.
    pub image: &'a str,
    pub revision: oer_hil_flash::Revision,
    /// The run that flashes it.
    pub origin: String,
}

/// Flash `bundle` into the device under test of `lab` through the flash
/// operation under the run's access `lock` to it: written and receipted,
/// started, then journaled.
pub(crate) fn flash_dut(
    lab: &oer_hil_lab::config::LabConfig,
    lock: &DeviceAccess,
    bundle: &ImageBundle,
    flash: Flash<'_>,
) -> Result<()> {
    oer_hil_flash::flash(
        &oer_stand_journal::Journal::open()?,
        &oer_devices::image::Store::open()?,
        oer_stand_owners::from_environment()?.as_str(),
        &lab.dut_board()?.lease(lock)?,
        &oer_hil_flash::Image {
            bundle,
            name: flash.image,
            revision: flash.revision,
            origin: flash.origin,
        },
        oer_stand_board::Via::Usb,
    )
    .map(drop)
}

/// The bundle of an archived `application` of `chip` with what a run keeps
/// beside it: the bootstrap ELF, from which the image pipeline encodes its
/// boot files again. The bundle goes below this checkout's replay directory
/// of `run_id`.
pub(crate) fn archived(
    root: &Path,
    chip: &str,
    application: &Path,
    run_id: &str,
    image: ImageClass,
) -> Result<ImageBundle> {
    let bootstrap = application.with_file_name("bootstrap.elf");
    if !bootstrap.is_file() {
        return Err(format!(
            "the archived {} image has no bootstrap ELF to encode its boot files from; \
             rebuild it",
            image.id()
        )
        .into());
    }
    let boot = BootFiles::Encode { elf: &bootstrap };
    oer_image::bundle::around(
        root,
        chip,
        application,
        boot,
        &root
            .join("target/hil")
            .join(chip)
            .join("replay")
            .join(run_id)
            .join(image.id()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_without_its_bootstrap_elf_cannot_be_replayed() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let directory = tempfile::tempdir().unwrap();
        let application = directory.path().join("application.bin");
        std::fs::write(&application, b"application").unwrap();
        let chip = oer_chip_profile::Profile::all(&root).unwrap()[0].id.clone();
        let run = format!("r{}", std::process::id());
        let error = archived(&root, &chip, &application, &run, ImageClass::SystemWatchdog)
            .unwrap_err()
            .to_string();
        assert!(error.contains("bootstrap ELF"), "{error}");
    }
}
