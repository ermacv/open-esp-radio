//! Suite, scenario and repetition lifecycle orchestration.

use oer_hil_run_bundle_format::run::RunEventKind;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::{Result, emit_json, fixture};
use oer_hil_lab::config::LabConfig;
use oer_hil_run_bundle::run::RunSession;
use oer_hil_run_bundle_format::run::Failure;
use oer_hil_run_bundle_format::run::FailureKind;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::PlanDisposition;
use oer_hil_run_bundle_format::run::PlanEntry;
use oer_hil_run_bundle_format::run::PlannedFirmware;
use oer_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_hil_run_bundle_format::run::RepetitionResult;
use oer_hil_run_bundle_format::run::RunPlan;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_schema::image::ImageClass;

use crate::scenario::{Catalog, Families, Scenario, requirements};
use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_workload::family::Registry as _;

use super::{
    firmware::{self, RunFirmware},
    preflight,
};

pub(crate) fn selection_description(tags: &[String]) -> String {
    if tags.is_empty() {
        String::from("all scenarios")
    } else {
        format!("all scenarios with tags: {}", tags.join(", "))
    }
}

pub(crate) struct Invocation {
    pub(crate) arguments: Vec<OsString>,
    /// Shell command run after the scenarios within the run's lease.
    pub(crate) then: Option<String>,
}

pub(crate) fn run_all(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &[&Scenario],
    description: String,
    build: oer_hil_image::CurrentBuild,
    invocation: Invocation,
) -> Result<()> {
    let mut session = start_run(
        root,
        lab,
        catalog,
        selected,
        description,
        Some(PlannedFirmware::BuildCurrent),
        invocation,
    )?;
    let prebuilt = prebuild(&mut session, lab, selected, build.clone())?;
    let lease = lease_stand(&mut session, lab, selected)?;
    let results = {
        let mut operations = LiveSuite {
            root,
            lab,
            firmware: FirmwarePreparation::BuildCurrent(build),
            prebuilt,
            lease: Some(lease),
            flashed: None,
            peer_image: None,
            peer_flash: None,
            recovered: Vec::new(),
            recovery_silent: false,
        };
        execute_selected(&mut session, &mut operations, selected)?
    };
    finish_run(session, results)
}

pub(crate) fn run_one(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &Scenario,
    firmware: RunFirmware,
    mut invocation: Invocation,
) -> Result<()> {
    let then = invocation.then.take();
    let selected_entries = [selected];
    let mut session = start_run(
        root,
        lab,
        catalog,
        &selected_entries,
        format!("scenario: {}", selected.id()),
        Some(firmware.plan()),
        invocation,
    )?;
    let prebuilt = match &firmware {
        RunFirmware::BuildCurrent(build) => {
            prebuild(&mut session, lab, &selected_entries, build.clone())?
        }
        RunFirmware::Replay(_) => Vec::new(),
    };
    let lease = lease_stand(&mut session, lab, &selected_entries)?;
    let results = {
        let mut operations = LiveSuite {
            root,
            lab,
            firmware: FirmwarePreparation::Selected(&firmware),
            prebuilt,
            lease: Some(lease),
            flashed: None,
            peer_image: None,
            peer_flash: None,
            recovered: Vec::new(),
            recovery_silent: false,
        };
        let results = execute_one(&mut session, &mut operations, selected)?;
        operations.run_then(then.as_deref(), &mut session)?;
        results
    };
    finish_run(session, results)
}

/// Execute several explicitly named scenarios as one run: the images are
/// built first, then one stand lease covers every flash and scenario.
pub(crate) fn run_many(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &[&Scenario],
    firmware: RunFirmware,
    mut invocation: Invocation,
) -> Result<()> {
    let then = invocation.then.take();
    let mut session = start_run(
        root,
        lab,
        catalog,
        selected,
        scenarios_description(selected),
        Some(firmware.plan()),
        invocation,
    )?;
    let prebuilt = match &firmware {
        RunFirmware::BuildCurrent(build) => prebuild(&mut session, lab, selected, build.clone())?,
        RunFirmware::Replay(_) => Vec::new(),
    };
    let lease = lease_stand(&mut session, lab, selected)?;
    let results = {
        let mut operations = LiveSuite {
            root,
            lab,
            firmware: FirmwarePreparation::Selected(&firmware),
            prebuilt,
            lease: Some(lease),
            flashed: None,
            peer_image: None,
            peer_flash: None,
            recovered: Vec::new(),
            recovery_silent: false,
        };
        let results = execute_selected(&mut session, &mut operations, selected)?;
        operations.run_then(then.as_deref(), &mut session)?;
        results
    };
    finish_run(session, results)
}

/// The named catalog scenarios, in order; each may be named once.
pub(crate) fn named_scenarios(catalog: &Catalog, ids: &[String]) -> Result<Vec<Scenario>> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| {
            if ids[..index].contains(id) {
                return Err(format!("scenario `{id}` is selected twice").into());
            }
            let scenario = catalog.get(id)?;
            refuse_unsupported(&[scenario])?;
            Ok(scenario.clone())
        })
        .collect()
}

/// The one image class a replayed firmware must serve.
pub(crate) fn single_image_class(selected: &[Scenario]) -> Result<ImageClass> {
    let first = selected.first().ok_or("no scenario selected")?;
    match selected
        .iter()
        .find(|scenario| scenario.image() != first.image())
    {
        Some(other) => Err(format!(
            "--firmware-from replays one image class; `{}` uses `{}`, `{}` uses `{}`",
            first.id(),
            first.image().id(),
            other.id(),
            other.image().id()
        )
        .into()),
        None => Ok(first.image()),
    }
}

pub(crate) fn scenarios_description(selected: &[&Scenario]) -> String {
    format!(
        "scenarios: {}",
        selected
            .iter()
            .map(|scenario| scenario.id())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Build the current-source image of every class with a scenario whose
/// configuration preconditions hold. Building needs no hardware, so it runs
/// before the stand is leased; flashing and execution follow under the lease.
fn prebuild(
    session: &mut RunSession,
    lab: &LabConfig,
    selected: &[&Scenario],
    build: oer_hil_image::CurrentBuild,
) -> Result<Vec<(ImageClass, firmware::Built)>> {
    let mut built = Vec::new();
    for (class, scenarios) in group_selected_scenarios(selected) {
        oer_os_process::check_cancelled()?;
        if scenarios
            .iter()
            .all(|scenario| fixture::preflight::scenario_precondition(lab, scenario).is_some())
        {
            continue;
        }
        built.push((class, firmware::build_image(class, build.clone(), session)?));
    }
    Ok(built)
}

/// Refuse an explicit selection naming a scenario the current firmware
/// cannot run, before any image is built.
pub(crate) fn refuse_unsupported(selected: &[&Scenario]) -> Result<()> {
    match selected
        .iter()
        .find_map(|scenario| Some((scenario.id(), scenario.header.unsupported.as_deref()?)))
    {
        Some((id, reason)) => Err(format!("scenario `{id}` is unsupported: {reason}").into()),
        None => Ok(()),
    }
}

/// The chip that runs `selected`: `requested`, or the only chip the runner
/// builds every selected image for. A scenario names no chip; the chips
/// whose HIL agents build its image run it.
pub(crate) fn select_chip(
    root: &Path,
    selected: &[&Scenario],
    requested: Option<&str>,
) -> Result<String> {
    let chips = oer_hil_image::chips_building(root, &image_classes(selected))?;
    match (requested, chips.as_slice()) {
        (Some(chip), _) if chips.iter().any(|known| known == chip) => Ok(chip.to_owned()),
        (Some(chip), []) => Err(format!(
            "no chip builds every image the selected scenarios use, {chip} neither"
        )
        .into()),
        (Some(chip), _) => Err(format!(
            "{chip} builds no image some selected scenario uses; they run on {}",
            chips.join(", ")
        )
        .into()),
        (None, [chip]) => Ok(chip.clone()),
        (None, []) => {
            Err("no chip builds every image the selected scenarios use; run them separately".into())
        }
        (None, _) => Err(format!(
            "the selected scenarios run on {}; name one with --chip",
            chips.join(", ")
        )
        .into()),
    }
}

/// Tag of scenarios that measure the radio environment and transmit without
/// regard for others: they need their ranges quiet and are noisy themselves.
/// It stands for `air-strict` plus `air-noisy`.
pub(crate) const AIR_EXCLUSIVE_TAG: &str = "air-exclusive";
/// Tag of scenarios that need no other transmitter in their ranges.
pub(crate) const AIR_STRICT_TAG: &str = "air-strict";
/// Tag of scenarios that transmit noisily: a continuous carrier, transmission
/// without CSMA, DTM or a throughput flood.
pub(crate) const AIR_NOISY_TAG: &str = "air-noisy";

/// How `scenario` uses the air: the ranges its family states, tolerant of
/// others' protocol traffic and transmitting normally unless its tags say
/// otherwise.
pub(crate) fn air_use(lab: &LabConfig, scenario: &Scenario) -> Vec<oer_hil_lab::lock::Spectrum> {
    use oer_hil_lab::lock::{BAND_2G4, Emits, Need, Spectrum};
    use oer_hil_scenario::{AirUse, ScenarioFamily as _};
    let tagged = |tag: &str| scenario.header.tags.iter().any(|known| known == tag);
    let need = if tagged(AIR_EXCLUSIVE_TAG) || tagged(AIR_STRICT_TAG) {
        Need::Strict
    } else {
        Need::Tolerant
    };
    let emits = if tagged(AIR_EXCLUSIVE_TAG) || tagged(AIR_NOISY_TAG) {
        Emits::Noisy
    } else {
        Emits::Normal
    };
    scenario
        .family
        .air_use()
        .into_iter()
        .map(|air| match air {
            AirUse::Band2G4 => Spectrum::new(BAND_2G4, need, emits),
            AirUse::Ieee802154Channel(channel) => Spectrum::ieee802154(channel, need, emits),
            AirUse::WifiLink => {
                Spectrum::new(lab.wifi_range_khz(scenario.plan().wifi), need, emits)
            }
        })
        .collect()
}

/// The stand lease of a run: its boards and fixtures, the frequency ranges
/// of its scenarios, and yielding at scenario boundaries when the run has
/// several scenarios.
pub(crate) fn lease_request(
    lab: &LabConfig,
    selected: &[&Scenario],
) -> oer_hil_lab::lock::LeaseRequest {
    let mut air = Vec::new();
    for range in selected.iter().flat_map(|scenario| air_use(lab, scenario)) {
        if !air.contains(&range) {
            air.push(range);
        }
    }
    oer_hil_lab::lock::LeaseRequest {
        required: requirements(selected),
        scenarios: selected
            .iter()
            .map(|scenario| scenario.id().to_owned())
            .collect(),
        air,
        divisible: selected.len() > 1,
        device: true,
        run: None,
    }
}

/// Wait for the stand, then observe the leased fixture.
fn lease_stand(
    session: &mut RunSession,
    lab: &LabConfig,
    selected: &[&Scenario],
) -> Result<oer_hil_lab::lock::FixtureLock> {
    session.record_event(RunEventKind::StandLeaseRequested, None, None, None)?;
    let request = oer_hil_lab::lock::LeaseRequest {
        run: Some(session.id().to_owned()),
        ..lease_request(lab, selected)
    };
    let fixture = oer_hil_lab::lock::FixtureLock::lease(lab, request.clone())?;
    session.record_event(RunEventKind::StandLeaseGranted, None, None, None)?;
    oer_os_durable::atomic_json(
        &session.directory().join("air.json"),
        &oer_hil_lab::lock::air_record(&request)?,
    )?;
    let lab_provenance = oer_hil_lab::provenance::capture(lab, requirements(selected))?;
    session.record_lab_provenance(&lab_provenance)?;
    session.record_event(RunEventKind::LabProvenanceCaptured, None, None, None)?;
    Ok(fixture)
}

enum FirmwarePreparation<'a> {
    BuildCurrent(oer_hil_image::CurrentBuild),
    Selected(&'a RunFirmware),
}

struct LiveSuite<'a> {
    root: &'a Path,
    lab: &'a LabConfig,
    firmware: FirmwarePreparation<'a>,
    /// Images built before the lease; a class without one is prepared whole.
    prebuilt: Vec<(ImageClass, firmware::Built)>,
    lease: Option<oer_hil_lab::lock::FixtureLock>,
    /// The class on the device and, when built by this run, its archive.
    flashed: Option<(ImageClass, Option<Box<oer_hil_image::Artifacts>>)>,
    /// The peer image this run brought up to its current catalog build.
    peer_image: Option<&'static str>,
    /// What the board journal recorded for that image's flash.
    peer_flash: Option<PeerImageRecord>,
    /// The image classes whose silence this run already answered with the
    /// recovery image.
    recovered: Vec<ImageClass>,
    /// Whether this run's own recovery attempt found the recovery image
    /// silent: flashing it again would only repeat that attempt and
    /// overwrite its evidence.
    recovery_silent: bool,
}

trait SuiteEffects {
    fn check_cancelled(&mut self) -> Result<()>;
    fn preflight(&mut self, scenario: &Scenario) -> Option<Failure>;
    fn prepare_image(
        &mut self,
        class: ImageClass,
        session: &mut RunSession,
    ) -> Result<Option<Failure>>;
    fn execute_scenario(
        &mut self,
        scenario: &Scenario,
        session: &RunSession,
    ) -> Result<ScenarioResult>;
    /// After a scenario: when its image stopped answering after it booted
    /// and the board is still loadable, flash the chip's recovery image.
    fn after_scenario(&mut self, _scenario: &Scenario, _session: &mut RunSession) -> Result<()> {
        Ok(())
    }
    /// A boundary between steps of divisible work. An over-budget lease that
    /// blocks waiting requests yields here and queues again; the image of
    /// `class` is then flashed again before the next scenario.
    fn yield_point(
        &mut self,
        _class: Option<ImageClass>,
        _session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        Ok(None)
    }
}

impl SuiteEffects for LiveSuite<'_> {
    fn check_cancelled(&mut self) -> Result<()> {
        oer_os_process::check_cancelled()
    }

    fn preflight(&mut self, scenario: &Scenario) -> Option<Failure> {
        if let Err(error) = self.restore_peer(scenario) {
            return Some(Failure::new(
                FailureKind::Precondition,
                format!(
                    "cannot restore `{}` on the peer board: {error}",
                    scenario
                        .family
                        .peer_image()
                        .map_or("the peer image", |image| image.name)
                ),
            ));
        }
        if let Err(error) = self.peer_answers(scenario) {
            return Some(Failure::new(FailureKind::Precondition, error.to_string()));
        }
        preflight::scenario_failure(self.lab, scenario)
    }

    fn prepare_image(
        &mut self,
        class: ImageClass,
        session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        let (failure, archive) =
            if let Some(index) = self.prebuilt.iter().position(|(built, _)| *built == class) {
                let (_, built) = self.prebuilt.remove(index);
                let archive = match &built {
                    firmware::Built::Archived(artifacts) => Some(artifacts.clone()),
                    firmware::Built::Failed(_) => None,
                };
                (
                    firmware::flash_built(self.lab, self.device_lock()?, class, built, session)?,
                    archive,
                )
            } else {
                let lock = self.device_lock()?;
                match &self.firmware {
                    FirmwarePreparation::BuildCurrent(build)
                    | FirmwarePreparation::Selected(RunFirmware::BuildCurrent(build)) => {
                        // The archive stays known, so a workload can write
                        // the scenario's image back (`BoardImages`).
                        let built = firmware::build_image(class, build.clone(), session)?;
                        let archive = match &built {
                            firmware::Built::Archived(artifacts) => Some(artifacts.clone()),
                            firmware::Built::Failed(_) => None,
                        };
                        (
                            firmware::flash_built(self.lab, lock, class, built, session)?,
                            archive,
                        )
                    }
                    FirmwarePreparation::Selected(firmware) => (
                        firmware::prepare_run_image(
                            self.root, self.lab, lock, class, firmware, session,
                        )?,
                        None,
                    ),
                }
            };
        self.flashed = failure.is_none().then_some((class, archive));
        Ok(failure)
    }

    fn after_scenario(&mut self, scenario: &Scenario, session: &mut RunSession) -> Result<()> {
        let class = scenario.image();
        let recovery = RECOVERY_IMAGE;
        let silent = oer_hil_lab::recovery::image_silent(class.id());
        if !needs_recovery_image(
            silent,
            oer_hil_lab::recovery::device_quarantined(),
            self.recovered.contains(&class),
            self.recovery_silent,
        ) {
            if silent && self.recovery_silent {
                eprintln!(
                    "hil: the {} image went silent after booting; the {} recovery image \
                     already proved silent in this run, so it is not flashed again",
                    class.id(),
                    recovery.id()
                );
            }
            return Ok(());
        }
        self.recovered.push(class);
        let build = match &self.firmware {
            FirmwarePreparation::BuildCurrent(build)
            | FirmwarePreparation::Selected(RunFirmware::BuildCurrent(build)) => build.clone(),
            FirmwarePreparation::Selected(RunFirmware::Replay(_)) => oer_hil_image::CurrentBuild {
                source: oer_hil_image::ImageSource::Tree(self.root.to_owned()),
                layout_seed: None,
                features: Default::default(),
            },
        };
        eprintln!(
            "hil: the {} image went silent after booting; flashing the {} recovery image",
            class.id(),
            recovery.id()
        );
        let failure =
            firmware::prepare_image(self.lab, self.device_lock()?, recovery, build, session)?;
        self.flashed = failure.is_none().then_some((recovery, None));
        let output = session.directory().join("recovery-image");
        let outcome = after_reflash(failure, || {
            crate::execution::preflight::answers_as(self.lab, recovery, &output)
        });
        let mac = self.lab.dut.mac.clone();
        let origin = format!("run {}", session.id());
        match outcome {
            AfterReflash::Recovered => {
                oer_hil_lab::recovery::record_reflash(Some(mac), origin);
                eprintln!("hil: the board answers its recovery image");
            }
            AfterReflash::ImageFault(why) => {
                // The board's ROM answered before the reflash: the run's
                // remaining scenarios of the recovery image are blocked
                // like those of any other silent image.
                oer_hil_lab::recovery::mark_image_silent(recovery.id());
                self.recovery_silent = true;
                eprintln!(
                    "hil: the board's ROM answers, so it is not quarantined; the {} recovery \
                     image built from this checkout is at fault: {why}",
                    recovery.id()
                );
            }
            AfterReflash::FlashFailed(why) => {
                // The flash needs the ROM, which may have gone silent since:
                // the board's own ladder decides, and quarantines it only if
                // its ROM stays silent.
                eprintln!(
                    "hil: the {} recovery image did not flash ({why}); climbing the board's \
                     reset ladder",
                    recovery.id()
                );
                let board = self.lab.dut_board()?.lease(self.device_lock()?)?;
                match oer_hil_lab::recovery::recover(&board, &output, None, &origin)? {
                    Some(judged) => {
                        if !judged.quarantined() {
                            oer_hil_lab::recovery::mark_image_silent(recovery.id());
                            self.recovery_silent = true;
                        }
                        eprintln!("hil: {}", judged.describe());
                    }
                    None => eprintln!("hil: the board was not judged"),
                }
            }
        }
        Ok(())
    }

    fn execute_scenario(
        &mut self,
        scenario: &Scenario,
        session: &RunSession,
    ) -> Result<ScenarioResult> {
        if scenario.family.peer_image().is_some()
            && let Some(record) = &self.peer_flash
        {
            let directory = session.scenario_directory(scenario.id());
            fs::create_dir_all(&directory)?;
            oer_os_durable::atomic_json(&directory.join("peer-image.json"), record)?;
        }
        let images = match &self.flashed {
            Some((class, Some(artifacts))) if *class == scenario.image() => {
                let repository = session.repository();
                Some(RunImages {
                    lab: self.lab,
                    lock: self.device_lock()?,
                    class: *class,
                    bundle: &artifacts.bundle,
                    revision: oer_hil_flash::Revision {
                        commit: Some(repository.commit.clone()),
                        dirty: Some(repository.dirty),
                    },
                    origin: format!("run {}", session.id()),
                    flashed: std::sync::atomic::AtomicBool::new(false),
                })
            }
            _ => None,
        };
        let result = run_scenario(
            self.lab,
            scenario,
            session,
            images
                .as_ref()
                .map(|images| images as &dyn oer_hil_workload::context::BoardImages),
            self.lease
                .as_ref()
                .and_then(oer_hil_lab::lock::FixtureLock::device),
        );
        // A workload that wrote another image leaves the board's image
        // unknown: the next scenario flashes its own.
        if images.is_some_and(|images| images.flashed.into_inner()) {
            self.flashed = None;
        }
        result
    }

    fn yield_point(
        &mut self,
        class: Option<ImageClass>,
        session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        // An over-budget lease yields to a waiter that outranks it; one that
        // has held long renews itself, so no step meets the hard limit.
        if !self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.yield_requested() || lease.renewal_due())
        {
            return Ok(None);
        }
        session.record_event(RunEventKind::StandLeaseYielded, None, class, None)?;
        let lease = self.lease.take().ok_or("the run holds no stand lease")?;
        self.lease = Some(lease.requeue(self.lab)?);
        session.record_event(RunEventKind::StandLeaseGranted, None, class, None)?;
        let Some(class) = class else {
            return Ok(None);
        };
        // Other owners may have flashed the board meanwhile.
        match self.flashed.take() {
            Some((flashed, Some(artifacts))) if flashed == class => {
                let failure = firmware::flash_archived_artifacts(
                    self.lab,
                    self.device_lock()?,
                    class,
                    &artifacts,
                    session,
                )?;
                self.flashed = failure.is_none().then_some((class, Some(artifacts)));
                Ok(failure)
            }
            Some((flashed, None)) if flashed == class => {
                let failure = match self.firmware {
                    FirmwarePreparation::Selected(RunFirmware::Replay(archived)) => {
                        firmware::reflash_replayed(
                            self.root,
                            self.lab,
                            self.device_lock()?,
                            archived,
                            session,
                        )?
                    }
                    _ => return self.prepare_image(class, session),
                };
                self.flashed = failure.is_none().then_some((class, None));
                Ok(failure)
            }
            _ => self.prepare_image(class, session),
        }
    }
}

impl LiveSuite<'_> {
    /// Run `command` with `sh -c` while the lease is held, with the lease's
    /// context so nested `cargo hil` commands join it, and the run's
    /// directory in `OER_HIL_RUN_DIRECTORY`. Its exit status is reported and
    /// recorded as an event; it never changes the run's outcome.
    fn run_then(&self, command: Option<&str>, session: &mut RunSession) -> Result<()> {
        let Some(command) = command else {
            return Ok(());
        };
        let lease = self.lease.as_ref().ok_or("the run holds no stand lease")?;
        session.record_event(RunEventKind::ThenStarted, None, None, None)?;
        eprintln!("hil: running `{command}` within the run's lease");
        let mut child = then_command(self.root, command, &lease.context()?, session.directory())?;
        let status =
            oer_os_process::owned::Child::spawn(&mut child).and_then(|mut child| child.wait());
        let kind = match &status {
            Ok(status) if status.success() => RunEventKind::ThenSucceeded,
            _ => RunEventKind::ThenFailed,
        };
        match status {
            Ok(status) => eprintln!("hil: `{command}` exited with {status}"),
            Err(error) => eprintln!("hil: cannot start `{command}`: {error}"),
        }
        session.record_event(kind, None, None, None)
    }

    /// The run's access to its device under test.
    fn device_lock(&self) -> Result<&oer_lab_board::lock::DeviceAccess> {
        self.lease
            .as_ref()
            .and_then(oer_hil_lab::lock::FixtureLock::device)
            .ok_or_else(|| "the run holds no lease of its device under test".into())
    }

    /// Bring the peer board to the catalog's peer image when another
    /// consumer's image or an older build is on it: the flash operation's
    /// catalog flash of the peer board this run leases, receipted and
    /// journaled like any flash. The scenario's `peer-image.json` records
    /// what the board's receipt says it runs.
    fn restore_peer(&mut self, scenario: &Scenario) -> Result<()> {
        let Some(image) = scenario.family.peer_image() else {
            return Ok(());
        };
        let image = image.name;
        // Once per image and run: the catalog build and the journal decide
        // whether the board already carries the current build.
        if self.peer_image == Some(image) {
            return Ok(());
        }
        let lock = self
            .lease
            .as_ref()
            .and_then(oer_hil_lab::lock::FixtureLock::peer)
            .ok_or("the run holds no lease of its peer board")?;
        let board = self.lab.peer_board()?.lease(lock)?;
        let journal = oer_lab_arbiter::journal::Journal::open()?;
        let flashed = oer_hil_flash::catalog::flash(
            self.root,
            &journal,
            &oer_lab_board::image::Store::open()?,
            oer_lab_arbiter::owner::from_environment()?.as_str(),
            &board,
            &oer_hil_flash::catalog::Request {
                image,
                chip: board.chip(),
                if_changed: true,
                via: oer_lab_board::stand::Via::Usb,
                origin: "HIL run",
            },
        )?;
        self.peer_image = Some(image);
        self.peer_flash = Some(peer_image_record(&journal, board.mac(), flashed.receipt())?);
        Ok(())
    }

    /// The peer console answers `SYNC` before the scenario starts, under the
    /// run's lease: a wedged console fails the scenario's precondition with
    /// its recovery instead of breaking it midway.
    fn peer_answers(&self, scenario: &Scenario) -> Result<()> {
        if scenario.family.peer_image().is_none() {
            return Ok(());
        }
        let path = self.lab.peer()?.serial()?;
        let live = oer_hil_lab::peer_console(&path)
            .and_then(|mut link| oer_hil_link::peer::answers_sync(&mut link, PEER_SYNC_TIMEOUT));
        match live {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "the peer console {} does not answer SYNC; {PEER_WEDGE}",
                path.display()
            )
            .into()),
            Err(error) => Err(format!("{error}; {PEER_WEDGE}").into()),
        }
    }
}

/// The run's writer of the board under test's images during one scenario:
/// the scenario's archived bundle, written back through the flash operation
/// under the run's lock of the board, like any flash of the run.
struct RunImages<'a> {
    lab: &'a LabConfig,
    lock: &'a oer_lab_board::lock::DeviceAccess,
    class: ImageClass,
    bundle: &'a oer_image_build::bundle::ImageBundle,
    revision: oer_hil_flash::Revision,
    origin: String,
    /// Whether the workload wrote any image.
    flashed: std::sync::atomic::AtomicBool,
}

impl oer_hil_workload::context::BoardImages for RunImages<'_> {
    fn scenario_image(&self) -> &oer_image_build::bundle::ImageBundle {
        self.bundle
    }

    fn flash(&self, bundle: &oer_image_build::bundle::ImageBundle, name: &str) -> Result<()> {
        self.flashed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        crate::board::flash_dut(
            self.lab,
            self.lock,
            bundle,
            crate::board::Flash {
                image: name,
                revision: self.revision.clone(),
                origin: self.origin.clone(),
            },
        )
    }

    fn restore(&self) -> Result<()> {
        self.flash(self.bundle, self.class.id())
    }
}

/// The catalog image the reference peer board ran during a scenario, from
/// the devices layer's receipt of its flash: the whole bundle's digest and
/// the application's, with the source commit the board journal recorded
/// beside the same application digest (a hint, never the identity).
/// Written into the scenario's directory beside its results.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PeerImageRecord {
    pub schema: u8,
    pub board: String,
    pub image: String,
    pub bundle_digest: String,
    pub application_sha256: String,
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

/// The record of `receipt` on the board with `mac`.
fn peer_image_record(
    journal: &oer_lab_arbiter::journal::Journal,
    mac: &oer_lab_board::lock::DeviceId,
    receipt: &oer_lab_board::image::Receipt,
) -> Result<PeerImageRecord> {
    let application = receipt
        .segment("application")
        .ok_or("the peer's receipt names no application segment")?
        .sha256
        .clone();
    let (commit, dirty) = match journal.latest_flash(mac)?.map(|event| event.kind) {
        Some(oer_lab_arbiter::journal::BoardEventKind::Flashed {
            application_sha256,
            commit,
            dirty,
            ..
        }) if application_sha256.eq_ignore_ascii_case(&application) => (commit, dirty),
        _ => (None, None),
    };
    Ok(PeerImageRecord {
        schema: 2,
        board: mac.to_string(),
        image: receipt.image.clone(),
        bundle_digest: receipt.digest.clone(),
        application_sha256: application,
        commit,
        dirty,
    })
}

/// How long one `SYNC` of the preflight waits for the peer's `@READY`.
const PEER_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Where a wedged peer console's recovery is described.
const PEER_WEDGE: &str = "a wedged USB console recovers as the IEEE 802.15.4 \
    reference peer's README (hil/peers) describes under `A wedged USB console`";

/// `sh -c command` in `root` with the lease's `environment` and the run's
/// `directory` in `OER_HIL_RUN_DIRECTORY`.
fn then_command(
    root: &Path,
    command: &str,
    context: &oer_os_process::Context,
    directory: &Path,
) -> Result<std::process::Command> {
    let mut child = oer_os_process::command("sh");
    child
        .args(["-c", command])
        .current_dir(root)
        .env("OER_HIL_RUN_DIRECTORY", directory);
    context.apply(&mut child)?;
    Ok(child)
}

fn execute_selected(
    session: &mut RunSession,
    effects: &mut impl SuiteEffects,
    selected: &[&Scenario],
) -> Result<Vec<ScenarioResult>> {
    let mut results = Vec::with_capacity(selected.len());
    for (class, class_scenarios) in group_selected_scenarios(selected) {
        effects.check_cancelled()?;
        let mut executable = Vec::with_capacity(class_scenarios.len());
        for scenario in class_scenarios {
            if let Some(failure) = effects.preflight(scenario) {
                session.record_event(
                    RunEventKind::ScenarioBlocked,
                    Some(scenario.id()),
                    Some(class),
                    Some(Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(session, scenario, failure)?);
            } else {
                executable.push(scenario);
            }
        }
        if executable.is_empty() {
            continue;
        }
        effects.yield_point(None, session)?;
        if let Some(failure) = effects.prepare_image(class, session)? {
            for scenario in executable {
                session.record_event(
                    RunEventKind::ScenarioBlocked,
                    Some(scenario.id()),
                    Some(class),
                    Some(Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(session, scenario, failure.clone())?);
            }
            continue;
        }
        for (index, scenario) in executable.into_iter().enumerate() {
            effects.check_cancelled()?;
            if index > 0
                && let Some(failure) = effects.yield_point(Some(class), session)?
            {
                session.record_event(
                    RunEventKind::ScenarioBlocked,
                    Some(scenario.id()),
                    Some(class),
                    Some(Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(session, scenario, failure)?);
                continue;
            }
            session.record_event(
                RunEventKind::ScenarioStarted,
                Some(scenario.id()),
                Some(class),
                None,
            )?;
            let result = effects.execute_scenario(scenario, session)?;
            effects.after_scenario(scenario, session)?;
            session.record_event(
                RunEventKind::ScenarioFinished,
                Some(scenario.id()),
                Some(class),
                Some(result.outcome),
            )?;
            results.push(result);
        }
    }
    Ok(results)
}

fn execute_one(
    session: &mut RunSession,
    effects: &mut impl SuiteEffects,
    selected: &Scenario,
) -> Result<Vec<ScenarioResult>> {
    if let Some(failure) = effects.preflight(selected) {
        session.record_event(
            RunEventKind::ScenarioBlocked,
            Some(selected.id()),
            Some(selected.image()),
            Some(Outcome::Blocked),
        )?;
        return Ok(vec![write_blocked_scenario(session, selected, failure)?]);
    }
    if let Some(failure) = effects.prepare_image(selected.image(), session)? {
        session.record_event(
            RunEventKind::ScenarioBlocked,
            Some(selected.id()),
            Some(selected.image()),
            Some(Outcome::Blocked),
        )?;
        return Ok(vec![write_blocked_scenario(session, selected, failure)?]);
    }
    session.record_event(
        RunEventKind::ScenarioStarted,
        Some(selected.id()),
        Some(selected.image()),
        None,
    )?;
    let result = effects.execute_scenario(selected, session)?;
    effects.after_scenario(selected, session)?;
    session.record_event(
        RunEventKind::ScenarioFinished,
        Some(selected.id()),
        Some(selected.image()),
        Some(result.outcome),
    )?;
    Ok(vec![result])
}

/// What follows the recovery image's flash.
#[derive(Debug, Eq, PartialEq)]
enum AfterReflash {
    /// The board answers its recovery image: journal the recovery.
    Recovered,
    /// The recovery image did not build, or was flashed and did not answer,
    /// why. The flash proved the ROM answers, so the image or the checkout
    /// that built it is at fault, not the board.
    ImageFault(String),
    /// The recovery image did not flash, why: the ROM it needs may have gone
    /// silent since it last answered, so the board's ladder judges it.
    FlashFailed(String),
}

/// What the reflash of the recovery image showed: its preparation's
/// `failure`, or else whether the flashed image `answered`.
fn after_reflash(
    failure: Option<Failure>,
    answered: impl FnOnce() -> std::result::Result<(), String>,
) -> AfterReflash {
    match failure {
        Some(failure) if failure.kind == FailureKind::ImageFlash => {
            AfterReflash::FlashFailed(failure.message)
        }
        Some(failure) => AfterReflash::ImageFault(failure.message),
        None => match answered() {
            Ok(()) => AfterReflash::Recovered,
            Err(why) => AfterReflash::ImageFault(why),
        },
    }
}

/// Whether a scenario's end calls for the chip's recovery image: its image
/// went silent after booting, the board can still be loaded (not
/// quarantined), the run has not yet answered that image's silence, and its
/// own earlier recovery attempt did not find the recovery image silent.
fn needs_recovery_image(
    image_silent: bool,
    quarantined: bool,
    already: bool,
    recovery_silent: bool,
) -> bool {
    image_silent && !quarantined && !already && !recovery_silent
}

/// The image classes `selected` runs on, in the order runs build them.
/// The image a silent board is brought back to.
pub(crate) const RECOVERY_IMAGE: ImageClass = ImageClass::BootSmoke;

pub(crate) fn image_classes(selected: &[&Scenario]) -> Vec<ImageClass> {
    group_selected_scenarios(selected)
        .into_iter()
        .map(|(class, _)| class)
        .collect()
}

fn group_selected_scenarios<'a>(selected: &[&'a Scenario]) -> Vec<(ImageClass, Vec<&'a Scenario>)> {
    ImageClass::ALL
        .into_iter()
        .filter_map(|class| {
            let scenarios = selected
                .iter()
                .copied()
                .filter(|entry| entry.image() == class)
                .collect::<Vec<_>>();
            (!scenarios.is_empty()).then_some((class, scenarios))
        })
        .collect()
}

fn start_run(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &[&Scenario],
    selection: String,
    firmware: Option<PlannedFirmware>,
    invocation: Invocation,
) -> Result<RunSession> {
    let mut session = RunSession::create(
        root,
        lab.chip(),
        lab.cell_id(),
        &lab.dut.id,
        &lab.dut.serial,
        invocation.arguments,
    )?;
    session.report_interruption_to(|report| {
        let _ = oer_hil_workload::emit_json(report, false);
    });
    let entries = catalog
        .all()
        .iter()
        .map(|scenario| {
            let is_selected = selected.iter().any(|entry| entry.id() == scenario.id());
            PlanEntry {
                scenario: scenario.id().to_owned(),
                image: scenario.image(),
                repetitions: scenario.repetitions(),
                disposition: if is_selected {
                    PlanDisposition::Selected
                } else {
                    PlanDisposition::Filtered
                },
                reason: (!is_selected).then(|| format!("excluded by `{selection}`")),
                requirements: Some(scenario.requirements()),
            }
        })
        .collect();
    session.write_plan(&RunPlan {
        schema: RUN_SCHEMA,
        run_id: session.id().to_owned(),
        selection,
        firmware,
        entries,
    })?;
    session.record_event(RunEventKind::PlanResolved, None, None, None)?;
    for scenario in selected {
        let directory = session.scenario_directory(scenario.id());
        fs::create_dir_all(&directory)?;
        oer_os_durable::atomic_json(&directory.join("scenario.json"), scenario)?;
    }
    Ok(session)
}

fn finish_run(session: RunSession, results: Vec<ScenarioResult>) -> Result<()> {
    oer_os_process::check_cancelled()?;
    let (suite, completion) = session.finish(results, oer_hil_analysis::report::views)?;
    emit_json(&completion, false)?;
    // Cancellation of a derived history update occurs after the run was
    // finished. Publish completion first, then preserve the CLI signal status.
    oer_os_process::check_cancelled()?;
    if suite.outcome.is_passed() {
        Ok(())
    } else {
        Err(format!(
            "HIL run `{}` failed: {} passed, {} failed, {} blocked, {} broken",
            suite.run_id,
            suite.counts.passed,
            suite.counts.failed,
            suite.counts.blocked,
            suite.counts.broken,
        )
        .into())
    }
}

fn run_scenario(
    lab: &LabConfig,
    selected: &Scenario,
    session: &RunSession,
    images: Option<&dyn oer_hil_workload::context::BoardImages>,
    device: Option<&oer_lab_board::lock::DeviceAccess>,
) -> Result<ScenarioResult> {
    let scenario_output = session.scenario_directory(selected.id());
    fs::create_dir_all(&scenario_output)?;
    oer_os_durable::atomic_json(&scenario_output.join("scenario.json"), selected)?;
    // An A/B arm's image is its class's with the arm's feature delta.
    let features = session
        .manifest()
        .experiment
        .as_ref()
        .map(|experiment| experiment.variant.features.clone())
        .unwrap_or_default();
    let mut repetitions = Vec::with_capacity(usize::from(selected.repetitions()));
    for number in 1..=selected.repetitions() {
        oer_os_process::check_cancelled()?;
        let relative = PathBuf::from("scenarios")
            .join(selected.id())
            .join(format!("repetition-{number:03}"));
        let output = session.directory().join(&relative);
        fs::create_dir_all(&output)?;
        repetitions.push(run_scenario_repetition(
            lab, selected, &features, number, &relative, &output, images, device,
        )?);
        // The next iteration checks cancellation. After the last repetition,
        // publish its completed boundary before observing campaign cancellation.
    }
    let result = ScenarioResult::from_repetitions(
        selected.id().to_owned(),
        selected.image(),
        selected.repetitions(),
        repetitions,
    );
    oer_os_durable::atomic_json(&scenario_output.join("result.json"), &result)?;
    Ok(result)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one argument per input of a repetition: its scenario, image and output"
)]
fn run_scenario_repetition(
    lab: &LabConfig,
    selected: &Scenario,
    features: &oer_hil_schema::image::FeatureDelta,
    repetition: u8,
    artifacts: &Path,
    output: &Path,
    images: Option<&dyn oer_hil_workload::context::BoardImages>,
    device: Option<&oer_lab_board::lock::DeviceAccess>,
) -> Result<RepetitionResult> {
    let plan = selected.plan();
    let resolved = lab.resolve(plan.wifi);
    let lab = &resolved;
    let started_unix_millis = oer_os_durable::unix_millis();
    let started = std::time::Instant::now();
    // The USB watch follows the peer the scenario uses, when it has one.
    let peer_serial = selected
        .family
        .peer_image()
        .and_then(|_| lab.peer().ok())
        .and_then(|peer| peer.serial().ok());
    let usb = oer_hil_lab::usb_events::UsbWatch::start(
        std::iter::once(lab.dut.serial.as_path()).chain(peer_serial.as_deref()),
        started_unix_millis,
    );
    let cleanup = oer_hil_workload::fixture::cleanup::Scope::new(output);
    if oer_hil_lab::recovery::device_quarantined() {
        return finalize_repetition(
            repetition,
            artifacts,
            &usb,
            output,
            started_unix_millis,
            started,
            cleanup,
            Outcome::BoardQuarantined,
            Some(Failure::new(
                FailureKind::Infrastructure,
                "the board was quarantined earlier in this run; the repetition did not touch it",
            )),
            Vec::new(),
        );
    }
    if oer_hil_lab::recovery::image_silent(selected.image().id()) {
        return finalize_repetition(
            repetition,
            artifacts,
            &usb,
            output,
            started_unix_millis,
            started,
            cleanup,
            Outcome::Blocked,
            Some(Failure::new(
                FailureKind::Precondition,
                format!(
                    "the {} image did not answer after booting earlier in this run; the \
                     repetition did not touch the board",
                    selected.image().id()
                ),
            )),
            Vec::new(),
        );
    }
    let mut fixtures = oer_hil_workload::fixture::Fixtures::default();
    let (outcome, failure, measurements) = match fixture::preflight::check(lab, selected)
        .and_then(|()| {
            for provider in Families::FIXTURES {
                provider.prepare(lab, &plan, output, &mut fixtures)?;
            }
            Ok(())
        })
        .and_then(|()| preflight::validate_flashed_image(lab, selected, features, output, device))
    {
        // A board that needs a person must not serve the run's remaining
        // repetitions or the next lease: the run stops.
        Err(error) if error.is::<oer_hil_lab::recovery::UnrecordedQuarantine>() => {
            return Err(error);
        }
        Err(error) => {
            let mut failure = super::classify(&*error);
            let outcome = if oer_os_process::is_cancelled(&*error)
                || oer_os_process::cancellation_requested()
            {
                Outcome::Interrupted
            } else if failure.kind == FailureKind::Infrastructure {
                Outcome::Broken
            } else {
                failure.kind = FailureKind::Precondition;
                Outcome::Blocked
            };
            (outcome, Some(failure), Vec::new())
        }
        Ok(()) => {
            let evidence =
                super::execute_workload(lab, selected, output, &fixtures, images, device)?;
            (evidence.outcome(), evidence.failure, evidence.measurements)
        }
    };
    // The fixtures are restored, in their cleanup scope, before it closes.
    drop(fixtures);
    finalize_repetition(
        repetition,
        artifacts,
        &usb,
        output,
        started_unix_millis,
        started,
        cleanup,
        outcome,
        failure,
        measurements,
    )
}

#[allow(clippy::too_many_arguments)]
fn finalize_repetition(
    repetition: u8,
    artifacts: &Path,
    usb: &oer_hil_lab::usb_events::UsbWatch,
    output: &Path,
    started_unix_millis: u64,
    started: std::time::Instant,
    cleanup: oer_hil_workload::fixture::cleanup::Scope,
    mut outcome: Outcome,
    mut failure: Option<Failure>,
    measurements: Vec<oer_hil_run_bundle_format::run::Measurement>,
) -> Result<RepetitionResult> {
    let cleanup = cleanup.finish()?;
    let cleanup_failures = cleanup
        .iter()
        .filter_map(|record| record.failure.as_deref())
        .collect::<Vec<_>>();
    apply_cleanup_failures(&mut outcome, &mut failure, &cleanup_failures);
    usb.record(output);
    let attachments = oer_hil_run_bundle_format::run::collect_attachments(output, artifacts)?;
    let result = RepetitionResult {
        schema: RUN_SCHEMA,
        repetition,
        outcome,
        started_unix_millis,
        duration_millis: oer_hil_run_bundle::run::duration_millis(started.elapsed()),
        artifact_directory: artifacts.to_owned(),
        attachments,
        measurements,
        failure,
    };
    oer_os_durable::atomic_json(&output.join("result.json"), &result)?;
    Ok(result)
}

fn apply_cleanup_failures(
    outcome: &mut Outcome,
    failure: &mut Option<Failure>,
    cleanup_failures: &[&str],
) {
    if cleanup_failures.is_empty() {
        return;
    }
    let message = format!("fixture cleanup failed: {}", cleanup_failures.join("; "));
    if let Some(failure) = failure {
        failure.message.push_str(&format!("; {message}"));
    } else {
        *outcome = Outcome::Broken;
        *failure = Some(Failure::new(FailureKind::Infrastructure, message));
    }
}

fn write_blocked_scenario(
    session: &mut RunSession,
    selected: &Scenario,
    failure: Failure,
) -> Result<ScenarioResult> {
    let output = session.scenario_directory(selected.id());
    fs::create_dir_all(&output)?;
    oer_os_durable::atomic_json(&output.join("scenario.json"), selected)?;
    let result = ScenarioResult::blocked(
        selected.id().to_owned(),
        selected.image(),
        selected.repetitions(),
        failure,
    );
    Ok(result)
}

#[cfg(test)]
mod tests;
