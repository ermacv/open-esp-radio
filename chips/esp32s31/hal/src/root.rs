//! The reset root: [`Root::take`] claims the chip once
//! ([`oer_esp32s31_pac::take`]) and splits its partitions by domain, for
//! the bootstrap image ([`Root::into_bootstrap`]) or for stage two
//! ([`Root::into_runtime`]). Each image has its own root; stage two adopts
//! the platform the bootstrap initialized without repeating cold boot.
//!
//! The radio's partitions stay together in [`RadioHardware`], the
//! protocol-neutral radio root: [`RadioHardware::split`] hands every
//! protocol partition out at once and places the shared partitions in one
//! [`RadioDomain`]; [`RadioHardware::reunite`] reunites them only after the
//! shared restore obligations are complete.
//!
//! Only one root exists per image:
//!
//! ```
//! use oer_esp32s31_hal::root::Root;
//! # fn image() {
//! let first = Root::take();
//! assert!(Root::take().is_none(), "the chip is claimed once");
//! # drop(first);
//! # }
//! ```

use oer_esp32s31_pac::ownership::{
    BootPeripherals, MacLocalTimePeripherals, ModemEtmPeripherals, Partitions, SocPeripherals,
};

use crate::{
    bluetooth::{BluetoothControllerParts, BluetoothModemLpTimerParts, BluetoothPartition},
    boot::{
        ActivePower, Brownout, CacheRegisters, ClockRegisters, FLASH_MMU_WINDOW, FlashRegisters,
        MmuWindow, PsramRegisters, Soc,
    },
    cores,
    domain::{
        BluetoothClient, CoexAbsent, Ieee802154Client, RadioDomain, RadioDomainReleaseError,
        WifiClient,
    },
    ieee80211::registers::{MacLocalTime, WifiMacPartition, WifiRxDmaRegisters},
    ieee802154::mac::Ieee802154Partition,
    modem::SharedRadioRegisters,
    owner::WifiInterruptBanks,
    phy::{RadioPhyRegisters, registration::PhyRegistration, restore::PhyRouteState},
    platform::{
        EfuseRegisters, EntropyRegisters, InterruptBlocks, ResetCauseRegisters, SoftwareInterrupts,
        SystemTimer,
        core_control::{CoreControlRegisters, Uart0ClockRegisters},
        system_timer::{Monotonic, MonotonicStuck, open_clock},
        timg0_alarm::Timg0AlarmParts,
    },
    power::{ChipClocks, PlatformClockBackend, PsramMpll},
    usb_serial_jtag::{self, ConsoleRegisters},
    watchdog::{Mwdt0Registers, RwdtRegisters, SwdRegisters, WatchdogRegisters, Watchdogs},
};

/// The chip's reset root: every claimed part, once.
#[must_use = "dropping the root permanently loses every register authority"]
pub struct Root {
    partitions: Partitions,
}

/// Why the reset root could not publish its image's platform owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitError {
    /// The console's parts were installed before.
    ConsoleInstalled,
    /// The system timer's counter latch never settled during bootstrap.
    MonotonicStuck,
    /// Stage two's core-control words were installed before.
    CoreControlInstalled,
}

impl InitError {
    /// The stable reason used by an image's boot-failure record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ConsoleInstalled => "console-installed",
            Self::MonotonicStuck => "monotonic",
            Self::CoreControlInstalled => "core-control-installed",
        }
    }
}

impl From<usb_serial_jtag::Installed> for InitError {
    fn from(_: usb_serial_jtag::Installed) -> Self {
        Self::ConsoleInstalled
    }
}

impl From<MonotonicStuck> for InitError {
    fn from(_: MonotonicStuck) -> Self {
        Self::MonotonicStuck
    }
}

impl Root {
    /// The root, the first time it is called in an image; `None` after.
    pub fn take() -> Option<Self> {
        oer_esp32s31_pac::take().map(|partitions| Self { partitions })
    }

    /// Split the root for the bootstrap image: the console's parts go to the
    /// early writer, the system timer's clock opens for the boot's
    /// monotonic clock, and the bootstrap's owners are returned. The other
    /// partitions are dropped: the bootstrap writes no other word.
    ///
    /// # Errors
    ///
    /// The console was installed before, or the system timer's counter
    /// latch never settled: no boot wait can be bounded.
    pub fn into_bootstrap(self) -> Result<Bootstrap, InitError> {
        let Partitions {
            usb_serial_jtag,
            core_control,
            watchdog,
            system_timer,
            radio_phy,
            boot,
            soc,
            analog_bus,
            ..
        } = self.partitions;
        usb_serial_jtag::install(ConsoleRegisters::new(usb_serial_jtag))?;
        let SocPeripherals {
            lp_apm_filter,
            hp_apm_filter,
            hp_mem_apm_filter,
            trng_init,
            lp_periclkrst_rng_init,
            mut hp_sys_clkrst_systimer,
        } = soc;
        open_clock(&mut hp_sys_clkrst_systimer);
        let SystemTimer { counter, .. } = SystemTimer::new(system_timer);
        let monotonic = Monotonic::new(counter)?;
        let BootPeripherals {
            hp_sys_clkrst_boot_clock,
            lp_aon_clkrst_cpll,
            pmu_boot_clock,
            hp_alive_sys_pll_clock,
            cache_boot,
            spi0_flash,
            spi1_flash,
            memspi2_psram,
            memspi3_psram,
            iomux_mspi_psram_pins,
            lp_ana_brownout,
        } = boot;
        let WatchdogRegisters {
            main0,
            rtc,
            super_watchdog,
            ..
        } = WatchdogRegisters::new(watchdog);
        Ok(Bootstrap {
            monotonic,
            soc: Soc::new(
                lp_apm_filter,
                hp_apm_filter,
                hp_mem_apm_filter,
                trng_init,
                lp_periclkrst_rng_init,
                CoreControlRegisters::new(core_control),
            ),
            brownout: Brownout::new(lp_ana_brownout, analog_bus),
            main_watchdog0: main0,
            rtc_watchdog: rtc,
            super_watchdog,
            clock: ClockRegisters::new(
                hp_sys_clkrst_boot_clock,
                lp_aon_clkrst_cpll,
                pmu_boot_clock,
                hp_alive_sys_pll_clock,
            ),
            active_power: ActivePower::new(radio_phy.pmu_radio),
            psram: PsramRegisters::new(memspi2_psram, memspi3_psram, iomux_mspi_psram_pins),
            cache: CacheRegisters::new(cache_boot),
            flash: FlashRegisters::new(spi0_flash, spi1_flash),
            flash_window: FLASH_MMU_WINDOW,
        })
    }

    /// Split the root for stage two, which adopts the platform the
    /// bootstrap initialized: the console's parts go to the early writer,
    /// the core-control words to the cores' owner and the fatal paths,
    /// hart 0's pending sources and debug status to the executor's
    /// platform, and the platform's and radio's owners are returned. The
    /// bootstrap's partitions are dropped: stage two repeats no cold boot.
    ///
    /// The platform clock words go to the one clock owner
    /// ([`ChipClocks`]), which opens the Wi-Fi power gate and adopts the
    /// PSRAM mapping's MPLL reference.
    ///
    /// Taking the watchdogs arms the RTC watchdog
    /// ([`Watchdogs::new`]): from here the image's executor must feed it
    /// ([`ArmedRtcWatchdog::feed_every`](crate::watchdog::ArmedRtcWatchdog::feed_every)),
    /// and a fatal path that cannot prepare the reset waits for it.
    ///
    /// # Errors
    ///
    /// The console or core-control words were installed before.
    pub fn into_runtime(self) -> Result<Runtime, InitError> {
        let Partitions {
            usb_serial_jtag,
            core_control,
            uart0_clock,
            efuse,
            entropy,
            reset_cause,
            watchdog,
            system_timer,
            software_interrupt_0,
            software_interrupt_1,
            software_interrupt_2,
            software_interrupt_3,
            interrupt_matrix,
            wifi_mac,
            wifi_mac_interface,
            wifi_rx_dma,
            wifi_interrupts,
            mac_local_time,
            // No production PMU wake observer is installed.
            pmu_wake: _,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            modem_etm,
            ieee802154,
            boot,
            soc,
            analog_bus,
            debug,
            chip_clocks,
            timer_group0,
        } = self.partitions;
        #[cfg(target_arch = "riscv32")]
        ClockRegisters::new(
            boot.hp_sys_clkrst_boot_clock,
            boot.lp_aon_clkrst_cpll,
            boot.pmu_boot_clock,
            boot.hp_alive_sys_pll_clock,
        )
        .adopt_short_settle_clock();
        #[cfg(not(target_arch = "riscv32"))]
        let _ = boot;
        usb_serial_jtag::install(ConsoleRegisters::new(usb_serial_jtag))?;
        // Stage two runs from the PSRAM the bootstrap mapped over MPLL.
        let clocks = ChipClocks::install(chip_clocks, PsramMpll::adopted());
        cores::install(
            CoreControlRegisters::new(core_control),
            Uart0ClockRegisters::new(uart0_clock),
        )
        .map_err(|_| InitError::CoreControlInstalled)?;
        crate::executor::publish_debug_status(debug);
        Ok(Runtime {
            watchdogs: Watchdogs::new(WatchdogRegisters::new(watchdog)),
            system_timer: SystemTimer::new(system_timer),
            timg0_alarm: Timg0AlarmParts::new(timer_group0),
            software_interrupts: SoftwareInterrupts::new(
                software_interrupt_0,
                software_interrupt_1,
                software_interrupt_2,
                software_interrupt_3,
            ),
            interrupts: InterruptBlocks::new(interrupt_matrix),
            efuse: EfuseRegisters::new(efuse),
            entropy: EntropyRegisters::new(entropy, soc.lp_periclkrst_rng_init),
            reset_cause: ResetCauseRegisters::new(reset_cause),
            wifi_interrupts: WifiInterruptBanks::new(wifi_interrupts),
            clocks,
            radio: RadioHardware::new(RadioPartitions::compose(RadioParts {
                wifi_mac,
                wifi_mac_interface,
                wifi_rx_dma,
                mac_local_time,
                radio_phy,
                coexistence,
                bluetooth,
                bluetooth_modem_lp_timer,
                bluetooth_interrupts,
                shared_radio,
                modem_etm,
                ieee802154,
                analog_bus,
            })),
        })
    }
}

/// The bootstrap's owners: the boot's monotonic clock, the SoC initializer,
/// the brownout detector, the three watchdogs the bootstrap takes over, the
/// clock, PLL power, PSRAM, cache and flash owners, and the MMU windows.
#[must_use = "the bootstrap's owners bring the chip up"]
pub struct Bootstrap {
    /// The boot's monotonic clock, counting before the boot deadline starts.
    pub monotonic: Monotonic,
    /// The SoC initialization.
    pub soc: Soc,
    /// The brownout detector and the analog bus of its threshold.
    pub brownout: Brownout,
    /// Timer group 0's main watchdog.
    pub main_watchdog0: Mwdt0Registers,
    /// The RTC watchdog.
    pub rtc_watchdog: RwdtRegisters,
    /// The super watchdog.
    pub super_watchdog: SwdRegisters,
    /// The CPU clock, its PLL and update, the flash clock, the memory PLL
    /// and the PSRAM supply.
    pub clock: ClockRegisters,
    /// The PLLs' active-state power.
    pub active_power: ActivePower,
    /// The PSRAM controllers, command port, pads and MMU.
    pub psram: PsramRegisters,
    /// The L1 cache's fetch suspension and sync engine.
    pub cache: CacheRegisters,
    /// The flash command port, timing and MMU.
    pub flash: FlashRegisters,
    /// The flash MMU's window.
    pub flash_window: MmuWindow,
}

/// Stage two's owners of the platform and the radio.
#[must_use = "the runtime's owners drive the platform and the radio"]
pub struct Runtime {
    /// Every watchdog, the RTC watchdog armed: the image's executor feeds
    /// it.
    pub watchdogs: Watchdogs,
    /// The system timer's comparators, counter and alarm parts.
    pub system_timer: SystemTimer,
    /// TIMG0 T0 program and permanent IRQ owners, adopted without writes.
    pub timg0_alarm: Timg0AlarmParts,
    /// The four software interrupt lines and their interrupt parts.
    pub software_interrupts: SoftwareInterrupts,
    /// Hart 0's interrupt controller and hart 1's bank.
    pub interrupts: InterruptBlocks,
    /// The factory MAC words.
    pub efuse: EfuseRegisters,
    /// The TRNG's output and status.
    pub entropy: EntropyRegisters,
    /// The HP cores' reset causes.
    pub reset_cause: ResetCauseRegisters,
    /// The Wi-Fi MAC's interrupt banks, which the image's interrupt table
    /// binds to the radio interrupt handler once.
    pub wifi_interrupts: WifiInterruptBanks,
    /// The platform clock owner: the provider and backend of the radio's
    /// clock references ([`RadioHardware::split`]`::<ChipClocks, _>`).
    pub clocks: ChipClocks,
    /// The protocol-neutral radio root.
    pub radio: RadioHardware,
}

/// The radio's partitions of the root, as `take()` hands them out.
struct RadioParts {
    wifi_mac: oer_esp32s31_pac::ownership::WifiMacPeripherals,
    wifi_mac_interface: oer_esp32s31_pac::ownership::WifiMacInterfacePeripherals,
    wifi_rx_dma: oer_esp32s31_pac::ownership::WifiRxDmaPeripherals,
    mac_local_time: MacLocalTimePeripherals,
    radio_phy: oer_esp32s31_pac::ownership::RadioPhyPeripherals,
    coexistence: oer_esp32s31_pac::ownership::CoexistencePeripherals,
    bluetooth: oer_esp32s31_pac::ownership::BluetoothControllerPeripherals,
    bluetooth_modem_lp_timer: oer_esp32s31_pac::ownership::BluetoothModemLpTimerPeripherals,
    bluetooth_interrupts: oer_esp32s31_pac::ownership::BluetoothInterruptPeripherals,
    shared_radio: oer_esp32s31_pac::ownership::SharedRadioPeripherals,
    modem_etm: ModemEtmPeripherals,
    ieee802154: oer_esp32s31_pac::ownership::Ieee802154Peripherals,
    analog_bus: oer_esp32s31_pac::ownership::AnalogBusPeripherals,
}

/// The radio's owners: the shared radio owner and the protocol partitions.
pub(crate) struct RadioPartitions {
    pub(crate) shared: SharedRadioRegisters,
    pub(crate) wifi: WifiPartition,
    pub(crate) bluetooth: BluetoothPartition,
    pub(crate) ieee802154: Ieee802154RadioPartition,
}

impl RadioPartitions {
    /// The radio's owners of a fictitious host-test root
    /// ([`RadioHardware::for_host_test`]): no test reaches a register
    /// through them.
    #[cfg(test)]
    pub(crate) fn for_host_test() -> Self {
        RadioHardware::for_host_test().partitions
    }

    /// Compose the radio's owners from the root's partitions. This performs
    /// no MMIO.
    ///
    /// The MAC local-time counter is a `Copy` reader: the radio PHY (its
    /// SDM-stability deadline) and the Wi-Fi MAC (its receive timestamps)
    /// each hold one. The shared baseband's RSSI word moves to the IEEE
    /// 802.15.4 partition; the modem ETM channels go to their protocols.
    fn compose(parts: RadioParts) -> Self {
        let RadioParts {
            wifi_mac,
            wifi_mac_interface,
            wifi_rx_dma,
            mac_local_time,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            modem_etm,
            ieee802154,
            analog_bus,
        } = parts;
        let ModemEtmPeripherals {
            modem_etm_ieee802154,
            modem_etm_bluetooth_phy,
            modem_etm_bluetooth,
        } = modem_etm;
        let local_time = MacLocalTime::new(mac_local_time);
        let (shared, rx_info) = SharedRadioRegisters::new(
            RadioPhyRegisters::new(radio_phy, analog_bus, local_time),
            coexistence,
            shared_radio,
        );
        Self {
            shared,
            wifi: WifiPartition {
                mac: WifiMacPartition::new(wifi_mac, wifi_mac_interface, local_time),
                rx_dma: WifiRxDmaRegisters::new(wifi_rx_dma),
            },
            bluetooth: BluetoothPartition {
                controller: BluetoothControllerParts {
                    controller: bluetooth,
                    phy_etm: modem_etm_bluetooth_phy,
                },
                modem_lp_timer: BluetoothModemLpTimerParts {
                    timer: bluetooth_modem_lp_timer,
                    etm: modem_etm_bluetooth,
                },
                interrupts: bluetooth_interrupts,
            },
            ieee802154: Ieee802154RadioPartition {
                mac: Ieee802154Partition::new(ieee802154, modem_etm_ieee802154, rx_info),
            },
        }
    }
}

/// Unique protocol-neutral owner of every ESP32-S31 radio partition.
///
/// It can be consumed by exactly one concurrent split, which cannot
/// manufacture a second owner. The split returns the complete root after
/// every partition and the domain have been reunited. Protocol-specific
/// cached state is deliberately not retained in this neutral owner.
///
/// ```compile_fail
/// use oer_esp32s31_hal::root::RadioHardware;
///
/// # fn split_twice<B: oer_esp32s31_hal::power::PlatformClockBackend>(hardware: RadioHardware) {
/// let _first = hardware.split::<B, _>(());
/// let _second = hardware.split::<B, _>(());
/// # }
/// ```
#[must_use = "dropping the radio root permanently loses the unique hardware capability"]
pub struct RadioHardware {
    partitions: RadioPartitions,
    phy_registration: PhyRegistration,
}

impl RadioHardware {
    fn new(partitions: RadioPartitions) -> Self {
        Self {
            partitions,
            phy_registration: PhyRegistration::new(),
        }
    }

    /// A fictitious complete radio root for one host test: no MMIO exists
    /// behind it, and no test reaches a register through it.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_host_test() -> Self {
        #[allow(
            unsafe_code,
            clippy::disallowed_methods,
            reason = "a host-test root claims the typed blocks of its fictitious chip"
        )]
        // SAFETY: only host tests build this root. It is fictitious: no
        // device registers exist behind the claimed addresses on the host
        // and no test accesses one through it, so the claim need not be
        // unique.
        let claims = unsafe { oer_mmio::assume_owned::<oer_esp32s31_pac::ownership::Claims>() };
        Self::of_partitions(oer_esp32s31_pac::ownership::partition(claims))
    }

    /// The radio root of `partitions`, the rest dropped.
    #[cfg(any(test, feature = "test-support"))]
    fn of_partitions(partitions: Partitions) -> Self {
        let Partitions {
            wifi_mac,
            wifi_mac_interface,
            wifi_rx_dma,
            mac_local_time,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            modem_etm,
            ieee802154,
            analog_bus,
            ..
        } = partitions;
        Self::new(RadioPartitions::compose(RadioParts {
            wifi_mac,
            wifi_mac_interface,
            wifi_rx_dma,
            mac_local_time,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            modem_etm,
            ieee802154,
            analog_bus,
        }))
    }

    /// Reassemble the neutral root from a returning route.
    ///
    /// Leaving the route retires the current PHY registration, so a
    /// registration result held apart from its hardware cannot describe a
    /// later route.
    fn returned(partitions: RadioPartitions, phy: PhyRouteState) -> Self {
        Self {
            partitions,
            phy_registration: phy.into_registration(),
        }
    }
}

/// The Wi-Fi MAC's partitions, taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiPartition {
    mac: WifiMacPartition,
    rx_dma: WifiRxDmaRegisters,
}

impl WifiPartition {
    /// The Wi-Fi MAC local-time counter: the clock of receive timestamps.
    pub const fn mac_local_time(&self) -> MacLocalTime {
        self.mac.mac_local_time()
    }

    pub(crate) fn into_parts(self) -> (WifiMacPartition, WifiRxDmaRegisters) {
        (self.mac, self.rx_dma)
    }

    pub(crate) const fn from_parts(mac: WifiMacPartition, rx_dma: WifiRxDmaRegisters) -> Self {
        Self { mac, rx_dma }
    }
}

/// The IEEE 802.15.4 MAC's partition with its event-task matrix channels
/// and the shared baseband's RSSI word, taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154RadioPartition {
    mac: Ieee802154Partition,
}

impl Ieee802154RadioPartition {
    pub(crate) fn into_mac(self) -> Ieee802154Partition {
        self.mac
    }

    pub(crate) const fn from_mac(mac: Ieee802154Partition) -> Self {
        Self { mac }
    }
}

/// Protocol partitions of a split radio root.
///
/// Each field can move to its own protocol driver; the shared radio stays in
/// the [`RadioDomain`] returned beside it.
/// Each protocol also receives its coexistence state, outside coexistence;
/// it returns with the partition when the split reunites.
pub struct ProtocolPartitions {
    pub wifi: WifiPartition,
    pub wifi_coex: CoexAbsent<WifiClient>,
    pub bluetooth: BluetoothPartition,
    pub bluetooth_coex: CoexAbsent<BluetoothClient>,
    pub ieee802154: Ieee802154RadioPartition,
    pub ieee802154_coex: CoexAbsent<Ieee802154Client>,
}

/// Why a split cannot become the neutral root again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReunionError {
    Shared(RadioDomainReleaseError),
}

/// Rejected reunion returning both unchanged owners.
#[must_use = "a rejected reunion still owns the domain and every partition"]
pub struct ReunionFailure<B: PlatformClockBackend, T = ()> {
    shared: RadioDomain<B, T>,
    partitions: ProtocolPartitions,
    error: ReunionError,
}

impl<B: PlatformClockBackend, T> ReunionFailure<B, T> {
    pub const fn error(&self) -> ReunionError {
        self.error
    }

    pub fn into_parts(self) -> (RadioDomain<B, T>, ProtocolPartitions) {
        (self.shared, self.partitions)
    }
}

impl RadioHardware {
    /// Split the root for concurrently running protocols. This performs no
    /// MMIO.
    ///
    /// The shared radio partitions and the shared PHY state go to the
    /// returned [`RadioDomain`], and every protocol partition to
    /// [`ProtocolPartitions`]. The two are separate owners, so a protocol
    /// driver takes its partition while the radio system owns the domain.
    ///
    /// `B` is the platform clock backend shared by every provider used with
    /// this domain.
    ///
    /// `attachment` is the upper layer's state kept with the domain, such
    /// as the PHY layer's registered domain slot.
    pub fn split<B: PlatformClockBackend, T>(
        self,
        attachment: T,
    ) -> (RadioDomain<B, T>, ProtocolPartitions) {
        let RadioPartitions {
            shared,
            wifi,
            bluetooth,
            ieee802154,
        } = self.partitions;
        (
            RadioDomain::new(
                shared,
                PhyRouteState::new(self.phy_registration),
                attachment,
            ),
            ProtocolPartitions {
                wifi,
                bluetooth,
                ieee802154,
                wifi_coex: CoexAbsent::issued(),
                bluetooth_coex: CoexAbsent::issued(),
                ieee802154_coex: CoexAbsent::issued(),
            },
        )
    }

    /// Reunite a split into the neutral root. This performs no MMIO.
    ///
    /// Leaving retires the PHY registration, as every route return does.
    ///
    /// # Errors
    ///
    /// Returns both owners while a client holds common power or a PHY
    /// calibration still owns a restore obligation.
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns the domain and every partition without allocation"
    )]
    pub fn reunite<B: PlatformClockBackend, T>(
        shared: RadioDomain<B, T>,
        partitions: ProtocolPartitions,
    ) -> Result<(Self, T), ReunionFailure<B, T>> {
        let (registers, phy, attachment) = match shared.into_parts() {
            Ok(parts) => parts,
            Err((shared, error)) => {
                return Err(ReunionFailure {
                    shared,
                    partitions,
                    error: ReunionError::Shared(error),
                });
            }
        };
        let ProtocolPartitions {
            wifi,
            wifi_coex: _,
            bluetooth,
            bluetooth_coex: _,
            ieee802154,
            ieee802154_coex: _,
        } = partitions;
        Ok((
            Self::returned(
                RadioPartitions {
                    shared: registers,
                    wifi,
                    bluetooth,
                    ieee802154,
                },
                phy,
            ),
            attachment,
        ))
    }
}

/// Why a cold protocol route cannot release the neutral radio root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioPhyReleaseError {
    /// TX-DC PWDET still owns fields that must be restored in this route.
    TxDcPwdetRestorePending,
    /// TX-IQ still owns a tone-control image that must be restored.
    TxIqToneControlRestorePending,
    /// RX-DCO still owns one or both nested control-field snapshots.
    RxDcoControlRestorePending,
    /// Bluetooth TX-power calibration still owns analog-control snapshots.
    BluetoothTxPowerControlRestorePending,
    /// The route still owns TSENS/SAR-I2C, or an acquisition failed mid-reset.
    TemperatureResourcesHeld,
    /// A route-owned cold-power field did not return to its captured baseline.
    WifiPowerRestore(WifiPowerRestoreCheckpoint),
}

/// Exact stage whose route-owned cold-power baseline could not be restored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPowerRestoreCheckpoint {
    /// Modem syscon clocks and resets did not read back.
    ModemSyscon,
    /// The upstream PLL-source fields did not read back.
    ModemSourceClocks,
    /// The modem register bus clock did not read back.
    ModemRegisterBusClock,
}

/// Reject release while a PHY calibration still owns a restore obligation.
pub(crate) fn check_phy_restore_complete(
    restore: &PhyRouteState,
) -> Result<(), RadioPhyReleaseError> {
    if restore.txdc_pending() {
        return Err(RadioPhyReleaseError::TxDcPwdetRestorePending);
    }
    if restore.txiq_pending() {
        return Err(RadioPhyReleaseError::TxIqToneControlRestorePending);
    }
    if restore.rx_dco_pending() {
        return Err(RadioPhyReleaseError::RxDcoControlRestorePending);
    }
    if restore.bluetooth_tx_power_control_pending() {
        return Err(RadioPhyReleaseError::BluetoothTxPowerControlRestorePending);
    }
    if restore.temperature_blocks_release() {
        return Err(RadioPhyReleaseError::TemperatureResourcesHeld);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
