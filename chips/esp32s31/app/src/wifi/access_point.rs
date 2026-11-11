//! The Wi-Fi access point on the S31 and its IP service: a WPA2-Personal
//! BSS of the image's settings on the radio the station would take, its
//! runner and its IP stack on the application executor, beside the
//! image's roots.
//!
//! [`AccessPoint::start`] runs in the image's boot; the application then
//! names the channel and the BSS once with [`AccessPoint::serve`]. Once the
//! executor runs, the library's access-point task serves the BSS and the
//! stack together; every exit keeps its typed cause and resets nothing.

use super::*;
use oer_chips_esp32s31_radio_ieee80211_station::access_point::{
    self as ap, AccessPointControl, AccessPointRunner, AccessPointStorage,
};
use oer_portable_embassy_net::{
    NetworkInterfaceId, OwnedNetworkTxFrame,
    ip::{self, GeneralPool},
};
use oer_protocols_ieee80211_runtime::access_point::network::{ApNetDevice, ApNetTransmit};

pub use ap::{ApExit, ApSettings, ApSettingsError, StartRefused};
pub use oer_protocols_ieee80211_runtime::access_point::ApStatus;
pub use oer_protocols_ieee80211_runtime::access_point::ApUnsupported;

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

/// How the access point's installation ended: its exit, or the
/// installation that did not retire, whole
/// ([`AccessPoint::installation_end`]).
pub type ApInstallationEnd = ap::AccessPointEnd<binding::Binding>;

static AP_END: StaticCell<EndHome<ApInstallationEnd>> = StaticCell::new();

/// The access point's task: its runner, its IP service and the home its
/// end goes to.
pub(crate) struct ApStart {
    runner: AccessPointRunner<binding::Binding>,
    service: ApIpService,
    home: EndSender<ApInstallationEnd>,
}

/// The access point: its control and its IP stack.
pub struct AccessPoint {
    control: AccessPointControl,
    /// Where the access point's task leaves how its installation ended.
    end: Option<EndReceiver<ApInstallationEnd>>,
    stack: ApStack,
    net: &'static ip::Control<'static>,
    address: [u8; 6],
}

impl AccessPoint {
    /// Prepare the access point on `wifi` with `address` as its static
    /// IPv4 configuration: the radio executor, the radio actor, the access
    /// point's runner and its IP service. Its task runs once the executor
    /// does and waits for [`Self::serve`]; `config` is the regulatory
    /// configuration until then.
    ///
    /// # Errors
    /// The radio started already, or a binding, storage or the stack's
    /// configuration was refused.
    pub fn start(
        wifi: WifiPeripheral,
        config: Config,
        address: ip::Ipv4Configuration,
        boot: &mut Boot,
    ) -> Result<Self, Error> {
        let (home, end) = end_home(&AP_END)?;
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
        boot.pending.access_point = Some(ApStart {
            runner,
            service,
            home,
        });
        Ok(Self {
            control,
            end: Some(end),
            stack,
            net,
            address: mac,
        })
    }

    /// Serve the BSS of `settings` on the channel of `config`: the runner
    /// derives the PMK, installs the radio and beacons. An access point
    /// serves once.
    ///
    /// # Errors
    /// The radio cannot beacon ([`StartRefused::Unsupported`], checked
    /// before anything starts, the radio included), or it serves already;
    /// the settings come back either way.
    pub fn serve(&self, config: Config, settings: ApSettings) -> Result<(), StartRefused> {
        self.control.start(config.chip, settings)
    }

    /// The access point's status now.
    pub fn status(&self) -> ApStatus {
        self.control.status()
    }

    /// How the access point ended, once it did: an ordinary stop, or a
    /// command the port refused, after which the radio was released.
    pub fn exit(&self) -> Option<ApExit> {
        self.control.exit()
    }

    /// How the access point's installation ended, once its task ended: its
    /// exit, or the installation that did not retire, whole
    /// ([`ap::AccessPointEnd::Unfinished`]: the runner with its port, the
    /// items a port call could not keep, the cleanup's receipt, the exit
    /// as its cause and the phase), which the application holds from
    /// here. `None` once taken.
    pub async fn installation_end(&mut self) -> Option<ApInstallationEnd> {
        ended(&mut self.end).await
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

/// The access point's task: the runner and the IP service together. Every
/// end of the access point keeps its typed cause in [`AccessPoint::exit`]
/// and resets nothing: a completed retire returned its owners and released
/// the radio; until its stop is confirmed and its release ended, the
/// runner keeps every owner in its task and the exit says how far the end
/// came; an installation that did not retire goes whole to the
/// application ([`AccessPoint::installation_end`]); a refused install
/// names its cause, its started radio released. An IP service that ends
/// stops the access point, its cause in the service's status.
pub(crate) async fn serve_access_point(start: ApStart) {
    let ApStart {
        runner,
        mut service,
        home,
    } = start;
    let ip = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}));
    let stop = runner.stop_handle();
    let mut run = core::pin::pin!(runner.run());
    let ended = match select(run.as_mut(), ip).await {
        Either::First(ended) => ended,
        Either::Second(_) => {
            // The stack's end is in its status; the access point stops.
            stop.request_stop();
            run.await
        }
    };
    super::leave(home, ended).await;
}
