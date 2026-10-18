#![no_main]
#![no_std]

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

use core::fmt::Write as _;

use embassy_futures::select::{Either, select};
use esp_hal::{
    Blocking, efuse,
    interrupt::software::SoftwareInterrupt,
    rng::Rng,
    timer::{OneShotTimer, systimer::SystemTimer},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use oer_esp32s31_hal::{domain::RadioDomain, root::ProtocolPartitions};
use oer_esp32s31_ieee80211::startup::{
    RadioStartConfig, RadioStartFailure, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
    start_esp32s31_radio,
};
use oer_esp32s31_ieee80211_mac::init::{
    MacDelayEntropy, MacSlowClockCalibration, MacSlowClockCalibrationSource,
};
use oer_esp32s31_phy::{
    HARDWARE_EDGE_LIMIT, NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_platform_board as board;
use oer_esp32s31_radio_client::{LpClock, PhyPreparation, RcSlow, claim};
use oer_esp32s31_radio_esp_hal::{EspHalClocks, RadioSingletons, take_radio};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor_embassy::Executor;
use oer_espressif_staged_runtime::panic;
use oer_ieee80211_mac::channel::{WifiChannel, WifiChannelWidth};
use oer_ieee80211_softmac::WifiMacAddress;
use oer_time_embassy::EmbassyClock;
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    /// The executor's wake.
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The time driver's alarm.
    alarm: AlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor_embassy::timer_interrupt, Priority1, ProCpu;
}

type Console = UsbSerialJtag<'static, Blocking>;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();

/// What the run needs from the boot.
struct Boot {
    console: Console,
    domain: RadioDomain<ConcurrentPhy>,
    partitions: ProtocolPartitions,
    clocks: EspHalClocks,
    identity: PhyCalibrationIdentity,
    station: [u8; 6],
    access_point: [u8; 6],
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // SAFETY: `runtime_main` runs once on CPU0, right after `_runtime_start`,
    // with interrupts disabled and the bootstrap's mapping intact, which
    // nothing has reset or remapped; interrupts stay disabled until the
    // executor and time driver are bound below.
    let psram = unsafe {
        oer_espressif_staged_runtime::adopt(INTERRUPT_TABLE, || {
            board::adopt_psram_mapping(peripherals.PSRAM)
        })
    };
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

    let systimer = SystemTimer::new(peripherals.SYSTIMER);
    oer_espressif_executor_embassy::init(OneShotTimer::new(systimer.alarm0), interrupts.alarm);
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    // SAFETY: the vectors, the interrupt stack, the time driver and, from
    // `run`, the executor's wake are bound; the interrupt table routes only
    // their two sources, and interrupts were disabled since the handoff.
    unsafe { oer_espressif_staged_runtime::enable_interrupts_after_handoff() };
    let boot = Boot {
        console,
        domain,
        partitions,
        clocks,
        identity,
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

/// The radio system's actor and the Wi-Fi client, polled together.
#[embassy_executor::task]
async fn w0(boot: Boot) {
    let Boot {
        mut console,
        domain,
        partitions,
        clocks,
        identity,
        station,
        access_point,
    } = boot;
    let _ = write!(
        console,
        "OER w0 identity rf_cal_version={} base_mac={:02x?} mac_ext={:#06x}\r\n",
        identity.rf_cal_version, identity.base_mac_address, identity.mac_extension
    );
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        let _ = console.write(b"OER-RESULT: fail w0 claim\r\n");
        return;
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
    let app = async {
        let verdict = run(&mut console, cells.wifi, wifi, station, access_point).await;
        match verdict {
            Ok(()) => {
                let _ = console.write(b"OER-RESULT: pass w0\r\n");
            }
            Err(reason) => {
                let _ = write!(console, "OER-RESULT: fail w0 {reason}\r\n");
            }
        }
        core::future::pending::<()>().await
    };
    match select(actor.run(), app).await {
        Either::First(never) => match never {},
        Either::Second(()) => {}
    }
}

async fn run(
    console: &mut Console,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    station: [u8; 6],
    access_point: [u8; 6],
) -> Result<(), &'static str> {
    let (Ok(station), Ok(access_point)) = (
        WifiMacAddress::new(station),
        WifiMacAddress::new(access_point),
    ) else {
        return Err("efuse-mac");
    };
    let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let config = RadioStartConfig::new(
        WifiColdStartConfig::new(channel),
        WifiMacStartConfig::new(u32::from(HARDWARE_EDGE_LIMIT), station, access_point),
    );
    let (cell, started) = start_esp32s31_radio(cell, wifi, Platform::default(), config).await;
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
                "OER w0 ready_waits tx_dc_max_not_ready={} pwdet_sar_max_not_ready={} limit={HARDWARE_EDGE_LIMIT}\r\n",
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
#[derive(Default)]
struct Platform {
    tx_power: Option<PhyTxTargetPowerProfile>,
}

impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        Rng::new().random()
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
    loop {
        // SAFETY: `wfi` only waits until the next interrupt.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
