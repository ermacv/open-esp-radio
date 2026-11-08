//! The Wi-Fi access point on the S31 and its IP service: a WPA2-Personal
//! BSS of the image's settings on the radio the station would take, its
//! runner and its IP stack on the application executor, beside the
//! image's roots.
//!
//! [`AccessPoint::start`] runs in the image's boot. Once the executor runs,
//! the library's access-point task serves the BSS and the stack together;
//! an exit other than an ordinary stop records and resets.

use super::*;
use oer_chips_esp32s31_radio_ieee80211_station::access_point::{
    self as ap, AccessPointControl, AccessPointRunner, AccessPointStorage, ApExit,
};
use oer_portable_embassy_net::{
    NetworkInterfaceId, OwnedNetworkTxFrame,
    ip::{self, GeneralPool},
};
use oer_protocols_ieee80211_runtime::access_point::network::{ApNetDevice, ApNetTransmit};

pub use ap::{ApSettings, ApSettingsError};
pub use oer_protocols_ieee80211_runtime::access_point::ApStatus;

/// The access point's receive packets: each one 1514-byte Ethernet frame.
pub const AP_RX: usize = 4;

/// The packets the access point's stack makes: the stack writes each one,
/// the access point copies it into its BSS, and its memory is free again.
const AP_PACKETS: usize = 4;

/// The stack's sending path into the access point's BSS.
type ApTransmit = ApNetTransmit<'static, OwnedNetworkTxFrame>;

/// The access point's stack's executor context: the application's.
type ApContext = ThreadContext<0>;

type ApIpResources = ip::Resources<ApContext, AP_RX, ApTransmit>;
type ApIpService = ip::Service<ApContext, AP_RX, ApTransmit, ApNetDevice<'static>, 1>;

/// The IP stack's device of the access point, which sockets name.
pub type ApIpDevice =
    oer_portable_embassy_net::OwnedNetworkDevice<'static, ApContext, AP_RX, ApTransmit>;

/// The access point's IP stack.
pub type ApStack = embassy_net::Stack<'static, ApIpDevice>;

static AP_STORAGE: StaticCell<AccessPointStorage> = StaticCell::new();
static AP_IP: StaticCell<ApIpResources> = StaticCell::new();
static AP_GENERAL: StaticCell<GeneralPool<AP_PACKETS>> = StaticCell::new();
static AP_CONTROL: StaticCell<ip::Control<'static>> = StaticCell::new();

/// The access point's task: its runner and its IP service.
pub(crate) struct ApStart {
    runner: AccessPointRunner<binding::Binding>,
    service: ApIpService,
}

/// The access point: its control and its IP stack.
pub struct AccessPoint {
    control: AccessPointControl,
    stack: ApStack,
    net: &'static ip::Control<'static>,
    address: [u8; 6],
}

impl AccessPoint {
    /// Start the access point of `settings` on `wifi` at `address` (its
    /// static IPv4 configuration, no gateway) on the channel of `config`:
    /// the radio executor, the radio actor, the MAC driver, the access
    /// point's runner and its IP service. Its task runs once the executor
    /// does.
    ///
    /// # Errors
    /// The radio started already, or a binding, storage or the stack's
    /// configuration was refused.
    pub fn start(
        wifi: WifiPeripheral,
        config: Config,
        settings: ApSettings,
        address: ip::Ipv4Configuration,
        boot: &mut Boot,
    ) -> Result<Self, Error> {
        let started = start_radio(wifi, boot)?;
        let mac = started.resources.ap;
        let seed = started.seed;
        let storage = AP_STORAGE
            .try_uninit()
            .map(|place| place.write(const { AccessPointStorage::new() }))
            .ok_or(Error::Storage)?;
        let (control, device, transmit, runner) =
            wifi::access_point::access_point::<binding::Binding, OwnedNetworkTxFrame>(
                started.resources,
                config.chip,
                settings,
                storage,
                started.nonces,
            );
        let general = AP_GENERAL
            .try_uninit()
            .map(|place| place.write(const { GeneralPool::new() }))
            .ok_or(Error::Storage)?
            .allocator();
        let resources = AP_IP
            .try_uninit()
            .map(|place| place.write(const { ApIpResources::new() }))
            .ok_or(Error::Storage)?;
        let (net, service) = resources
            .start(
                device,
                transmit,
                None,
                general,
                NetworkInterfaceId::new(1),
                mac,
                seed,
                ip::Configuration::Static(address),
            )
            .map_err(Error::Net)?;
        let net: &'static ip::Control<'static> = AP_CONTROL.try_init(net).ok_or(Error::Storage)?;
        let stack = service.stack();
        boot.pending.access_point = Some(ApStart { runner, service });
        Ok(Self {
            control,
            stack,
            net,
            address: mac,
        })
    }

    /// The access point's status now.
    pub fn status(&self) -> ApStatus {
        self.control.status()
    }

    /// Wait for a status other than `seen`.
    pub async fn changed(&self, seen: ApStatus) -> ApStatus {
        self.control.changed(seen).await
    }

    /// Stop the access point: it tears its stations down and stops the
    /// radio.
    pub fn stop(&self) {
        self.control.stop();
    }

    /// The stack sockets of the access point's network open on.
    pub fn stack(&self) -> ApStack {
        self.stack
    }

    /// The IP service's link, IPv4 and lifecycle now.
    pub fn net_status(&self) -> Status {
        self.net.status()
    }

    /// The access point's own MAC address, its BSSID.
    pub fn address(&self) -> [u8; 6] {
        self.address
    }
}

/// The access point's task: the runner and the IP service together. An
/// ordinary stop ends it; every other exit records its cause and resets.
pub(crate) async fn serve_access_point(start: ApStart) {
    let ApStart {
        runner,
        mut service,
    } = start;
    let ip = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}));
    let exit = match select(runner.run(), ip).await {
        Either::First(exit) => exit,
        // The service ends only with a refused context.
        Either::Second(_) => crate::fail(oer_portable_diagnostics::name!("ap-net"; 32)),
    };
    match exit {
        ApExit::Stopped => {}
        ApExit::Unavailable(error) => failure(error),
        ApExit::ReleaseFailed(_) => crate::fail(oer_portable_diagnostics::name!("ap-release"; 32)),
        ApExit::StartRefused => crate::fail(oer_portable_diagnostics::name!("ap-start"; 32)),
        ApExit::StopUnconfirmed => crate::fail(oer_portable_diagnostics::name!("ap-stop"; 32)),
        ApExit::Faulted => crate::fail(oer_portable_diagnostics::name!("ap-faulted"; 32)),
        ApExit::Contract(_) => crate::fail(oer_portable_diagnostics::name!("ap-contract"; 32)),
    }
}
