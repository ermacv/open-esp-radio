//! The Bluetooth LE client of the Wi-Fi image's shared radio.
//!
//! The Wi-Fi image owns the radio system, its PHY tracking and coexistence
//! schedule tasks and the HIL console. This client parks the Controller on the
//! radio's Bluetooth partition, starts it on the shared radio and runs the
//! radio runner and the HCI service on one client task, as the Bluetooth images
//! do, and the Trouble GATT application on its own. The application's observations are published for the Wi-Fi
//! console's `QueryBluetoothGatt` answer.

use core::cell::Cell;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
use oer_esp32s31_bluetooth_system::{
    BluetoothEntropy, BluetoothHostTransport, BluetoothParked, start_bluetooth_hci,
};
use oer_esp32s31_hal::{
    root::BluetoothPartition,
    shared_radio::{BluetoothClient, CoexAbsent},
};
use oer_tool_data_hil_protocol::bluetooth::BluetoothGattEvidence;

use super::{Radio, VERSION, gatt};

/// The last application observation; `None` until the Host reports one.
static EVIDENCE: Mutex<CriticalSectionRawMutex, Cell<Option<BluetoothGattEvidence>>> =
    Mutex::new(Cell::new(None));

/// Start the client on the shared radio after the radio system exists.
///
/// The caller keeps running the radio's PHY tracking and coexistence
/// schedule; this client adds no radio-system task.
pub(crate) fn start(
    spawner: Spawner,
    radio: &'static Radio,
    partition: BluetoothPartition,
    coex: CoexAbsent<BluetoothClient>,
    interrupts: oer_adapters_esp_hal_radio::EspHalBluetoothInterruptRoutes,
    public_address: BluetoothPublicDeviceAddress,
    entropy: &'static BluetoothEntropy<'static>,
) {
    spawner.spawn(
        client(
            spawner,
            radio,
            partition,
            coex,
            interrupts,
            public_address,
            entropy,
        )
        .expect("Bluetooth client task must allocate once"),
    );
}

/// The application's observations with the current CPU0 stack measurement.
pub(crate) fn evidence() -> BluetoothGattEvidence {
    let mut value = EVIDENCE.lock(Cell::get).unwrap_or_default();
    value.cpu0_stack = Some(crate::cpu0_stack_usage_snapshot());
    value
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the start result crosses one poll boundary; the linked image's stack gate and runtime stack painting check its stack use"
)]
async fn client(
    spawner: Spawner,
    radio: &'static Radio,
    partition: BluetoothPartition,
    coex: CoexAbsent<BluetoothClient>,
    interrupts: oer_adapters_esp_hal_radio::EspHalBluetoothInterruptRoutes,
    public_address: BluetoothPublicDeviceAddress,
    entropy: &'static BluetoothEntropy<'static>,
) {
    let Ok(parked) = BluetoothParked::new(partition, coex, interrupts) else {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-memory\r\n");
    };
    let (system, backend) =
        match oer_esp32s31_bluetooth_system::start(radio, parked, public_address).await {
            Ok(started) => started,
            Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-start\r\n"),
        };
    crate::console::runtime_log(format_args!(
        "OPEN_RADIO_HIL bluetooth=started at_micros={}",
        embassy_time::Instant::now().as_micros()
    ));
    let hci = start_bluetooth_hci(public_address, Some(VERSION), entropy);
    spawner
        .spawn(super::client(radio, system, backend, hci.service).expect("Bluetooth client task"));
    spawner.spawn(application(hci.host).expect("Bluetooth GATT task"));
}

#[embassy_executor::task]
async fn application(host: BluetoothHostTransport) {
    let stack = gatt::build(host);
    let mut runner = stack.runner();
    let state = Cell::new(BluetoothGattEvidence::default());
    let host = core::pin::pin!(runner.run());
    let application = core::pin::pin!(oer_bluetooth_gatt_trouble::gatt::run(&stack, |event| {
        gatt::observe(&state, event);
        EVIDENCE.lock(|evidence| evidence.set(Some(state.get())));
    }));
    match select(host, application).await {
        Either::First(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-host-stopped\r\n")
        }
        Either::Second(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-gatt-stopped\r\n")
        }
    }
}
