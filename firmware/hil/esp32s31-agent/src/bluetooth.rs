//! Bluetooth LE images over the production composition and its HCI
//! Controller.
//!
//! Every image starts the shared radio with
//! `oer_esp32s31_radio_system::start`, which spawns its coexistence
//! schedule, starts the Bluetooth client on it, runs the radio's periodic PHY
//! tracking on its own task, which reports a tracking failure, and serves the
//! typed HIL console. The
//! `bluetooth-hil` image drives Direct Test Mode with HCI commands and the
//! `bluetooth-gatt` image runs the Trouble Host and the GATT application; both
//! run the radio runner and the HCI service, the body of the port, on one
//! client task ([`client`]), which has no restart: when the service ends,
//! the port's `disable` has accounted every admitted event, the task closes
//! the Host epoch, stops the Controller and only then reports why. The
//! `bluetooth-secure-gatt` image runs them beside its Host and restarts the
//! Controller epoch on request. All reach the radio only through the HCI
//! transport.
//!
//! The `wifi-ble-coex` image instead starts the GATT application as a client
//! of the Wi-Fi image's shared radio ([`shared`]); the Wi-Fi image keeps the
//! radio system, its periodic tasks and the HIL console.

#[cfg(feature = "bluetooth-radio")]
mod console;
#[cfg(feature = "bluetooth-hil")]
mod dtm;
#[cfg(any(feature = "bluetooth-gatt", feature = "wifi-ble-coex"))]
mod gatt;
#[cfg(feature = "bluetooth-secure-gatt")]
mod secure;
#[cfg(feature = "wifi-ble-coex")]
pub(crate) mod shared;

use oer_bluetooth_controller::LeVersionInformation;
#[cfg(any(feature = "bluetooth-hil", feature = "bluetooth-gatt"))]
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_bluetooth_system::{BluetoothEntropy, BluetoothParked, start_bluetooth_hci};
#[cfg(not(feature = "bluetooth-secure-gatt"))]
use oer_esp32s31_bluetooth_system::{BluetoothHciService, BluetoothSystem, BluetoothSystemBackend};
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_hal::root::ConcurrentPartitions;
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_soc_esp_hal::entropy::Entropy;
#[cfg(feature = "bluetooth-radio")]
use static_cell::StaticCell;

/// Development Controller identity for Link Layer version exchange: Core
/// 5.4, the unassigned company value and subversion 1.
const VERSION: LeVersionInformation = LeVersionInformation::new(0x0d, 0xffff, 1);

#[cfg(feature = "bluetooth-radio")]
pub(super) type Radio = oer_esp32s31_radio_system::SharedRadio;
#[cfg(feature = "wifi-ble-coex")]
pub(super) type Radio = oer_esp32s31_ieee80211_system::SharedRadio;

#[cfg(feature = "bluetooth-radio")]
static ENTROPY: StaticCell<BluetoothEntropy<'static>> = StaticCell::new();

#[cfg(feature = "bluetooth-radio")]
pub(super) fn start(
    executor: &'static mut super::Executor<0>,
    wake: crate::ExecutorWake,
    interrupts: oer_esp32s31_radio_esp_hal::EspHalBluetoothInterruptRoutes,
    platform: EspHalRadioPlatform,
    usb: crate::transport::Usb,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let entropy = Entropy::new(rng);
    let boot = u64::from_le_bytes(entropy.random_bytes().expect("HIL boot entropy")).max(1);
    crate::transport::CONSOLE.start(boot);
    crate::transport::init_logger();
    let entropy = ENTROPY.init(BluetoothEntropy::new(entropy));
    let public_address = platform.bluetooth_public_address();
    executor.run(wake, |spawner| {
        // The image keeps its own tracking task, which reports a tracking
        // failure as `reason=phy-tracking` before the reset.
        let start = oer_esp32s31_radio_system::RadioStart::new()
            .with_tracking(oer_esp32s31_radio_system::Tracking::Caller);
        let (radio, partitions) = oer_esp32s31_radio_system::start(spawner, platform, start)
            .expect("the shared radio must start once");
        spawner.spawn(
            main(
                spawner,
                radio,
                partitions,
                interrupts,
                public_address,
                entropy,
                usb,
                boot,
            )
            .expect("Bluetooth task"),
        );
    })
}

#[cfg(feature = "bluetooth-radio")]
#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the start result crosses one poll boundary; the linked image's stack gate and runtime stack painting check its stack use"
)]
async fn main(
    spawner: embassy_executor::Spawner,
    radio: &'static Radio,
    partitions: ConcurrentPartitions,
    interrupts: oer_esp32s31_radio_esp_hal::EspHalBluetoothInterruptRoutes,
    public_address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    entropy: &'static BluetoothEntropy<'static>,
    usb: crate::transport::Usb,
    boot: u64,
) {
    let ConcurrentPartitions {
        bluetooth,
        bluetooth_coex,
        ..
    } = partitions;
    let Ok(parked) = BluetoothParked::new(bluetooth, bluetooth_coex, interrupts) else {
        super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-memory\r\n");
    };
    let (system, backend) =
        match oer_esp32s31_bluetooth_system::start(radio, parked, public_address).await {
            Ok(started) => started,
            Err(_) => super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-start\r\n"),
        };
    let hci = start_bluetooth_hci(public_address, Some(VERSION), entropy);
    spawner.spawn(tracking(radio).expect("PHY tracking task"));
    #[cfg(trace_journal)]
    if let Ok(hold) = crate::trace::hold_limit_task() {
        spawner.spawn(hold);
    }
    #[cfg(feature = "bluetooth-secure-gatt")]
    secure::run(
        spawner,
        radio,
        system,
        backend,
        hci,
        public_address,
        usb,
        boot,
    )
    .await;
    #[cfg(not(feature = "bluetooth-secure-gatt"))]
    {
        spawner.spawn(client(radio, system, backend, hci.service).expect("Bluetooth client task"));
        image(spawner, hci.host, usb, boot).await;
    }
}

#[cfg(feature = "bluetooth-hil")]
async fn image(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: crate::transport::Usb,
    boot: u64,
) -> ! {
    dtm::run(spawner, host, usb, boot).await
}

#[cfg(feature = "bluetooth-gatt")]
async fn image(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: crate::transport::Usb,
    boot: u64,
) {
    spawner.spawn(gatt::task(host, usb, boot).expect("Bluetooth GATT task"));
}

#[cfg(feature = "bluetooth-radio")]
#[embassy_executor::task]
async fn tracking(radio: &'static Radio) {
    let _error = radio.run_tracking().await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=phy-tracking\r\n");
}

/// Drive the radio runner and the HCI service of a client without restart
/// until the service ends, then close the Host epoch, stop the Controller
/// and report why the service ended.
///
/// A runner fault poisons the port, so the service ends at its next call;
/// it, a poisoned port or a failed Controller stop keeps every other owner
/// as it is: the report diverges with them alive.
#[cfg(not(feature = "bluetooth-secure-gatt"))]
#[embassy_executor::task]
async fn client(
    radio: &'static Radio,
    mut system: BluetoothSystem,
    backend: BluetoothSystemBackend,
    mut service: BluetoothHciService,
) {
    use core::pin::pin;
    use embassy_futures::select::{Either, select};
    use oer_esp32s31_bluetooth_system::Served;

    let (runner_faulted, (served, uninstalled)) = {
        let mut runner = pin!(system.run(radio));
        let mut serving = pin!(service.run(backend, core::future::pending()));
        match select(runner.as_mut(), serving.as_mut()).await {
            Either::First(_fault) => (true, serving.await),
            Either::Second(ended) => (false, ended),
        }
    };
    if runner_faulted {
        super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-runner-fault\r\n");
    }
    let (Served::Stopped(exit), Some(uninstalled)) = (served, uninstalled) else {
        super::fail(stop_failure_reason(&served));
    };
    // No restart follows: the Host epoch closes for good.
    service.close();
    let stopped = system.stop(uninstalled, radio).await;
    let Ok(_parked) = stopped else {
        super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-stop\r\n");
    };
    super::fail(exit_reason(exit))
}

/// The report of a service that ended for `exit` and disabled the port; the
/// Controller then stops without a restart.
#[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
fn exit_reason(exit: oer_esp32s31_bluetooth_system::ServeExit) -> &'static core::ffi::CStr {
    use oer_esp32s31_bluetooth_system::ServeExit;
    match exit {
        ServeExit::EpochExhausted(_) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-epoch-exhausted\r\n"
        }
        ServeExit::Stopped | ServeExit::Closed | ServeExit::Transport(_) | ServeExit::Clock(_) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-service\r\n"
        }
    }
}

/// The report of a service whose port's `disable` failed, naming why the
/// service stopped serving, or that found the port poisoned. The caller
/// keeps every owner as it is.
#[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
fn stop_failure_reason(
    served: &oer_esp32s31_bluetooth_system::Served<
        oer_esp32s31_bluetooth_system::BluetoothSystemFault,
    >,
) -> &'static core::ffi::CStr {
    use oer_esp32s31_bluetooth_system::{ServeExit, Served};
    match served {
        Served::DisableFailed {
            exit: ServeExit::EpochExhausted(_),
            ..
        } => c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-epoch-exhausted-stop-failed\r\n",
        Served::DisableFailed { .. } => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-service-stop-failed\r\n"
        }
        Served::Poisoned(cause) => poisoned_reason(*cause),
        Served::Unretired(_) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-driver-unretired\r\n"
        }
        // A stopped service is no stop failure.
        Served::Stopped(exit) => exit_reason(*exit),
    }
}

/// The report of a port poisoned while it served, naming the fault.
#[cfg(any(feature = "bluetooth-radio", feature = "wifi-ble-coex"))]
fn poisoned_reason(
    cause: oer_esp32s31_bluetooth_system::BluetoothSystemFault,
) -> &'static core::ffi::CStr {
    use oer_esp32s31_bluetooth_system::{
        BluetoothFault as Fault, BluetoothRuntimeFault as Runtime, RadioFault as Radio,
    };
    match cause {
        Fault::Radio(Radio::UnsupportedHardwareResult) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-radio-unsupported-result\r\n"
        }
        Fault::Radio(Radio::MemoryInconsistency) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-radio-memory\r\n"
        }
        Fault::Radio(Radio::OutOfOrderCompletion) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-radio-out-of-order\r\n"
        }
        Fault::Runner(Runtime::Scheduler(_)) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-scheduler\r\n"
        }
        Fault::Runner(Runtime::Start(_)) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-start\r\n"
        }
        Fault::Runner(Runtime::StopSequence(_)) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-stop-sequence\r\n"
        }
        Fault::Runner(Runtime::FinishedLists(_)) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-finished-lists\r\n"
        }
        Fault::Runner(Runtime::Stop) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-stop\r\n"
        }
        Fault::Runner(Runtime::Time(_)) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-time\r\n"
        }
        Fault::Runner(Runtime::TransitionTimedOut) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-transition-timed-out\r\n"
        }
        Fault::Runner(Runtime::Abandoned) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-abandoned\r\n"
        }
        Fault::Runner(Runtime::Unsettled) => {
            c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-unsettled\r\n"
        }
        Fault::Platform => c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-poisoned-platform\r\n",
    }
}
