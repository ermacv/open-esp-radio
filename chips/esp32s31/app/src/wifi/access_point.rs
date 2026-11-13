//! The Wi-Fi access point on the S31: a WPA2-Personal BSS of the image's
//! settings on the radio the station would take, its runner on the
//! application executor beside the image's roots, and its network end for
//! the group, where [`super::Network::start`] takes it.
//!
//! [`AccessPoint::start`] runs in the image's boot; the application then
//! names the channel and the BSS once with [`AccessPoint::serve`]. Once the
//! executor runs, the library's access-point task serves the BSS; every
//! exit keeps its typed cause and resets nothing. The access point's
//! network end is the station's ([`super::Device`]): the group starts a
//! network over it with a given address ([`super::Addressing::Given`]).

use super::*;
use oer_chips_esp32s31_radio_ieee80211_station::access_point::{
    self as ap, AccessPointControl, AccessPointRunner, AccessPointStorage,
};

pub use ap::{ApExit, ApSettings, ApSettingsError, StartRefused};
pub use oer_protocols_ieee80211_runtime::access_point::ApStatus;
pub use oer_protocols_ieee80211_runtime::access_point::ApUnsupported;

// With the network on hart 1, one signal wakes the access point's runner:
// hart 1's network raises it through `FromPeerHart`, the runner's own
// queue ends through `FromOwnTask`. The network's own wake is the
// station's (`NETWORK`): one task on hart 1 serves the group's networks.
#[cfg(feature = "group")]
oer_chips_family_platform_runtime::placed_static! {
    static ACCESS_POINT_SIGNAL: IrqSignal<wifi::DriverExecutor> = IrqSignal::new::<(oer_chips_family_platform_interrupt_table::signal::FromPeerHart<super::super::hart1::Hart1Executor, super::super::Hart1WakeToken>, FromOwnTask<wifi::DriverExecutor, WakeToken>), WakeToken>(), in critical_data "hart_peers";
    static ACCESS_POINT_RUNNER: oer_chips_family_platform_interrupt_table::signal::PeerNotify<wifi::DriverExecutor> = oer_chips_family_platform_interrupt_table::signal::PeerNotify::new(&ACCESS_POINT_SIGNAL), in critical_data "hart_peers";
}

static AP_STORAGE: StaticCell<AccessPointStorage<binding::Binding>> = StaticCell::new();

/// How the access point's installation ended: its exit, or the
/// installation that did not retire, whole
/// ([`AccessPoint::installation_end`]).
pub type ApInstallationEnd = ap::AccessPointEndOwner<binding::Binding>;

static AP_END: StaticCell<EndHome<ApInstallationEnd>> = StaticCell::new();

/// The access point's task: its runner and the home its end goes to.
pub(crate) struct ApStart {
    runner: AccessPointRunner<binding::Binding>,
    home: EndSender<ApInstallationEnd>,
}

/// The access point: its control.
pub struct AccessPoint {
    control: AccessPointControl,
    /// Where the access point's task leaves how its installation ended.
    end: Option<EndReceiver<ApInstallationEnd>>,
    address: [u8; 6],
}

impl AccessPoint {
    /// Prepare the access point on `wifi`: the radio executor, the radio
    /// actor and the access point's runner, and its network device for the
    /// group. Its task runs once the executor does and waits for
    /// [`Self::serve`]; `config` is the regulatory configuration until then.
    ///
    /// # Errors
    /// The radio started already, or a binding or storage was refused.
    pub fn start(wifi: WifiPeripheral, config: Config, boot: &mut Boot) -> Result<Self, Error> {
        let (home, end) = end_home(&AP_END)?;
        let started = start_radio(wifi, boot)?;
        let mac = started.resources.ap;
        let seed = started.seed;
        let storage = AP_STORAGE
            .try_uninit()
            .map(|place| {
                #[cfg(not(feature = "group"))]
                let storage =
                    place.write(const { AccessPointStorage::new(wifi::NetworkChannels::new()) });
                #[cfg(feature = "group")]
                let storage = place.write(
                    const {
                        AccessPointStorage::new(wifi::NetworkChannels::across_harts(
                            &NETWORK,
                            &ACCESS_POINT_RUNNER,
                        ))
                    },
                );
                storage
            })
            .ok_or(Error::Storage)?;
        let (control, network, transmit, stopper, runner) =
            wifi::access_point::access_point::<binding::Binding>(
                started.resources,
                config.chip,
                storage,
                started.nonces,
            );
        boot.pending.access_point = Some(ApStart { runner, home });
        boot.access_point_device = Some(Device {
            network: Some((network, transmit, stopper)),
            address: mac,
            seed,
        });
        Ok(Self {
            control,
            end: Some(end),
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

    /// Stop the access point: it tears its stations down, stops the radio,
    /// then its network.
    pub fn stop(&self) {
        self.control.stop();
    }

    /// The access point's own MAC address, its BSSID.
    pub fn address(&self) -> [u8; 6] {
        self.address
    }
}

/// The access point's task: the runner. Every end of the access point keeps
/// its typed cause in [`AccessPoint::exit`] and resets nothing: a completed
/// retire returned its owners and released the radio; until its stop is
/// confirmed and its release ended, the runner keeps every owner in its
/// task and the exit says how far the end came; an installation that did
/// not retire goes whole to the application
/// ([`AccessPoint::installation_end`]); a refused install names its cause,
/// its started radio released.
pub(crate) async fn serve_access_point(start: ApStart) {
    let ApStart { runner, home } = start;
    let ended = runner.run().await;
    super::leave(home, ended).await;
}
