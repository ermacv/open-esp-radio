#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]

//! W0: the radio system's first run on the chip. The actor registers the PHY
//! domain for Wi-Fi's `join`, tracks it, prepares the MAC for `track` and
//! closes RF after `leave`; the image logs the calibration path, the
//! registration and tracking durations, the temperature and the longest
//! not-ready runs of the bounded readiness waits, then prints
//! `OER-RESULT: pass w0`. MAC gates stay closed; no BASE or runtime start is
//! published without a ring. Nothing is transmitted.
//!
//! eFuse is only read: the base MAC and its extension word for the
//! calibration identity, and the interface addresses derived from
//! the base MAC.

// The image declares the trace class in its manifest; `cargo fw` sets the cfg.
#[cfg(not(oer_trace))]
compile_error!(
    "w0 declares the trace class: build it with `cargo fw build w0`, check it with RUSTFLAGS=\"--cfg oer_trace\""
);

use oer_diagnostics::{DiagPath, DiagnosticPath, Note};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::ieee80211::init::{
    MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
    MacSlowClockCalibrationSource,
};
use oer_esp32s31_hal::platform::{
    FactoryIdentity, FromCpu0Irq, FromCpu2Irq, Interface, SystimerAlarm0, SystimerAlarm1,
};
use oer_esp32s31_hal::power::ChipClocks;
use oer_esp32s31_hal::root::{ProtocolPartitions, Root};
use oer_esp32s31_hal::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters};
use oer_esp32s31_ieee80211_driver::startup::{
    RadioStartConfig, RadioStartFailure, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
    start_esp32s31_radio,
};
use oer_esp32s31_phy::tracking::TemperatureAcquisition;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{DomainHealth, Failure};
use oer_esp32s31_radio_client::{LpClock, PhyPreparation, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_espressif_staged_runtime::panic;
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 18, context radio, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 19, context app_wake, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 20, context app_wake, hart 0, Hardware;
}

/// The radio executor between the latching interrupts and the application.
mod usb;
use usb::Console;

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<0> {
        w0(boot: Boot) = w0;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        ChipClocks,
        oer_espressif_executor::Cooperative<EmbassyClock>,
        NoopPhyTargetObserver,
    >,
    EmbassyClock,
>;

/// What the Wi-Fi client needs from the boot.
struct Boot {
    console: Console,
    /// The console's pads, held so that nothing switches them off.
    _pads: oer_esp32s31_hal::usb_serial_jtag::UsbPads,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    access_point: [u8; 6],
}

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::platform::Entropy<EmbassyClock>;

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let Some(root) = Root::take() else {
        fail("root-taken")
    };
    let runtime = root.into_runtime();
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    if let Some(record) = previous {
        usb::early_previous_panic(&record);
    }
    let Some(interrupts) = Interrupts::take() else {
        fail("interrupt-table")
    };

    let factory = FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(Interface::Station);
    let access_point = factory.address(Interface::AccessPoint);

    let (domain, partitions) = runtime.radio.split::<ChipClocks, _>(ConcurrentPhy::new());
    usb::early(&Note::w0_identity(
        identity.rf_cal_version,
        &identity.base_mac_address,
        u32::from(identity.mac_extension),
    ));
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        fail("claim")
    };
    let ProtocolPartitions {
        wifi, wifi_coex, ..
    } = partitions;
    let backend = Esp32s31Radio::new(
        domain,
        wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));

    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    );
    oer_espressif_executor::bind::<0, _, _>(adopted.router(), interrupts.app_alarm, timer.alarm1);
    let software = runtime.software_interrupts;
    let radio = InterruptExecutor::<2>::new(software.line2).start(
        adopted.router(),
        interrupts.radio,
        software.irq2,
    );
    if RadioTasks::radio_actor(&radio, actor).is_err() {
        fail("radio-actor-start");
    }
    let (console, pads) = usb::open(adopted.router(), interrupts.console_usb);
    let executor = Executor::<0>::new(software.line0);
    let mut router = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let rng = Rng::new(runtime.entropy, EmbassyClock);
    let watchdog = runtime.watchdogs.rtc;
    executor.run(
        &mut router,
        interrupts.wake,
        software.irq0,
        move |spawner| {
            let boot = Boot {
                console,
                _pads: pads,
                cell: cells.wifi,
                wifi,
                rng,
                station,
                access_point,
            };
            if AppTasks::w0(spawner, boot).is_err()
                || AppTasks::rtc_watchdog(spawner, watchdog).is_err()
            {
                fail("task-start");
            }
        },
    )
}

/// Feed the RTC watchdog for good: the image resets when this executor
/// stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

include!(concat!(env!("OUT_DIR"), "/w0_config.rs"));

/// The radio system's actor, on the radio executor (P2).
async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}

/// The Wi-Fi client, on the application executor.
async fn w0(boot: Boot) {
    let Boot {
        mut console,
        _pads,
        cell,
        wifi,
        rng,
        station,
        access_point,
    } = boot;
    match run(&mut console, cell, wifi, rng, station, access_point).await {
        Ok(()) => {
            note(&mut console, &Note::w0_pass()).await;
        }
        Err(reason) => {
            note(&mut console, &Note::w0_fail(reason)).await;
        }
    }
}

async fn run(
    console: &mut Console,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    access_point: [u8; 6],
) -> Result<(), &'static str> {
    let (Ok(station), Ok(access_point)) = (
        IndividualAddress::for_interface(station),
        IndividualAddress::for_interface(access_point),
    ) else {
        return Err("efuse-mac");
    };
    let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let country = COUNTRY.ok_or("unconfigured")?;
    let domain = Country::new(country)
        .ok()
        .and_then(RegulatoryDomain::of)
        .ok_or("domain")?;
    let channel = domain
        .permit(channel)
        .map_err(|_| "channel-not-permitted")?;
    let ceiling = channel.tx_ceiling(board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        None,
    ));
    let config = RadioStartConfig::new(
        WifiColdStartConfig::new(channel, ceiling),
        WifiMacStartConfig::new(MAC_COLD_HANDSHAKE_DEADLINE, station, access_point)
            .map_err(|_| "mac-address")?,
    );
    let (cell, started) = start_esp32s31_radio(
        cell,
        wifi,
        Platform {
            rng,
            tx_power: None,
        },
        config,
    )
    .await;
    let mut stopped = match started {
        Ok(stopped) => stopped,
        Err(RadioStartFailure::Join { failed, .. }) => {
            note(console, &Note::w0_join_failed(failure_name(failed.cause))).await;
            return Err("join");
        }
        Err(RadioStartFailure::Track { failed, .. }) => {
            note(console, &Note::w0_track_failed(failure_name(failed.cause))).await;
            return Err("track");
        }
    };
    if !stopped.radio_mut().0.mac_core_gates_closed() {
        return Err("mac-gates-open");
    }
    let report = stopped.start_report();
    match report.phy {
        PhyPreparation::Registered {
            calibration_path,
            micros,
            outcome,
        } => {
            let mut path = DiagPath::new();
            calibration_path.push_path(&mut path);
            note(
                console,
                &Note::w0_join(
                    report.epoch.get(),
                    path.names().first().copied().unwrap_or(""),
                    micros,
                ),
            )
            .await;
            note(
                console,
                &Note::w0_ready_waits(
                    u32::from(outcome.ready_waits.tx_dc_max_not_ready_samples),
                    u32::from(outcome.ready_waits.pwdet_sar_max_not_ready_samples),
                ),
            )
            .await;
            match outcome.rf_calibration {
                None => note(console, &Note::w0_rf_calibration_none()).await,
                Some(rf) => {
                    let frequency = rf.frequency;
                    note(
                        console,
                        &Note::w0_rf_calibration(
                            rf.charge_pump_locked,
                            frequency.is_some(),
                            frequency.map_or(0, |frequency| u32::from(frequency.table_entries)),
                        ),
                    )
                    .await;
                    if let Some(frequency) = frequency {
                        for (name, point) in [
                            ("nominal", frequency.nominal),
                            ("low", frequency.low),
                            ("high", frequency.high),
                        ] {
                            note(
                                console,
                                &Note::w0_rf_point(
                                    name,
                                    point.lock_observed,
                                    u32::from(point.initial_cap),
                                    u32::from(point.final_cap),
                                    u32::from(point.accepted_cap_samples),
                                ),
                            )
                            .await;
                        }
                    }
                }
            }
        }
        other => {
            let (name, micros) = match other {
                PhyPreparation::AlreadyOpen => ("AlreadyOpen", 0),
                PhyPreparation::Woke { micros } => ("Woke", micros),
                PhyPreparation::Registered { .. } => ("Registered", 0),
            };
            note(console, &Note::w0_join_phy(name, micros)).await;
            return Err("not-registered");
        }
    }
    note(
        console,
        &Note::w0_track(
            report.track.tracking_micros.is_some(),
            report.track.tracking_micros.unwrap_or(0),
            report.track.mac.handshake_samples,
            report.track.mac.handshake_observations,
        ),
    )
    .await;
    let (name, started, completed) = match report.track.temperature.acquisition {
        TemperatureAcquisition::Unobserved => ("Unobserved", 0, 0),
        TemperatureAcquisition::Undated => ("Undated", 0, 0),
        TemperatureAcquisition::Overlong => ("Overlong", 0, 0),
        TemperatureAcquisition::ClockReversed => ("ClockReversed", 0, 0),
        TemperatureAcquisition::Window { started, completed } => ("Window", started, completed),
    };
    note(
        console,
        &Note::w0_temperature(
            i32::from(report.track.temperature.value),
            name,
            started,
            completed,
        ),
    )
    .await;
    let (_cell, released) = stopped.release(cell).await;
    match released {
        Ok(released) => {
            let cold = matches!(
                released.wifi,
                oer_esp32s31_radio_client::wifi::WifiReturned::Cold(_)
            );
            note(
                console,
                &Note::w0_leave(
                    match released.domain {
                        DomainHealth::Healthy => "Healthy",
                        DomainHealth::Poisoned(_) => "Poisoned",
                    },
                    cold,
                ),
            )
            .await;
            if !cold || released.domain != oer_esp32s31_radio_client::DomainHealth::Healthy {
                return Err("leave");
            }
        }
        Err(failure) => {
            let (name, state) = match failure.error {
                oer_esp32s31_hal::owner::maintenance::Error::MacActive { state } => {
                    ("MacActive", u32::from(state))
                }
                oer_esp32s31_hal::owner::maintenance::Error::RxWalkerEnabled => {
                    ("RxWalkerEnabled", 0)
                }
            };
            note(console, &Note::w0_release_refused(name, state)).await;
            return Err("release");
        }
    }
    Ok(())
}

/// Wi-Fi's own platform values for the MAC bring-up.
struct Platform {
    rng: Rng,
    tx_power: Option<PhyTxTargetPowerProfile>,
}

impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng.random()
    }
}

impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        // W0 calibrates no RTC slow clock; the MAC records that none was
        // supplied.
        MacSlowClockCalibration::Unavailable
    }
}

impl WifiMacPlatform for Platform {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.tx_power = Some(profile);
    }
}

/// Writes `note` to the console as a typed diagnostic line.
async fn note(console: &mut Console, note: &Note<'_>) {
    let _ = oer_espressif_usb_serial_jtag::write_note(console, note).await;
}

fn failure_name(cause: Failure) -> &'static str {
    match cause {
        Failure::Refused => "Refused",
        Failure::ClientFailed => "ClientFailed",
        Failure::Poisoned(_) => "Poisoned",
    }
}

/// Report a failed boot step on the early writer and wait: no task feeds
/// the RTC watchdog, which resets the chip.
fn fail(reason: &'static str) -> ! {
    usb::early(&Note::w0_fail(reason));
    oer_espressif_staged_runtime::idle()
}
