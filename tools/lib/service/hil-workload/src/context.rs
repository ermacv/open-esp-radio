//! Immutable laboratory inputs and the result channels of one workload
//! repetition.

use crate::{Result, measurements::Recorder, results::Results};
use oer_hil_lab::config::LabConfig;
use oer_hil_link::{Dut, Profile, SerialCapture};
use std::path::Path;

pub struct Context<'a> {
    pub lab: &'a LabConfig,
    /// The repetition's measurements: every capture's projection and the
    /// workload's own.
    pub measurements: Recorder,
    /// The repetition's typed observations.
    pub results: Results,
    /// The program-counter profile each capture arms after the boot's hello
    /// and drains when it finishes.
    pub profile: Option<oer_hil_scenario_catalog::ProfileRequest>,
    output: &'a Path,
    /// How a capture opens; the board under test reset through its console
    /// unless a test replaces it.
    opener: Option<Opener<'a>>,
    /// The run's writer of the board under test's images, when the run
    /// knows the scenario's image bundle.
    images: Option<&'a dyn BoardImages>,
    /// The run's access to the board under test, when its lease holds it:
    /// what every operation of a workload on the board goes through.
    device: Option<&'a oer_device_lock::DeviceAccess>,
}

/// The images of the board under test within the run's lease, for a
/// workload that alternates firmware on it (the PHY calibration
/// cross-check flashes the vendor firmware between production boots). Every
/// write goes through the flash operation (`oer-hil-flash`), which journals
/// it; the runner reflashes the scenario's image before its next scenario.
pub trait BoardImages: Sync {
    /// The scenario's own image, as the run built, archived and flashed it.
    fn scenario_image(&self) -> &oer_image_bundle::ImageBundle;
    /// Write `bundle`, which the board journal names `name`, and start it.
    fn flash(&self, bundle: &oer_image_bundle::ImageBundle, name: &str) -> Result<()>;
    /// Write the scenario's own image back and start it.
    fn restore(&self) -> Result<()>;
}

/// Opens the capture of one boot in a directory.
type Opener<'a> = Box<dyn Fn(&Path) -> Result<SerialCapture> + 'a>;

impl<'a> Context<'a> {
    pub fn new(lab: &'a LabConfig, output: &'a Path) -> Self {
        Self {
            lab,
            output,
            measurements: Recorder::default(),
            results: Results::default(),
            profile: None,
            opener: None,
            images: None,
            device: None,
        }
    }

    /// Let the workload operate the board under test under `device`.
    pub fn with_device(mut self, device: Option<&'a oer_device_lock::DeviceAccess>) -> Self {
        self.device = device;
        self
    }

    /// The board under test, leased under the run's access to it: its
    /// resets, console and power.
    pub fn dut_board(&self) -> Result<oer_stand_board::LeasedBoard> {
        let device = self
            .device
            .ok_or("this run holds no lease of its device under test")?;
        self.lab.dut_board()?.lease(device)
    }

    /// Let the workload write the board under test's images through
    /// `images`.
    pub fn with_images(mut self, images: Option<&'a dyn BoardImages>) -> Self {
        self.images = images;
        self
    }

    /// The run's writer of the board under test's images; an error when the
    /// run cannot write them (a replayed image the run did not build).
    pub fn images(&self) -> Result<&'a dyn BoardImages> {
        self.images.ok_or_else(|| {
            "this run cannot write the board under test's images: it did not build the scenario's image"
                .into()
        })
    }

    /// Open every capture with `opener` instead of resetting the board.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_opener(mut self, opener: impl Fn(&Path) -> Result<SerialCapture> + 'a) -> Self {
        self.opener = Some(Box::new(opener));
        self
    }

    /// Profile every capture of this workload.
    pub fn with_profile(
        mut self,
        profile: Option<oer_hil_scenario_catalog::ProfileRequest>,
    ) -> Self {
        self.profile = profile;
        self
    }

    /// The repetition's output directory.
    pub fn output(&self) -> &Path {
        self.output
    }

    /// Every workload family uses the same reset/capture/measurement lifetime.
    /// Explicit finish and error unwinding both return observations to this
    /// repetition; fixture restoration remains with the concrete fixture owner.
    pub fn capture(&self, output: &Path) -> Result<SerialCapture> {
        self.capture_of(self.lab, output)
    }

    /// [`Self::capture`] of the board under test as `dut` describes it, which
    /// may differ from the laboratory's in its console or reset path.
    pub fn capture_of(&self, dut: &dyn Dut, output: &Path) -> Result<SerialCapture> {
        oer_process::check_cancelled()?;
        let relative = output
            .strip_prefix(self.output)
            .map_err(|_| "capture output is outside its repetition")?;
        let recorder = self.measurements.capture(relative)?;
        let capture = match &self.opener {
            Some(open) => open(output)?,
            None => {
                let operation = self
                    .device
                    .ok_or("this run holds no device lease")?
                    .operation()?;
                SerialCapture::start_with_reset(dut, output)?
                    .with_io_lifetime(operation.lifetime().clone())
            }
        }
        .observed_by(Box::new(recorder));
        Ok(match self.profile {
            Some(profile) => capture.profiled(Profile {
                control: oer_hil_scenario::profile_control(profile),
                request: serde_json::to_value(profile)?,
            })?,
            None => capture,
        })
    }

    pub fn with_capture<T>(
        &self,
        output: &Path,
        operation: impl FnOnce(&SerialCapture) -> Result<T>,
    ) -> Result<T> {
        let capture = self.capture(output)?;
        let result = operation(&capture);
        capture.finish_with(result)
    }

    /// Write the repetition's typed observations: the one result writer,
    /// called once when the workload returned, whatever its outcome.
    pub fn finish(&self) -> Result<()> {
        self.results.write(self.output)
    }
}

#[cfg(test)]
mod tests;
