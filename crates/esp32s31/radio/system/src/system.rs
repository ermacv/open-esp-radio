use embassy_executor::{SpawnError, Spawner};
use oer_esp32s31_hal::root::{ConcurrentPartitions, RadioHardware};
use oer_esp32s31_phy::PhyCalibrationCache;
use oer_esp32s31_radio_esp_hal::{EspHalRadioClocks, EspHalRadioPlatform};
use oer_esp32s31_radio_runtime::RadioSystem;
use oer_time_embassy::EmbassyClock;
use static_cell::StaticCell;

/// The shared ESP32-S31 radio every protocol composition joins.
pub type SharedRadio = RadioSystem<EspHalRadioPlatform, EspHalRadioClocks, EmbassyClock>;

static RADIO: StaticCell<SharedRadio> = StaticCell::new();

/// Who runs the radio's periodic PHY tracking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tracking {
    /// A task of this crate. A tracking failure stops the program: the PHY
    /// state after a failed tick is not known, as with the vendor's assert.
    FailStop,
    /// The caller spawns its own task over the returned radio, for example
    /// one that suspends tracking during a measurement.
    Caller,
}

/// Who runs the radio's coexistence schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Schedule {
    /// A task of this crate, for the radio's lifetime, as the vendor's
    /// coexistence module runs it once `esp_coex` is up.
    Spawned,
    /// The caller runs it over the returned radio, for example only while a
    /// session takes part in coexistence
    /// (`RadioSystem::run_coex_schedule_until`). Until it does, no phase
    /// advances and no radio is notified.
    Caller,
}

/// How [`start`] brings the radio up.
pub struct RadioStart {
    tracking: Tracking,
    schedule: Schedule,
    calibration: Option<PhyCalibrationCache>,
}

impl RadioStart {
    /// Fail-stop tracking and the coexistence schedule by this crate, and a
    /// full calibration at the first PHY registration.
    pub const fn new() -> Self {
        Self {
            tracking: Tracking::FailStop,
            schedule: Schedule::Spawned,
            calibration: None,
        }
    }

    /// Replay `cache` at the first PHY registration instead of calibrating.
    pub fn with_calibration_cache(mut self, cache: PhyCalibrationCache) -> Self {
        self.calibration = Some(cache);
        self
    }

    pub const fn with_tracking(mut self, tracking: Tracking) -> Self {
        self.tracking = tracking;
        self
    }

    pub const fn with_schedule(mut self, schedule: Schedule) -> Self {
        self.schedule = schedule;
        self
    }
}

impl Default for RadioStart {
    fn default() -> Self {
        Self::new()
    }
}

/// Why [`start`] could not bring the radio up.
#[derive(Debug)]
pub enum RadioStartError {
    /// The radio hardware already has an owner: [`start`] runs once.
    AlreadyStarted,
    /// The executor has no storage left for one of the radio's tasks.
    Spawn(SpawnError),
}

impl From<SpawnError> for RadioStartError {
    fn from(error: SpawnError) -> Self {
        Self::Spawn(error)
    }
}

/// Create the shared radio over `platform`, keep it in static storage and
/// spawn, with [`Schedule::Spawned`], its coexistence schedule and, with
/// [`Tracking::FailStop`], its PHY tracking. Returns the radio and the
/// protocol partitions to hand to each composition.
///
/// # Errors
///
/// [`RadioStartError::AlreadyStarted`] on a second call, and
/// [`RadioStartError::Spawn`] when the executor cannot hold a task; the
/// radio hardware stays claimed in both cases.
pub fn start(
    spawner: Spawner,
    mut platform: EspHalRadioPlatform,
    options: RadioStart,
) -> Result<(&'static SharedRadio, ConcurrentPartitions), RadioStartError> {
    let analog_bus = platform
        .analog_bus_ownership()
        .ok_or(RadioStartError::AlreadyStarted)?;
    let hardware = RadioHardware::take(analog_bus).ok_or(RadioStartError::AlreadyStarted)?;
    let identity = platform.phy_calibration_identity();
    let (radio, partitions) = SharedRadio::new(
        hardware,
        platform,
        EspHalRadioClocks::new(),
        identity,
        EmbassyClock,
    );
    let radio = match options.calibration {
        Some(cache) => radio.with_calibration_cache(cache),
        None => radio,
    };
    let radio: &'static SharedRadio = RADIO
        .try_init(radio)
        .ok_or(RadioStartError::AlreadyStarted)?;
    if options.schedule == Schedule::Spawned {
        spawner.spawn(coexistence_schedule(radio)?);
    }
    if options.tracking == Tracking::FailStop {
        spawner.spawn(phy_tracking(radio)?);
    }
    Ok((radio, partitions))
}

#[embassy_executor::task]
async fn coexistence_schedule(radio: &'static SharedRadio) {
    radio.run_coex_schedule().await
}

#[embassy_executor::task]
async fn phy_tracking(radio: &'static SharedRadio) {
    let error = radio.run_tracking().await;
    panic!("shared PHY tracking failed: {error:?}");
}
