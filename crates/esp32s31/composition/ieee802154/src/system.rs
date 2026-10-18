//! Bring-up and teardown of the IEEE 802.15.4 client on the chip.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_esp32s31_coex::CoexError;
use oer_esp32s31_hal::{
    ieee802154::{
        Ieee802154Clocked, Ieee802154Cold, Ieee802154FoundationCheckpoint, Ieee802154Powered,
        Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
        coex::{Ieee802154CoexConfig, Ieee802154Coexistence, resolve_priorities},
        ll::Ieee802154MacOwners,
    },
    root::Ieee802154RadioPartition,
    shared_radio::{
        CoexAbsent, Coexisting, CommonPower, CommonRadioPowerError, Ieee802154Client,
        ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
};
use oer_esp32s31_ieee802154_esp_hal::{
    BoundEspHalIeee802154InterruptRoute, EspHalIeee802154InterruptRouteError, bind, random,
};
use oer_esp32s31_phy::RomShortDelay;
use oer_esp32s31_phy::{
    ConcurrentPhyTrackingError, ConcurrentRfError, ConcurrentTrackingTick, NoopPhyTargetObserver,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError},
    ieee802154_client::{
        Ieee802154PhyClientError, Ieee802154PhyMembership, RegisteredIeee802154Operational,
        RegisteredIeee802154OperationalRoute, leave_ieee802154,
    },
    maintain_concurrent_phy,
};
use oer_esp32s31_radio_runtime::{
    CoexLeaveFailure, Ieee802154JoinError, RadioPhyError, RadioSystem,
};
use oer_espressif_ieee802154_engine::{
    engine::{Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Interfaces},
    pib::Ieee802154PibDefaults,
};
use oer_espressif_ieee802154_runtime::{
    Ieee802154Random, Ieee802154Runtime, Ieee802154RuntimeBackend, Ieee802154RuntimeCaps,
    Ieee802154RuntimeParts,
};
use oer_time_embassy::EmbassyClock;

use crate::maintenance::Ieee802154PhyMaintenance;

/// Events the runtime queues before the consumer takes them.
pub const IEEE802154_EVENT_CAPACITY: usize = 16;

/// The image's monotonic clock the runtime reads: its radio clock, which a
/// synchronous reader such as OpenThread takes through
/// `IRuntime::clock`.
pub type Ieee802154SystemClock = EmbassyClock;

/// The process-wide IEEE 802.15.4 runtime.
pub type Ieee802154SystemRuntime = Ieee802154Runtime<
    'static,
    CriticalSectionRawMutex,
    Ieee802154MacOwners,
    EmbassyClock,
    HardwareRandom,
    IEEE802154_EVENT_CAPACITY,
>;

/// The running client's port backend, with the capabilities marker `K`
/// ([`Esp32s31I`](crate::Esp32s31I) or
/// [`Esp32s31IMultipan`](crate::Esp32s31IMultipan)), for
/// `run_port`.
pub type Ieee802154SystemBackend<K> = Ieee802154RuntimeBackend<
    'static,
    'static,
    K,
    CriticalSectionRawMutex,
    Ieee802154MacOwners,
    EmbassyClock,
    HardwareRandom,
    IEEE802154_EVENT_CAPACITY,
    BoundRoute,
>;

/// The bound CPU route of modem source 132, which the port's end masks
/// before it takes the engine.
pub struct BoundRoute(Option<BoundEspHalIeee802154InterruptRoute>);

impl oer_espressif_ieee802154_runtime::Ieee802154RouteDetach for BoundRoute {
    fn detach(&mut self) {
        if let Some(bound) = self.0.take()
            && bound.quiesce().is_err()
        {
            // The port's body runs on the route's core (owner decision on
            // ); another core cannot mask it.
            panic!("the IEEE 802.15.4 port's body runs off its MAC route's core");
        }
    }
}

/// The runtime's recent-RSSI reader, which outlives the installs.
pub type Ieee802154SystemRssi =
    oer_espressif_ieee802154_runtime::RuntimeRssi<'static, Ieee802154SystemRuntime>;

/// The engine and the MAC owners an ended port returns, for [`ISystem::stop`].
pub type Ieee802154SystemParts = Ieee802154RuntimeParts<'static, Ieee802154MacOwners>;

/// The radio's random words from the hardware generator.
pub struct HardwareRandom;

impl Ieee802154Random for HardwareRandom {
    fn random(&mut self) -> u32 {
        random()
    }
}

static RUNTIME: Ieee802154SystemRuntime = Ieee802154Runtime::new(EmbassyClock);

/// The interrupt-table handler of `MODEM_ZB_MAC`.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn ieee802154_interrupt() {
    RUNTIME.on_interrupt();
}

oer_memory::zeroed_static! {
    /// The engine's DMA frames. The MAC DMA reaches internal SRAM alone
    /// ([`IEEE802154_DMA_WINDOW`](oer_esp32s31_hal::i::IEEE802154_DMA_WINDOW)), so they
    /// live in the platform's DMA-visible section whatever the image's data
    /// placement; one engine takes them.
    static ENGINE_BUFFERS: oer_memory::zeroed::ZeroedStatic<Ieee802154EngineBuffers> =
        zeroed in ".dma.bss.open_radio_ieee802154_engine";
}

/// The IEEE 802.15.4 partition and MAC engine while the client is stopped.
pub struct Ieee802154Parked {
    /// The partition of the concurrent split.
    pub partition: Ieee802154RadioPartition,
    /// IEEE 802.15.4's coexistence state from the split, outside
    /// coexistence.
    pub coex: CoexAbsent<Ieee802154Client>,
    /// The MAC engine and its receive storage.
    pub engine: Ieee802154Engine<'static>,
}

impl Ieee802154Parked {
    /// Park a partition with a fresh engine over the composition's
    /// DMA-visible frames, using the ESP32-S31 BTBB transmit-power levels.
    /// `None` once an engine took the frames.
    pub fn new(
        partition: Ieee802154RadioPartition,
        coex: CoexAbsent<Ieee802154Client>,
        defaults: Ieee802154PibDefaults,
    ) -> Option<Self> {
        let buffers = ENGINE_BUFFERS.try_take()?;
        Some(Self {
            partition,
            coex,
            engine: Ieee802154Engine::new(
                buffers,
                oer_esp32s31_hal::ieee802154::ESP32S31_TX_POWER_LEVELS,
                defaults,
            ),
        })
    }

    /// Park a partition with a fresh multi-PAN engine of `interfaces`
    /// interfaces (`CONFIG_IEEE802154_MULTI_PAN_ENABLE`,
    /// `CONFIG_IEEE802154_INTERFACE_NUM`): the runtime's radio then accepts
    /// interface settings and transmissions of each interface. `None` once
    /// an engine took the composition's DMA-visible frames.
    pub fn multipan(
        partition: Ieee802154RadioPartition,
        coex: CoexAbsent<Ieee802154Client>,
        defaults: Ieee802154PibDefaults,
        interfaces: Ieee802154Interfaces,
    ) -> Option<Self> {
        let buffers = ENGINE_BUFFERS.try_take()?;
        Some(Self {
            partition,
            coex,
            engine: Ieee802154Engine::new_multipan(
                buffers,
                oer_esp32s31_hal::ieee802154::ESP32S31_TX_POWER_LEVELS,
                defaults,
                interfaces,
            ),
        })
    }
}

/// Why bring-up stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154StartError {
    /// Common radio power could not be entered.
    Power(CommonRadioPowerError),
    /// The module clocks could not be enabled.
    Clocks(ModemClockError),
    /// The shared PHY domain could not be registered or woken.
    Phy(RadioPhyError),
    /// IEEE 802.15.4 could not join the shared PHY domain or BTBB.
    PhyClient(Ieee802154PhyClientError),
    /// Tracking due at the join was rejected before it started.
    Tracking(ConcurrentPhyError),
    /// Tracking due at the join failed after it started.
    TrackingFailed,
    /// A MAC reset line did not read back released.
    Reset(Ieee802154ReadbackError<Ieee802154ResetCheckpoint>),
    /// A MAC foundation field did not read back.
    Foundation(Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>),
    /// A runtime is already installed.
    AlreadyInstalled,
    /// The CPU route of modem source 132 could not be bound.
    Route(EspHalIeee802154InterruptRouteError),
    /// The engine's DMA frames lie outside the memory the MAC DMA reaches.
    BuffersNotDmaVisible,
}

/// Owners retained after a failure that may have left hardware in an
/// unknown state. They are never returned; the chip must be reset.
#[must_use = "a fail-stop owner keeps the radio until the chip resets"]
pub struct Ieee802154FailStop {
    _owner: FailStopOwner,
    _power: CommonPower<Ieee802154Client>,
    _coex: CoexAbsent<Ieee802154Client>,
}

/// IEEE 802.15.4's part in coexistence with Wi-Fi, which it takes on
/// request and must leave before it leaves common power.
enum Ieee802154Coex {
    Absent(CoexAbsent<Ieee802154Client>),
    Coexisting(Coexisting<Ieee802154Client>),
}

#[allow(
    dead_code,
    clippy::large_enum_variant,
    reason = "the retained owners are never read or moved again"
)]
enum FailStopOwner {
    Powered(Ieee802154Powered),
    Clocked(Ieee802154Clocked),
    Joined(Ieee802154Clocked, Ieee802154PhyMembership),
    Installed(RegisteredIeee802154Operational, Ieee802154Engine<'static>),
}

/// Failed bring-up.
#[must_use = "a failed start still owns the partition or keeps it fail-stop"]
pub struct Ieee802154StartFailure {
    /// The first error.
    pub error: Ieee802154StartError,
    /// The stopped client when every earlier step was rolled back, or the
    /// fail-stop owner otherwise.
    pub owner: Result<Ieee802154Parked, Ieee802154FailStop>,
}

/// Why teardown stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154StopError {
    /// The MAC foundation did not read back after the operational epoch.
    Foundation(Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>),
    /// IEEE 802.15.4 could not leave the shared PHY domain or BTBB.
    PhyClient(Ieee802154PhyClientError),
    /// Shared RF could not be closed after the last PHY client left.
    PhyClose(ConcurrentRfError),
    /// The module clocks could not be released.
    Clocks(ModemClockError),
    /// Common radio power could not be left.
    Power(CommonRadioPowerError),
    /// IEEE 802.15.4 could not leave coexistence with Wi-Fi: a request
    /// timer could not be withdrawn.
    Coexistence(CoexError),
}

/// Failed teardown.
#[must_use = "a failed stop still owns the system or keeps it fail-stop"]
pub struct Ieee802154StopFailure {
    /// The first error.
    pub error: Ieee802154StopError,
    /// The unchanged running system and the ended port's parts when
    /// nothing was torn down yet, or the fail-stop owner otherwise.
    pub owner: Result<(Ieee802154System, Ieee802154SystemParts), Ieee802154FailStop>,
}

/// The running IEEE 802.15.4 client.
///
/// The MAC owners and the engine live in the runtime; this value keeps the
/// PHY membership until [`Self::stop`]. The radio's backend, which holds the
/// bound CPU route, goes to `run_port`, whose end masks the route and
/// returns the engine and the MAC owners to [`Self::stop`].
#[must_use = "the running IEEE 802.15.4 client must be stopped"]
// CAPABILITY: i-mac-operation-subset
pub struct Ieee802154System {
    route: RegisteredIeee802154OperationalRoute,
    /// The scene levels the MAC's priorities were resolved with
    /// (`s_coex_config`).
    coex_config: Ieee802154CoexConfig,
    /// The common power proof.
    power: CommonPower<Ieee802154Client>,
    /// IEEE 802.15.4's part in coexistence with Wi-Fi.
    coex: Ieee802154Coex,
}

/// Why PHY maintenance failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154MaintenanceError {
    /// The PHY domain rejected the evaluation or the maintenance before it
    /// started.
    Phy(ConcurrentPhyError),
    /// Tracking failed after it started; the domain is poisoned.
    TrackingFailed,
}

fn fail_stop(
    error: Ieee802154StartError,
    owner: FailStopOwner,
    power: CommonPower<Ieee802154Client>,
    coex: CoexAbsent<Ieee802154Client>,
) -> Ieee802154StartFailure {
    Ieee802154StartFailure {
        error,
        owner: Err(Ieee802154FailStop {
            _owner: owner,
            _power: power,
            _coex: coex,
        }),
    }
}

/// Roll a clocked client back to its partition.
fn unwind_clocked(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: Ieee802154Clocked,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
    power: CommonPower<Ieee802154Client>,
    coex: CoexAbsent<Ieee802154Client>,
) -> Ieee802154StartFailure {
    let powered = match clocked.disable_clocks(lease) {
        Ok(powered) => powered,
        Err(failure) => {
            return fail_stop(
                error,
                FailStopOwner::Clocked(failure.into_owner()),
                power,
                coex,
            );
        }
    };
    unwind_powered(lease, powered, engine, error, power, coex)
}

fn unwind_powered(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    powered: Ieee802154Powered,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
    power: CommonPower<Ieee802154Client>,
    coex: CoexAbsent<Ieee802154Client>,
) -> Ieee802154StartFailure {
    match powered.power_down(lease, power, &coex) {
        Ok(cold) => Ieee802154StartFailure {
            error,
            owner: Ok(Ieee802154Parked {
                partition: cold.into_partition(),
                coex,
                engine,
            }),
        },
        Err(failure) => {
            let (powered, power) = failure.into_owner();
            fail_stop(error, FailStopOwner::Powered(powered), power, coex)
        }
    }
}

/// `esp_i_enable` on the shared radio.
///
/// `radio` supplies the arbiter with the platform token and clock sources,
/// and registers or wakes the shared PHY domain for this client.
///
/// # Errors
///
/// A step failed. The failure returns the parked client when every earlier
/// step was rolled back, or a fail-stop owner when a started PHY, clock or
/// power transaction failed.
///
/// `K` names the parked engine's interfaces: one for
/// [`IParked::new`], four for [`IParked::multipan`] of
/// four.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure returns the parked partition and engine"
)]
// CAPABILITY: i-phy-and-rf-2-4-ghz-o-qpsk-250-kbit-s, i-security-power-and-coexistence-powered-lifecycle
pub async fn start<K: Ieee802154RuntimeCaps, P, C: PlatformClockProvider>(
    radio: &RadioSystem<P, C, EmbassyClock>,
    parked: Ieee802154Parked,
    defaults: Ieee802154PibDefaults,
) -> Result<(Ieee802154System, Ieee802154SystemBackend<K>), Ieee802154StartFailure> {
    // Frames the MAC DMA cannot reach leave the radio silent on air; refuse
    // before any hardware changes.
    if !parked
        .engine
        .buffers_dma_visible(&oer_esp32s31_hal::ieee802154::IEEE802154_DMA_WINDOW)
    {
        return Err(Ieee802154StartFailure {
            error: Ieee802154StartError::BuffersNotDmaVisible,
            owner: Ok(parked),
        });
    }
    let Ieee802154Parked {
        partition,
        coex,
        mut engine,
    } = parked;
    let mut guard = radio.lock().await;
    let (lease, _, clocks) = guard.parts();

    let (powered, power) = match Ieee802154Cold::from_partition(partition).power_up(lease, clocks) {
        Ok(powered) => powered,
        Err(failure) => {
            return Err(Ieee802154StartFailure {
                error: Ieee802154StartError::Power(failure.error()),
                owner: Ok(Ieee802154Parked {
                    partition: failure.into_owner().into_partition(),
                    coex,
                    engine,
                }),
            });
        }
    };
    let clocked = match powered.enable_clocks(lease, clocks) {
        Ok(clocked) => clocked,
        Err(failure) => {
            let error = Ieee802154StartError::Clocks(failure.error());
            if failure.error() == ModemClockError::Poisoned {
                return Err(fail_stop(
                    error,
                    FailStopOwner::Powered(failure.into_owner()),
                    power,
                    coex,
                ));
            }
            return Err(unwind_powered(
                lease,
                failure.into_owner(),
                engine,
                error,
                power,
                coex,
            ));
        }
    };

    let joined = {
        let joined = core::pin::pin!(guard.join_ieee802154(&clocked));
        joined.await
    };
    let (membership, acquired) = match joined {
        Ok(joined) => joined,
        Err(Ieee802154JoinError::Phy(error)) if error.started() => {
            return Err(fail_stop(
                Ieee802154StartError::Phy(error),
                FailStopOwner::Clocked(clocked),
                power,
                coex,
            ));
        }
        Err(error) => {
            let error = match error {
                Ieee802154JoinError::Phy(error) => Ieee802154StartError::Phy(error),
                Ieee802154JoinError::Client(error) => Ieee802154StartError::PhyClient(error),
            };
            let (lease, _, _) = guard.parts();
            return Err(unwind_clocked(lease, clocked, engine, error, power, coex));
        }
    };
    let (lease, platform, _) = guard.parts();
    if acquired == ConcurrentAcquire::TrackingDue {
        let tracked = maintain_concurrent_phy::<P, RomShortDelay, _>(
            &EmbassyClock,
            lease,
            platform,
            NoopPhyTargetObserver,
        )
        .await;
        match tracked {
            Ok(_outcome) => {}
            Err(ConcurrentPhyTrackingError::Rejected(error)) => {
                // Nothing started, but the domain still waits for tracking,
                // so the client cannot leave it.
                return Err(fail_stop(
                    Ieee802154StartError::Tracking(error),
                    FailStopOwner::Joined(clocked, membership),
                    power,
                    coex,
                ));
            }
            Err(ConcurrentPhyTrackingError::Failed(_)) => {
                return Err(fail_stop(
                    Ieee802154StartError::TrackingFailed,
                    FailStopOwner::Joined(clocked, membership),
                    power,
                    coex,
                ));
            }
        }
    }

    let reset = match clocked.reset_mac(lease) {
        Ok(reset) => reset,
        Err(failure) => {
            let error = Ieee802154StartError::Reset(failure.error());
            return Err(leave_and_unwind(
                lease,
                failure.into_clocked(),
                membership,
                engine,
                error,
                power,
                coex,
            ));
        }
    };
    let foundation = match reset.configure_foundation() {
        Ok(foundation) => foundation,
        Err(failure) => {
            let error = Ieee802154StartError::Foundation(failure.error());
            return Err(leave_and_unwind(
                lease,
                failure.into_reset().into_clocked(),
                membership,
                engine,
                error,
                power,
                coex,
            ));
        }
    };
    engine.set_coexistence(coexistence(guard.lease(), Ieee802154CoexConfig::VENDOR));
    drop(guard);

    let RegisteredIeee802154Operational {
        mut task,
        interrupts,
        route,
    } = RegisteredIeee802154Operational::new(foundation, membership);
    let interrupts = interrupts.activate(&mut task);
    let parts = Ieee802154RuntimeParts {
        engine,
        hardware: Ieee802154MacOwners::new(task, interrupts),
    };
    let backend = match RUNTIME.install::<K>(parts, HardwareRandom, defaults) {
        Ok(handles) => handles,
        Err(parts) => {
            let (mut task, interrupts) = parts.hardware.into_parts();
            let interrupts = interrupts.deactivate(&mut task);
            return Err(fail_stop(
                Ieee802154StartError::AlreadyInstalled,
                FailStopOwner::Installed(
                    RegisteredIeee802154Operational {
                        task,
                        interrupts,
                        route,
                    },
                    parts.engine,
                ),
                power,
                coex,
            ));
        }
    };
    match bind() {
        Ok(bound) => Ok((
            Ieee802154System {
                route,
                coex_config: Ieee802154CoexConfig::VENDOR,
                power,
                coex: Ieee802154Coex::Absent(coex),
            },
            backend.with_route(BoundRoute(Some(bound))),
        )),
        Err(error) => {
            let (parts, _) = backend.uninstall_unused();
            let (mut task, interrupts) = parts.hardware.into_parts();
            let interrupts = interrupts.deactivate(&mut task);
            Err(fail_stop(
                Ieee802154StartError::Route(error),
                FailStopOwner::Installed(
                    RegisteredIeee802154Operational {
                        task,
                        interrupts,
                        route,
                    },
                    parts.engine,
                ),
                power,
                coex,
            ))
        }
    }
}

fn leave_and_unwind(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: Ieee802154Clocked,
    membership: Ieee802154PhyMembership,
    engine: Ieee802154Engine<'static>,
    error: Ieee802154StartError,
    power: CommonPower<Ieee802154Client>,
    coex: CoexAbsent<Ieee802154Client>,
) -> Ieee802154StartFailure {
    if leave_ieee802154(lease, &clocked, membership).is_err() {
        return fail_stop(error, FailStopOwner::Clocked(clocked), power, coex);
    }
    unwind_clocked(lease, clocked, engine, error, power, coex)
}

/// The shared radio is a software-coexistence build: the MAC publishes the
/// priorities of `config` read from the arbiter's current table.
fn coexistence(
    lease: &SharedRadioLease<'_, ConcurrentPhy>,
    config: Ieee802154CoexConfig,
) -> Ieee802154Coexistence {
    Ieee802154Coexistence::Software(resolve_priorities(config, &lease.coex_pti_table()))
}

impl Ieee802154System {
    /// The process-wide runtime, for its clock ([`IRuntime::clock`])
    /// and its statistics. Commands and events go through the port of the
    /// backend [`start`] returned; its driver runs the CSMA-CA backoffs.
    pub fn runtime(&self) -> &'static Ieee802154SystemRuntime {
        &RUNTIME
    }

    /// Read the arbiter's coexistence table again and publish the scene
    /// levels of `config` (`esp_i_set_coex_config`). Call it after
    /// the table or the levels change; the TX/RX priority applies from the
    /// next operation start, the ACK priority from the next start of the
    /// client, as in the vendor driver.
    pub async fn update_coexistence<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C, EmbassyClock>,
        config: Ieee802154CoexConfig,
    ) {
        let mut guard = radio.lock().await;
        RUNTIME.set_coexistence(coexistence(guard.lease(), config));
        self.coex_config = config;
    }

    /// The MAC's power-sequencing words, read under the runtime's lock.
    pub fn power_sequence(&self) -> oer_esp32s31_hal::ieee802154::Ieee802154PowerSequence {
        RUNTIME
            .with_hardware(Ieee802154MacOwners::power_sequence)
            .expect("a running client's radio is installed")
    }

    /// Take part in coexistence with Wi-Fi, as ESP-IDF's
    /// `esp_coex_wifi_i154_enable` does for a Thread border router: enable
    /// coexistence for IEEE 802.15.4 and publish its schedule status, so the
    /// coexistence schedule counts it (`RadioGuard::enable_i_coex`).
    /// The image must run `RadioSystem::run_coex_schedule` once, as its
    /// Wi-Fi composition does. Nothing happens when it already takes part.
    pub async fn enable_wifi_coexistence<P, C: PlatformClockProvider>(
        self,
        radio: &RadioSystem<P, C, EmbassyClock>,
    ) -> Self {
        let Self {
            route,
            coex_config,
            power,
            coex,
        } = self;
        let coex = match coex {
            Ieee802154Coex::Absent(absent) => {
                Ieee802154Coex::Coexisting(radio.lock().await.enable_ieee802154_coex(absent))
            }
            coexisting @ Ieee802154Coex::Coexisting(_) => coexisting,
        };
        Self {
            route,
            coex_config,
            power,
            coex,
        }
    }

    /// Leave coexistence with Wi-Fi: withdraw the schedule status and
    /// disable coexistence for IEEE 802.15.4
    /// (`RadioGuard::disable_i_coex`). Nothing happens when it does
    /// not take part; [`Self::stop`] leaves it itself.
    ///
    /// # Errors
    ///
    /// A coexistence timer could not be disabled; the core keeps the timer
    /// as uncertain and the returned system still takes part, so a retry
    /// withdraws the requests again.
    #[allow(
        clippy::result_large_err,
        reason = "the failure returns the running system"
    )]
    pub async fn disable_wifi_coexistence<P, C: PlatformClockProvider>(
        self,
        radio: &RadioSystem<P, C, EmbassyClock>,
    ) -> Result<Self, (Self, CoexError)> {
        let Self {
            route,
            coex_config,
            power,
            coex,
        } = self;
        let (coex, error) = match leave_coexistence(coex, radio).await {
            Ok(absent) => (Ieee802154Coex::Absent(absent), None),
            Err(CoexLeaveFailure { proof, error }) => {
                (Ieee802154Coex::Coexisting(proof), Some(error))
            }
        };
        let system = Self {
            route,
            coex_config,
            power,
            coex,
        };
        match error {
            None => Ok(system),
            Some(error) => Err((system, error)),
        }
    }

    /// Whether IEEE 802.15.4 takes part in coexistence with Wi-Fi.
    pub const fn wifi_coexistence(&self) -> bool {
        matches!(self.coex, Ieee802154Coex::Coexisting(_))
    }

    /// The scene levels the MAC's priorities were last resolved with
    /// (`esp_i_get_coex_config`): the vendor default after start.
    pub const fn coex_config(&self) -> Ieee802154CoexConfig {
        self.coex_config
    }

    /// Run one shared PHY tracking tick, as one tick of ESP-IDF's periodic
    /// `phy_track_pll`: it runs with IEEE 802.15.4 receiving, as the vendor
    /// timer does, and never pauses the MAC; an operation that loses RF ends
    /// with its ordinary terminal event. The radio's
    /// [`RadioSystem::run_tracking`] is the periodic loop of these ticks.
    ///
    /// # Errors
    ///
    /// The domain rejected the evaluation or the maintenance, or tracking
    /// failed after it started.
    pub async fn maintain_phy<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C, EmbassyClock>,
    ) -> Result<Ieee802154PhyMaintenance, Ieee802154MaintenanceError> {
        let mut guard = radio.lock().await;
        match guard.track().await {
            Ok(ConcurrentTrackingTick::NotDue) => Ok(Ieee802154PhyMaintenance::NotDue),
            Ok(ConcurrentTrackingTick::Tracked(_outcome)) => Ok(Ieee802154PhyMaintenance::Tracked),
            Ok(ConcurrentTrackingTick::Unavailable(error)) => {
                Err(Ieee802154MaintenanceError::Phy(error))
            }
            Err(ConcurrentPhyTrackingError::Rejected(error)) => {
                Err(Ieee802154MaintenanceError::Phy(error))
            }
            Err(ConcurrentPhyTrackingError::Failed(_)) => {
                Err(Ieee802154MaintenanceError::TrackingFailed)
            }
        }
    }

    /// `esp_i_disable` with the parts the ended port returned (its
    /// end masked the CPU route and took the MAC owners out of the runtime):
    /// leave coexistence with Wi-Fi when IEEE 802.15.4 takes part, prove the
    /// foundation again, leave the shared PHY and BTBB
    /// (closing RF after the last PHY client), release the module clocks and
    /// leave common power. Must run on the core that started the client.
    ///
    /// # Errors
    ///
    /// A step failed. A coexistence timer that could not be withdrawn
    /// returns the system, still taking part
    /// ([`IStopError::Coexistence`]), with the parts. Any later
    /// failure returns a fail-stop owner.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure returns the running system"
    )]
    pub async fn stop<P, C: PlatformClockProvider>(
        self,
        parts: Ieee802154SystemParts,
        radio: &RadioSystem<P, C, EmbassyClock>,
    ) -> Result<Ieee802154Parked, Ieee802154StopFailure> {
        let Self {
            route,
            coex_config,
            power,
            coex,
        } = self;
        // Coexistence ends before the client leaves the PHY and power.
        let coex = match leave_coexistence(coex, radio).await {
            Ok(absent) => absent,
            Err(CoexLeaveFailure { proof, error }) => {
                return Err(Ieee802154StopFailure {
                    error: Ieee802154StopError::Coexistence(error),
                    owner: Ok((
                        Self {
                            route,
                            coex_config,
                            power,
                            coex: Ieee802154Coex::Coexisting(proof),
                        },
                        parts,
                    )),
                });
            }
        };
        // The port's end masked the CPU route before it took the engine
        // (`IRouteDetach`).
        let engine = parts.engine;
        let (mut task, interrupts) = parts.hardware.into_parts();
        let interrupts = interrupts.deactivate(&mut task);
        let (foundation, membership) = match route.into_foundation(task, interrupts) {
            Ok(parts) => parts,
            Err(failure) => {
                let error = Ieee802154StopError::Foundation(failure.failure().error());
                let (failure, membership) = failure.into_parts();
                return Err(stop_fail_stop(
                    error,
                    FailStopOwner::Joined(failure.into_reset().into_clocked(), membership),
                    power,
                    coex,
                ));
            }
        };
        let clocked = foundation.into_clocked();
        let mut guard = radio.lock().await;
        let left = {
            let left = core::pin::pin!(guard.leave_ieee802154(&clocked, membership));
            left.await
        };
        let left = match left {
            Ok(left) => left,
            Err(failure) => {
                let error = Ieee802154StopError::PhyClient(failure.error());
                return Err(stop_fail_stop(
                    error,
                    FailStopOwner::Joined(clocked, failure.into_membership()),
                    power,
                    coex,
                ));
            }
        };
        // RF closed after the last client; it was already closed, or stays
        // open for another client, otherwise.
        if let Err(error) = left.rf_closed {
            return Err(stop_fail_stop(
                Ieee802154StopError::PhyClose(error),
                FailStopOwner::Clocked(clocked),
                power,
                coex,
            ));
        }
        let (lease, _, _) = guard.parts();
        let powered = match clocked.disable_clocks(lease) {
            Ok(powered) => powered,
            Err(failure) => {
                return Err(stop_fail_stop(
                    Ieee802154StopError::Clocks(failure.error()),
                    FailStopOwner::Clocked(failure.into_owner()),
                    power,
                    coex,
                ));
            }
        };
        match powered.power_down(lease, power, &coex) {
            Ok(cold) => Ok(Ieee802154Parked {
                partition: cold.into_partition(),
                coex,
                engine,
            }),
            Err(failure) => {
                let error = Ieee802154StopError::Power(failure.error());
                let (powered, power) = failure.into_owner();
                Err(stop_fail_stop(
                    error,
                    FailStopOwner::Powered(powered),
                    power,
                    coex,
                ))
            }
        }
    }
}

/// Leave coexistence with Wi-Fi when IEEE 802.15.4 takes part.
async fn leave_coexistence<P, C: PlatformClockProvider>(
    coex: Ieee802154Coex,
    radio: &RadioSystem<P, C, EmbassyClock>,
) -> Result<CoexAbsent<Ieee802154Client>, CoexLeaveFailure<Coexisting<Ieee802154Client>, CoexError>>
{
    match coex {
        Ieee802154Coex::Absent(absent) => Ok(absent),
        Ieee802154Coex::Coexisting(coexisting) => {
            radio.lock().await.disable_ieee802154_coex(coexisting)
        }
    }
}

fn stop_fail_stop(
    error: Ieee802154StopError,
    owner: FailStopOwner,
    power: CommonPower<Ieee802154Client>,
    coex: CoexAbsent<Ieee802154Client>,
) -> Ieee802154StopFailure {
    Ieee802154StopFailure {
        error,
        owner: Err(Ieee802154FailStop {
            _owner: owner,
            _power: power,
            _coex: coex,
        }),
    }
}
