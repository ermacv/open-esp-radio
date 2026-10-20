//! The images this runner flashes into its device under test: the bundles a
//! build or a run's archive hands it, written by the flash operation under
//! the run's lock of the board.

use std::path::Path;

use oer_hil_model::schema::image::ImageClass;
use oer_image_build::bundle::BootFiles;
use oer_image_build::bundle::ImageBundle;
use oer_lab_board::lock::DeviceAccess;

use crate::Result;

/// What the journal records of an image the runner flashes.
pub(crate) struct Flash<'a> {
    /// The image class.
    pub image: &'a str,
    pub revision: crate::flash::Revision,
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
    crate::flash::flash(
        &oer_lab_arbiter::journal::Journal::open()?,
        &oer_lab_board::image::Store::open()?,
        oer_lab_arbiter::owner::from_environment()?.as_str(),
        &lab.dut_board()?.lease(lock)?,
        &crate::flash::Image {
            bundle,
            name: flash.image,
            revision: flash.revision,
            origin: flash.origin,
        },
        oer_lab_board::stand::Via::Usb,
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
    oer_image_build::bundle::around(
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
