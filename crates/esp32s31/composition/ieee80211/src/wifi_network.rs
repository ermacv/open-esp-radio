//! One production network scheduler for one permanent logical Wi-Fi device.

#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;

use crate::{WifiDevice, WifiStackResources};

#[cfg(feature = "owned-network")]
type NetworkRunner<'resources> = embassy_net::Runner<'resources>;

/// Eternal `embassy-net` execution obligation for one Wi-Fi device.
pub struct WifiNetworkRunner<'resources> {
    inner: NetworkRunner<'resources>,
}

impl WifiNetworkRunner<'_> {
    pub async fn run(mut self) -> ! {
        // The IP runner and socket owners share one executor. Bound each
        // ingress/egress turn so a continuously replenished device queue
        // cannot starve UDP/TCP consumers in one unbounded poll.
        self.inner.run().await
    }
}

impl<'resources> WifiNetworkRunner<'resources> {
    /// Construct the role-neutral IP stack, its Wi-Fi interface and the sole
    /// production runner.
    ///
    /// IP policy and socket capacity remain application choices: configure
    /// addresses, routes and DHCPv4 through the returned interface. The same
    /// stack owner is used by station and access-point epochs.
    pub fn new(
        device: WifiDevice,
        resources: &'resources mut WifiStackResources<'resources>,
        random_seed: u64,
    ) -> (embassy_net::iface::Iface<'resources>, Self) {
        let WifiStackResources {
            stack,
            device: device_slot,
        } = resources;
        #[cfg(feature = "owned-network")]
        let (stack, mut inner) =
            embassy_net::Stack::new(stack, random_seed, device.packet_allocator);
        #[cfg(feature = "owned-network")]
        inner.set_poll_budget(embassy_net::PollBudget::new(32, 32));
        let iface = stack
            .add_iface_borrowed(device_slot.write(device.inner))
            .expect("a new stack has room for its Wi-Fi interface");

        (iface, Self { inner })
    }
}
