#![no_main]
#![no_std]
#![forbid(unsafe_code)]

//! W0: the radio system's first run on the chip. The actor registers the PHY
//! domain for Wi-Fi's `join`, tracks it, brings the MAC up for `track` and
//! closes RF after `leave`; the image logs the calibration path, the
//! registration and tracking durations, the temperature and the longest
//! not-ready runs of the bounded readiness waits, then prints
//! `OER-RESULT: pass w0`. Nothing is transmitted.
//!
//! eFuse is only read: the base MAC and its extension word for the
//! calibration identity, and the interface addresses esp-hal derives from
//! the base MAC.

// The image declares the trace class in its manifest; `cargo fw` sets the cfg.
#[cfg(not(oer_trace))]
compile_error!(
    "w0 declares the trace class: build it with `cargo fw build w0`, check it with RUSTFLAGS=\"--cfg oer_trace\""
);

use core::fmt::Write as _;

use esp_hal::{
    Blocking, efuse, interrupt::software::SoftwareInterrupt, rng::RngOwner,
    usb::usb_serial_jtag::UsbSerialJtag,
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::ieee80211::init::{
    MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
    MacSlowClockCalibrationSource,
};
use oer_esp32s31_hal::root::ProtocolPartitions;
use oer_esp32s31_ieee80211_driver::startup::{
    RadioStartConfig, RadioStartFailure, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
    start_esp32s31_radio,
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, PhyPreparation, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_esp_hal_radio::{EspHalClocks, RadioSingletons, take_radio};
use oer_espressif_executor::{Executor, InterruptExecutor, LevelOrder};
use oer_espressif_staged_runtime::panic;
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    /// The application executor's wake (thread mode).
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu;
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<2>, Priority2, ProCpu;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu;
}

/// The radio executor between the latching interrupts and the application.
const LEVELS: LevelOrder<RadioAlarmToken, RadioToken, WakeToken> = LevelOrder::new();

type Console = UsbSerialJtag<'static, Blocking>;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO_EXECUTOR: StaticCell<InterruptExecutor<2>> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<EspHalClocks, EmbassyClock, NoopPhyTargetObserver>,
    EmbassyClock,
>;

/// What the Wi-Fi client needs from the boot.
struct Boot {
    console: Console,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: RngOwner<'static>,
    station: [u8; 6],
    access_point: [u8; 6],
}

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    if let Some(record) = previous {
        let _ = write!(console, "OER-PANIC previous {record}\r\n");
    }
    let Some(interrupts) = Interrupts::take() else {
        let _ = console.write(b"OER-RESULT: fail w0 interrupt-table\r\n");
        park()
    };

    let base_mac = efuse::base_mac_address();
    let mut base_mac_address = [0; 6];
    base_mac_address.copy_from_slice(base_mac.as_bytes());
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address,
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    };
    let mut station = [0; 6];
    station.copy_from_slice(
        efuse::interface_mac_address(efuse::InterfaceMacAddress::Station).as_bytes(),
    );
    let mut access_point = [0; 6];
    access_point.copy_from_slice(
        efuse::interface_mac_address(efuse::InterfaceMacAddress::AccessPoint).as_bytes(),
    );

    let singletons = RadioSingletons {
        i2c_ana_mst: peripherals.I2C_ANA_MST,
        modem_syscon: peripherals.MODEM_SYSCON,
        modem_lpcon: peripherals.MODEM_LPCON,
        wifi: peripherals.WIFI,
        bt: peripherals.BT,
        ieee802154: peripherals.IEEE802154,
        lp_tsens: peripherals.LP_TSENS,
    };
    let (hardware, clocks) = match take_radio(singletons, &psram) {
        Ok(radio) => radio,
        Err(error) => {
            let _ = write!(console, "OER-RESULT: fail w0 take_radio={error:?}\r\n");
            park()
        }
    };
    let (domain, partitions) = hardware.split(ConcurrentPhy::new());
    let _ = write!(
        console,
        "OER w0 identity rf_cal_version={} base_mac={:02x?} mac_ext={:#06x}\r\n",
        identity.rf_cal_version, identity.base_mac_address, identity.mac_extension
    );
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        let _ = console.write(b"OER-RESULT: fail w0 claim\r\n");
        park()
    };
    let ProtocolPartitions {
        wifi, wifi_coex, ..
    } = partitions;
    let backend = Esp32s31Radio::new(
        domain,
        wifi_coex,
        clocks,
        EmbassyClock,
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = RadioSystem::new(end, backend, EmbassyClock, lp);

    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    oer_espressif_executor::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor::bind::<0, _>(interrupts.app_alarm);
    let radio = RADIO_EXECUTOR
        .init(InterruptExecutor::<2>::new(SoftwareInterrupt::new(
            peripherals.FROM_CPU_INTR2,
        )))
        .start(interrupts.radio, LEVELS);
    if let Ok(token) = radio_actor(actor) {
        radio.spawn(token);
    }
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    adopted.enable_interrupts();
    let boot = Boot {
        console,
        cell: cells.wifi,
        wifi,
        rng: RngOwner::new(peripherals.RNG),
        station,
        access_point,
    };
    executor.run(interrupts.wake, move |spawner| {
        // The task's one slot is free: this is its only spawn.
        if let Ok(token) = w0(boot) {
            spawner.spawn(token);
        }
    })
}

include!(concat!(env!("OUT_DIR"), "/w0_config.rs"));

/// The radio system's actor, on the radio executor (P2).
#[embassy_executor::task]
async fn radio_actor(actor: Actor) {
    match actor.run().await {}
}

/// The Wi-Fi client, on the application executor.
#[embassy_executor::task]
async fn w0(boot: Boot) {
    let Boot {
        mut console,
        cell,
        wifi,
        rng,
        station,
        access_point,
    } = boot;
    match run(&mut console, cell, wifi, rng, station, access_point).await {
        Ok(()) => {
            let _ = console.write(b"OER-RESULT: pass w0\r\n");
        }
        Err(reason) => {
            let _ = write!(console, "OER-RESULT: fail w0 {reason}\r\n");
        }
    }
}

async fn run(
    console: &mut Console,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: RngOwner<'static>,
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
    let stopped = match started {
        Ok(stopped) => stopped,
        Err(RadioStartFailure::Join { failed, .. }) => {
            let _ = write!(console, "OER w0 join failed cause={:?}\r\n", failed.cause);
            return Err("join");
        }
        Err(RadioStartFailure::Track { failed, .. }) => {
            let _ = write!(console, "OER w0 track failed cause={:?}\r\n", failed.cause);
            return Err("track");
        }
    };
    let report = stopped.start_report();
    match report.phy {
        PhyPreparation::Registered {
            calibration_path,
            micros,
            outcome,
        } => {
            let _ = write!(
                console,
                "OER w0 join epoch={} registered calibration_path={calibration_path:?} registration_us={micros}\r\n",
                report.epoch.get()
            );
            let _ = write!(
                console,
                "OER w0 ready_waits tx_dc_max_not_ready={} pwdet_sar_max_not_ready={}\r\n",
                outcome.ready_waits.tx_dc_max_not_ready_samples,
                outcome.ready_waits.pwdet_sar_max_not_ready_samples
            );
            let _ = write!(
                console,
                "OER w0 rf_calibration {:?}\r\n",
                outcome.rf_calibration
            );
        }
        other => {
            let _ = write!(console, "OER w0 join phy={other:?}\r\n");
            return Err("not-registered");
        }
    }
    let _ = write!(
        console,
        "OER w0 track tracking_us={:?} temperature={:?} mac_handshake_samples={} mac_handshake_observations={}\r\n",
        report.track.tracking_micros,
        report.track.temperature,
        report.track.mac.handshake_samples,
        report.track.mac.handshake_observations
    );
    let (_cell, released) = stopped.release(cell).await;
    match released {
        Ok(released) => {
            let cold = matches!(
                released.wifi,
                oer_esp32s31_radio_client::wifi::WifiReturned::Cold(_)
            );
            let _ = write!(
                console,
                "OER w0 leave domain={:?} cold={cold}\r\n",
                released.domain
            );
            if !cold || released.domain != oer_esp32s31_radio_client::DomainHealth::Healthy {
                return Err("leave");
            }
        }
        Err(failure) => {
            let _ = write!(console, "OER w0 release refused {:?}\r\n", failure.error);
            return Err("release");
        }
    }
    Ok(())
}

/// Wi-Fi's own platform values for the MAC bring-up.
struct Platform {
    rng: RngOwner<'static>,
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

fn park() -> ! {
    oer_espressif_staged_runtime::idle()
}
