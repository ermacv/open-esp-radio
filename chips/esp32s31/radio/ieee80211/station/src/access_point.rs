//! The S31 access point: the S31 data port under the access point's
//! runner (`oer_protocols_ieee80211_runtime::access_point`).
//!
//! The composition starts the radio and installs the port as the station's
//! composition does, gives the runner the installation's access-point
//! tokens, its tickets and the first [`AP_BANK`] transmit buffers, and runs
//! it with the access point's own engine. The station's tokens of the
//! installation stay unused: an access-point-only image runs no station.
//! Once the runner stopped the radio, the composition releases the radio's
//! membership as the station's does.

use crate::chip::{Binding, Config, InitializationError, Resources};
use crate::radio::{self, Port};
use oer_chips_esp32s31_radio_ieee80211_backend::port::{BufferClass, PortInstallation, TX_TICKETS};
use oer_chips_family_radio_ieee80211_policy::{access_point::ADVERTISEMENT, ccmp, lmac};
use oer_portable_embassy_time::EmbassyClock;
use oer_protocols_ieee80211_contracts_port::{GroupKeyId, PhyRate, TxBufferClass as _};
use oer_protocols_ieee80211_core_roles_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    beacon::ApBeacon, engine::ApEngine, security_policy::ApSecurityPolicy,
};
use oer_protocols_ieee80211_core_security_rsn::{PmkDerivation, frames::RsnGtk};
use oer_protocols_ieee80211_runtime::access_point::{
    AP_BUFFERS, AP_KEYS, ApEnd, ApNonces, ApPolicy, ApResources, ApRunner, ApShared, ApStatus,
    GROUP_KEY_ID,
    network::{ApNetDevice, ApNetTransmit, ApNetworkEnd, ApNetworkStorage},
};
use oer_protocols_ieee80211_wire::{
    beacon::AP_BEACON_CAPACITY, phy::LegacyRate, security::spp::Interface,
    sequence::SequenceNumber, ssid::WifiSsid,
};

/// The transmit buffers the access point holds: the port keeps them from
/// the pool's first places.
pub const AP_BANK: u8 = AP_BUFFERS as u8;

const _: () = {
    if AP_BUFFERS > oer_chips_esp32s31_radio_ieee80211_backend::port::TX_BUFFERS {
        panic!("the access point's buffers come from the transmit pool");
    }
    if AP_BUFFERS > u8::MAX as usize {
        panic!("the bank counts the access point's buffers in a u8");
    }
    if <BufferClass<{ crate::chip::TX }>>::CAPACITY < AP_BEACON_CAPACITY {
        panic!("a beacon fits one transmit buffer");
    }
};

/// The BSS the access point serves.
pub struct ApSettings {
    /// Its SSID.
    pub ssid: WifiSsid,
    /// Its WPA2-Personal passphrase: 8 to 63 characters.
    pub passphrase: heapless::Vec<u8, 63>,
    /// The beacon interval, in TU.
    pub beacon_interval_tu: u16,
    /// The DTIM period, in beacons.
    pub dtim_period: u8,
    /// The stations it admits at most.
    pub client_limit: AccessPointClientLimit,
}

/// Why the settings are not a BSS the access point serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSettingsError {
    /// The SSID is empty or longer than 32 octets.
    Ssid,
    /// The passphrase is not 8 to 63 characters.
    Passphrase,
    /// The client limit is zero or beyond the access point's peers.
    ClientLimit,
}

impl ApSettings {
    /// A WPA2-Personal BSS named `ssid` with `passphrase`, the vendor's
    /// default beacon interval of 100 TU and DTIM period of 1, admitting
    /// `clients` stations.
    ///
    /// # Errors
    ///
    /// The value that is not a BSS's.
    pub fn wpa2(ssid: &[u8], passphrase: &[u8], clients: u8) -> Result<Self, ApSettingsError> {
        let ssid = WifiSsid::new(ssid).map_err(|_| ApSettingsError::Ssid)?;
        if !(8..=63).contains(&passphrase.len()) {
            return Err(ApSettingsError::Passphrase);
        }
        let passphrase =
            heapless::Vec::from_slice(passphrase).map_err(|_| ApSettingsError::Passphrase)?;
        let client_limit =
            AccessPointClientLimit::new(clients).map_err(|_| ApSettingsError::ClientLimit)?;
        Ok(Self {
            ssid,
            passphrase,
            beacon_interval_tu: 100,
            dtim_period: 1,
            client_limit,
        })
    }
}

/// The storage of one access point, placed once by the image.
pub struct AccessPointStorage {
    peers: AccessPointPeerStorage,
    template: [u8; AP_BEACON_CAPACITY],
    network: ApNetworkStorage,
    shared: ApShared,
}

impl AccessPointStorage {
    pub const fn new() -> Self {
        Self {
            peers: AccessPointPeerStorage::new(),
            template: [0; AP_BEACON_CAPACITY],
            network: ApNetworkStorage::new(),
            shared: ApShared::new(),
        }
    }
}

impl Default for AccessPointStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// The application's end of the access point.
pub struct AccessPointControl {
    shared: &'static ApShared,
}

impl AccessPointControl {
    /// The access point's status now.
    pub fn status(&self) -> ApStatus {
        self.shared.status()
    }

    /// Wait for a status other than `seen`.
    pub async fn changed(&self, seen: ApStatus) -> ApStatus {
        self.shared.changed(seen).await
    }

    /// Ask the access point to stop: its runner tears its peers down and
    /// stops the radio.
    pub fn stop(&self) {
        self.shared.request_stop();
    }
}

/// The ANonces of the access point's handshakes.
pub struct Nonces(&'static mut crate::NonceSource);

impl ApNonces for Nonces {
    fn nonce(&mut self) -> Option<[u8; 32]> {
        self.0.next_nonce().ok()
    }
}

/// How the access point ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApExit {
    /// It stopped, and the radio was released.
    Stopped,
    /// It stopped, but the radio's release did not complete.
    ReleaseFailed(crate::failure::ReleaseCause),
    /// It never started.
    Unavailable(InitializationError),
    /// Its radio did not start.
    StartRefused,
    /// Its radio's stop was not confirmed; the port keeps what it holds.
    StopUnconfirmed,
    /// The port faulted; the runner keeps every owner.
    Faulted,
    /// The runner's own contract broke; it keeps every owner.
    Contract(oer_protocols_ieee80211_runtime::access_point::ApContract),
}

/// The access point's task: it owns the radio.
pub struct AccessPointRunner<W: Binding> {
    boot: radio::Boot<W>,
    settings: ApSettings,
    network: ApNetworkEnd<'static>,
    peers: &'static mut AccessPointPeerStorage,
    template: &'static mut [u8; AP_BEACON_CAPACITY],
    shared: &'static ApShared,
    nonces: Nonces,
}

/// The access point's network device and its sending path.
pub type ApNetwork = ApNetDevice<'static>;

/// Assemble the access point over `resources`: its control, its network
/// device and sending path for the stack's frames `F`, and its task.
pub fn access_point<W: Binding, F>(
    resources: Resources<W>,
    config: Config,
    settings: ApSettings,
    storage: &'static mut AccessPointStorage,
    nonces: &'static mut crate::NonceSource,
) -> (
    AccessPointControl,
    ApNetwork,
    ApNetTransmit<'static, F>,
    AccessPointRunner<W>,
) {
    let Resources {
        events: _,
        storage: station,
        hardware,
        delay_random,
        address,
        ap,
        bound,
        bell,
        entropy,
        interrupts,
        mac,
        receive,
        power,
        dma,
    } = resources;
    let crate::WifiHardware { cell, wifi } = hardware;
    let crate::DmaResources { rx, tx } = dma;
    let crate::dma::TxDma {
        pool,
        descriptors,
        chains,
    } = tx;
    let pool = station
        .pool
        .try_init(
            oer_chips_esp32s31_radio_ieee80211_backend::port::TxPool::pin(pool)
                .unwrap_or_else(|_| W::fail(InitializationError::Tx)),
        )
        .unwrap_or_else(|| W::fail(InitializationError::Storage));
    let AccessPointStorage {
        peers,
        template,
        network,
        shared,
    } = storage;
    let (device, transmit, end) = network.split::<F>();
    let shared: &'static ApShared = shared;
    (
        AccessPointControl { shared },
        device,
        transmit,
        AccessPointRunner {
            boot: radio::Boot {
                cell,
                wifi,
                delay_random,
                station: address,
                ap,
                bound,
                bell,
                config,
                entropy,
                interrupts,
                mac,
                receive,
                power,
                rx,
                pool,
                descriptors,
                chains,
                storage: &mut station.driver,
            },
            settings,
            network: end,
            peers,
            template,
            shared,
            nonces: Nonces(nonces),
        },
    )
}

impl<W: Binding> AccessPointRunner<W> {
    /// Run the access point until its stop, then release the radio.
    pub async fn run(self) -> ApExit {
        let Self {
            boot,
            settings,
            network,
            peers,
            template,
            shared,
            mut nonces,
        } = self;
        let config = boot.config;
        let Some(mut installations) = W::take_installations() else {
            return ApExit::Unavailable(InitializationError::Storage);
        };
        // The PMK of the passphrase, a step at a time between the
        // executor's other work.
        let Ok(mut derivation) = PmkDerivation::new(&settings.passphrase, settings.ssid.as_bytes())
        else {
            return ApExit::Unavailable(InitializationError::Ssid);
        };
        let pmk = loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
            embassy_futures::yield_now().await;
        };
        let Some(gtk_bytes) = nonces.nonce() else {
            return ApExit::Unavailable(InitializationError::Storage);
        };
        let mut gtk = [0; 16];
        for (octet, source) in gtk.iter_mut().zip(gtk_bytes) {
            *octet = source;
        }
        let (parts, driver) = match radio::install(boot, config, AP_BANK).await {
            Ok(installed) => installed,
            Err(error) => return ApExit::Unavailable(error),
        };
        let Some(minted) = installations.first() else {
            return ApExit::Unavailable(InitializationError::Registration);
        };
        let Ok(PortInstallation {
            port,
            commands,
            resources,
        }) = Port::<W>::install(parts, minted, driver)
        else {
            return ApExit::Unavailable(InitializationError::Backend);
        };
        let address = port_address::<W>(&port);
        let channel = config.supported;
        let mut tickets = heapless::Vec::new();
        for ticket in resources.tickets {
            // The access point holds every ticket: `TX_TICKETS` is its
            // `AP_TICKETS`.
            if let Err(ticket) = tickets.push(ticket) {
                drop(ticket);
            }
        }
        let mut buffers = heapless::Vec::new();
        for buffer in resources.buffers.into_iter().take(AP_BUFFERS) {
            if let Err(buffer) = buffers.push(buffer) {
                drop(buffer);
            }
        }
        let (start, bank) = resources.access_point_association.into_parts();
        let (pairwise_slots, group_slots) = bank.into_parts();
        let mut pairwise = heapless::Vec::new();
        for slot in pairwise_slots {
            if pairwise.len() < AP_KEYS && pairwise.push(slot).is_err() {
                break;
            }
        }
        let [group, ..] = group_slots;
        let ap_resources = ApResources::<'static, Port<W>, BufferClass<{ crate::chip::TX }>> {
            tickets,
            access: resources.terminal_access,
            buffers,
            configure: resources.access_point_commands.configure.with_context(
                oer_protocols_ieee80211_core_roles_sta::machine::StaConfigurationContext::detached(
                ),
            ),
            association: start,
            pairwise,
            group,
            group_permission: resources.access_point_group_permission,
            beacon: resources.access_point_beacon,
        };
        let Some(key_id) = GroupKeyId::new(GROUP_KEY_ID) else {
            return ApExit::Unavailable(InitializationError::Storage);
        };
        let Ok(service) = AccessPointService::<Interface>::new(
            address,
            pmk,
            RsnGtk::new(key_id, true, &gtk),
            1,
            settings.client_limit,
            AccessPointInactiveTimeout::default(),
            peers.lend(),
        ) else {
            return ApExit::Unavailable(InitializationError::Storage);
        };
        let wire_channel = channel.channel();
        let Ok(beacon) = ApBeacon::new(
            template,
            &ADVERTISEMENT,
            address,
            &settings.ssid,
            wire_channel,
            settings.beacon_interval_tu,
            settings.dtim_period,
            SequenceNumber::ZERO,
            ApSecurityPolicy::Wpa2Personal,
        ) else {
            return ApExit::Unavailable(InitializationError::Ssid);
        };
        let engine = ApEngine::new(service, beacon, ADVERTISEMENT, wire_channel, key_id);
        let (Some(management_transmissions), Some(data_transmissions)) = (
            core::num::NonZeroU8::new(4),
            core::num::NonZeroU8::new(lmac::SHORT_RETRY_LIMIT),
        ) else {
            return ApExit::Unavailable(InitializationError::Tx);
        };
        let policy = ApPolicy {
            // The BSS's lowest basic rate, the archive's beacon rate.
            management_rate: PhyRate::Legacy(LegacyRate::Dsss1M),
            management_transmissions,
            data_transmissions,
            packet_numbers: ccmp::TX_PACKET_NUMBER_STEP,
            group_generation: 1,
        };
        let runner = ApRunner::new(
            port,
            commands,
            ap_resources,
            engine,
            policy,
            channel,
            gtk,
            network,
            || (),
            nonces,
        );
        match runner.run(shared, &EmbassyClock).await {
            ApEnd::Stopped(mut runner) => match radio::release::<W>(runner.port_mut()).await {
                Ok(()) => ApExit::Stopped,
                Err(cause) => {
                    // The runner keeps every owner of the unreleased radio.
                    let _kept = core::mem::ManuallyDrop::new(runner);
                    ApExit::ReleaseFailed(cause)
                }
            },
            ApEnd::StartRefused => ApExit::StartRefused,
            ApEnd::StopUnconfirmed(_) => ApExit::StopUnconfirmed,
            ApEnd::Faulted => ApExit::Faulted,
            ApEnd::Contract(contract) => ApExit::Contract(contract),
        }
    }
}

/// The access point's own address on `port`.
fn port_address<W: Binding>(port: &Port<W>) -> [u8; 6] {
    oer_protocols_ieee80211_contracts_port::AccessPointPort::access_point_address(port).get()
}

const _: () = {
    if TX_TICKETS > oer_protocols_ieee80211_runtime::access_point::AP_TICKETS {
        panic!("the access point holds every ticket of the installation");
    }
};
