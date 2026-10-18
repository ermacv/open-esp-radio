//! Owned-packet Xarxa/Embassy network composition.
#[cfg(feature = "task-poll-telemetry")]
use super::{observation, progress};

use core::mem::MaybeUninit;

use oer_esp32s31_ieee80211_system::{WifiDevice, WifiNetworkDevice};

use oer_hil_protocol::{
    wifi::WifiNetworkInterface, wifi::WifiRxChecksumPolicy, wifi::WifiTxUdpChecksumPolicy,
};
#[cfg(feature = "task-poll-telemetry")]
type Device = progress::Device<WifiNetworkDevice>;
#[cfg(not(feature = "task-poll-telemetry"))]
type Device = WifiNetworkDevice;
#[cfg(feature = "owned-network")]
pub(crate) type Runner<'a> = embassy_net::Runner<'a>;
/// One role's IP stack state and the device the stack borrows.
#[cfg(feature = "owned-network")]
pub(crate) struct Resources {
    stack: embassy_net::StackStorage<'static>,
    device: MaybeUninit<Device>,
}
#[cfg(feature = "owned-network")]
impl Resources {
    pub(crate) const fn new() -> Self {
        Self {
            stack: embassy_net::StackStorage::new(),
            device: MaybeUninit::uninit(),
        }
    }
}

use oer_hil_agent::network::embassy_ipv4 as ipv4;

pub(crate) use ipv4::{Iface, configure, info};
pub(crate) fn new(
    device: WifiDevice,
    resources: &'static mut Resources,
    settings: super::Settings,
    _role: WifiNetworkInterface,
) -> (Iface<'static>, Runner<'static>) {
    let device = device
        .with_software_ipv4_udp_rx_checksum_validation(
            settings.rx_checksum == WifiRxChecksumPolicy::Software,
        )
        .with_software_ipv4_udp_tx_checksum_generation(
            settings.tx_udp_checksum == WifiTxUdpChecksumPolicy::Software,
        );
    #[cfg(feature = "owned-network")]
    let (device, allocator) = device.into_owned();
    #[cfg(feature = "task-poll-telemetry")]
    let device = progress::Device::new(device, observation::counters(_role));
    let Resources {
        stack,
        device: device_slot,
    } = resources;
    #[cfg(feature = "owned-network")]
    let (stack, mut runner) = embassy_net::Stack::new(stack, settings.seed, allocator);
    #[cfg(feature = "owned-network")]
    runner.set_poll_budget(embassy_net::PollBudget::new(32, 32));
    let iface = Iface(
        stack
            .add_iface_borrowed(device_slot.write(device))
            .expect("a new stack has room for its Wi-Fi interface"),
    );
    configure(iface, settings.ipv4);
    (iface, runner)
}
