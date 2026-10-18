//! The plaintext Trouble GATT application over the HCI Controller.
//!
//! The Trouble Host owns HCI and ATT over the Host end of the transport; the
//! HIL console only reads the value-only observations the application
//! reports. The `wifi-ble-coex` image runs the same Host and application and
//! folds the same observations for the Wi-Fi console.

use core::cell::Cell;

#[cfg(feature = "bluetooth-gatt")]
use oer_bluetooth_gatt_trouble::gatt;
use oer_bluetooth_gatt_trouble::gatt::Observation;
use oer_bluetooth_hci::bt_hci::controller::ExternalController;
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
#[cfg(feature = "bluetooth-gatt")]
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::bluetooth::BluetoothGattEvidence;
#[cfg(feature = "bluetooth-gatt")]
use oer_hil_protocol::bluetooth::{GattState, GetGatt};
use static_cell::StaticCell;
use trouble_host::prelude::*;

#[cfg(feature = "bluetooth-gatt")]
use super::console;

/// Command slots the Host keeps in flight.
const COMMAND_SLOTS: usize = 4;

pub(super) type Controller = ExternalController<BluetoothHostTransport, COMMAND_SLOTS>;

static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 3>> = StaticCell::new();

#[cfg(feature = "bluetooth-gatt")]
struct Profile(Cell<BluetoothGattEvidence>);

#[cfg(feature = "bluetooth-gatt")]
oer_hil_agent::requests! {
    /// The Trouble GATT image's requests.
    pub(super) enum Request (sessions = false) {
        Get(GetGatt),
    }
}

#[cfg(feature = "bluetooth-gatt")]
impl console::Profile for Profile {
    type Request = Request;

    fn queue(&self) -> &'static console::Queue<Request> {
        static QUEUE: console::Queue<Request> = console::Queue::new();
        &QUEUE
    }

    fn maximum_payload_bytes(&self) -> u16 {
        0
    }

    async fn serve(&self, request: RequestIdentity, body: Request) {
        match body {
            Request::Get(GetGatt) => {
                let mut value = self.0.get();
                value.cpu0_stack = Some(crate::cpu0_stack_usage_snapshot());
                console::respond(request, Ok(GattState(value))).await;
            }
        }
    }
}

#[cfg(feature = "bluetooth-gatt")]
#[embassy_executor::task]
pub(super) async fn task(host: BluetoothHostTransport, usb: crate::transport::Usb, boot: u64) {
    let stack = build(host);
    let mut runner = stack.runner();
    let profile = Profile(Cell::new(BluetoothGattEvidence::default()));
    let host = core::pin::pin!(runner.run());
    let application = core::pin::pin!(gatt::run(&stack, |event| observe(&profile.0, event)));
    let console = core::pin::pin!(console::serve_console(usb, boot, &profile));
    let requests = core::pin::pin!(console::serve_requests(&profile));
    match embassy_futures::select::select4(host, application, console, requests).await {
        embassy_futures::select::Either4::First(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-host-stopped\r\n")
        }
        embassy_futures::select::Either4::Second(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-gatt-stopped\r\n")
        }
        embassy_futures::select::Either4::Third(never)
        | embassy_futures::select::Either4::Fourth(never) => match never {},
    }
}

// Host construction materializes security-manager and connection state
// beyond the caller-owned resource arrays; keep it out of the task frame.
#[inline(never)]
pub(super) fn build(host: BluetoothHostTransport) -> Stack<'static, Controller, DefaultPacketPool> {
    let resources = RESOURCES.init_with(HostResources::new);
    trouble_host::new(Controller::new(host), resources).build()
}

pub(super) fn observe(state: &Cell<BluetoothGattEvidence>, event: Observation) {
    let mut value = state.get();
    let increment =
        |counter: &mut u32| *counter = counter.checked_add(1).expect("GATT evidence exhausted");
    match event {
        Observation::Ready(address) => value.address = Some(address),
        Observation::Advertising => {
            value.advertising = true;
            increment(&mut value.advertising_starts);
        }
        Observation::Connected => {
            value.advertising = false;
            value.connected = true;
            increment(&mut value.connections);
        }
        Observation::Read(current) => {
            value.value = current;
            increment(&mut value.reads);
        }
        Observation::Written(current) => {
            value.value = current;
            increment(&mut value.writes);
        }
        Observation::Disconnected(reason) => {
            value.connected = false;
            value.last_disconnect_reason = Some(reason);
            increment(&mut value.disconnections);
        }
    }
    state.set(value);
}
