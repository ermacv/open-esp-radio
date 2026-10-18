//! Role-neutral network and aggregate-TX storage for the sole Wi-Fi runner.
//!
//! STA and AP own distinct RX/link/MAC endpoints and share one tagged physical
//! TX arena. All of that allocation belongs to the integration root, not to a
//! station `connected` transaction.

#[cfg(feature = "owned-network")]
use core::mem::MaybeUninit;
#[cfg(feature = "tx-psram-dma-probe")]
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use oer_memory::zeroed::ZeroedStatic;

use crate::resources::profile::{
    ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY as NETWORK_FRAME_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH as NETWORK_TX_QUEUE_DEPTH,
    ESP32S31_DEFAULT_NETWORK_TX_TRAILER as NETWORK_TX_TRAILER,
    ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
};
#[cfg(feature = "owned-network")]
use crate::resources::profile::{
    ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH as NETWORK_OWNER_TX_QUEUE_DEPTH,
    ESP32S31_DEFAULT_NETWORK_PACKET_POOL_CAPACITY as NETWORK_PACKET_POOL_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_RX_PACKET_POOL_CAPACITY as NETWORK_RX_PACKET_POOL_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH as NETWORK_RX_QUEUE_DEPTH,
};
#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;
#[cfg(feature = "owned-network")]
use embassy_net_owned::driver::{PacketBufAllocator, PacketPool, PacketPoolStorage};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(feature = "owned-network")]
use oer_embassy_net_owned::{OwnedEndpointResources, OwnedNetworkDevice, OwnedNetworkTxFrame};

use oer_esp32s31_ieee80211_dma::tx_ampdu_storage::AmpduDmaStorage;
#[cfg(feature = "owned-network")]
use oer_esp32s31_ieee80211_runtime::datapath::owned::DualOwnedDatapathNetwork;

use oer_esp32s31_ieee80211_runtime::datapath::{
    PinnedTxConsumer, PinnedTxFrame, PinnedTxPool, PinnedTxResources,
    tx::resources::AggregateTxResources,
};

use oer_esp32s31_ieee80211_mac::tx::ampdu::{
    HtAmpduTxError, HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage,
    TX_AMPDU_METADATA_SIZE,
};

use oer_ieee80211_runtime::station_network::{RunningStationNetwork, StationNetworkResources};

use static_cell::{ConstStaticCell, StaticCell};

pub(super) const NETWORK_TX_HEADROOM: usize =
    TX_AMPDU_METADATA_SIZE + oer_ieee80211_mac::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;
// The protected MPDU starts immediately after the aggregate metadata and must
// remain naturally aligned for the hardware TX path.
const _: () = assert!(TX_AMPDU_METADATA_SIZE.is_multiple_of(core::mem::align_of::<u32>()));
pub(super) const TX_AMPDU_BUFFER_SIZE: usize = 0;

#[cfg(feature = "owned-network")]
type NetworkResources = OwnedEndpointResources<
    CriticalSectionRawMutex,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_OWNER_TX_QUEUE_DEPTH,
>;
type NetworkTxResources = PinnedTxResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkTxPool = PinnedTxPool<
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type PhysicalTxConsumer = PinnedTxConsumer<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;

pub(super) type RadioTxBacking = PinnedTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
#[cfg(feature = "owned-network")]
pub(super) type RadioNetworkTxBacking = OwnedNetworkTxFrame<'static, CriticalSectionRawMutex>;
type RadioAmpduRetention = RetainedAmpduDmaStorage<RadioTxBacking, TX_AMPDU_FRAME_COUNT>;

#[cfg(feature = "owned-network")]
pub type WifiNetworkDevice = OwnedNetworkDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_OWNER_TX_QUEUE_DEPTH,
>;

/// Application-side Wi-Fi device plus the general packet allocator consumed
/// exactly once when Embassy constructs its Xarxa stack.
pub struct WifiDevice {
    pub(crate) inner: WifiNetworkDevice,
    #[cfg(feature = "owned-network")]
    pub(crate) packet_allocator: PacketBufAllocator,
}

#[cfg(feature = "owned-network")]
impl WifiDevice {
    /// Transfer the unique owned-packet device and its matching allocator together.
    pub fn into_owned(self) -> (WifiNetworkDevice, PacketBufAllocator) {
        (self.inner, self.packet_allocator)
    }
}

impl WifiDevice {
    /// Select software IPv4/UDP validation for received packets.
    pub fn with_software_ipv4_udp_rx_checksum_validation(mut self, enabled: bool) -> Self {
        use embassy_net::driver::ChecksumCapabilities;

        // Without software validation the device is trusted to have verified
        // received IPv4 and UDP checksums.
        let mut checksum = ChecksumCapabilities::default();
        checksum.ipv4.rx = !enabled;
        checksum.udp.rx = !enabled;
        self.inner = self.inner.with_checksum_capabilities(checksum);
        self
    }

    /// Select software generation of IPv4 UDP checksums.
    pub fn with_software_ipv4_udp_tx_checksum_generation(mut self, enabled: bool) -> Self {
        use embassy_net::driver::Driver as _;

        // Without software generation the device fills in UDP checksums.
        let mut checksum = self.inner.capabilities().checksum;
        checksum.udp.tx = !enabled;
        self.inner = self.inner.with_checksum_capabilities(checksum);
        self
    }
}

/// Storage for one Wi-Fi IP stack: the stack state and the device it borrows.
#[cfg(feature = "owned-network")]
pub struct WifiStackResources<'d> {
    pub(crate) stack: embassy_net::StackStorage<'d>,
    pub(crate) device: core::mem::MaybeUninit<WifiNetworkDevice>,
}

#[cfg(feature = "owned-network")]
impl WifiStackResources<'_> {
    pub const fn new() -> Self {
        Self {
            stack: embassy_net::StackStorage::new(),
            device: core::mem::MaybeUninit::uninit(),
        }
    }
}

#[cfg(feature = "owned-network")]
impl Default for WifiStackResources<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Permanent application-side devices for the two logical Wi-Fi interfaces.
/// Each device owns independent IP/link/RX state while both publish into the
/// one physical tagged TX fabric.
pub struct WifiDevices {
    pub station: WifiDevice,
    pub access_point: WifiDevice,
}
#[cfg(feature = "owned-network")]
pub(super) type RadioNetworkRunner = DualOwnedDatapathNetwork<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_OWNER_TX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
pub(super) type NetworkRunner = &'static mut RadioNetworkRunner;
pub(super) type RadioAmpduStorage =
    AggregateTxResources<'static, RadioTxBacking, TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>;
pub type WifiNetworkResources = StationNetworkResources<(), NetworkRunner, ()>;
pub(super) type RunningWifiNetwork = RunningStationNetwork<(), NetworkRunner>;

static NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
static ACCESS_POINT_NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
static NETWORK_TX_RESOURCES: ConstStaticCell<NetworkTxResources> =
    ConstStaticCell::new(NetworkTxResources::new());
static NETWORK_RUNNER: StaticCell<RadioNetworkRunner> = StaticCell::new();

/// xarxa's packet payload storage in a region the boot zeroes.
#[cfg(feature = "owned-network")]
struct ZeroedPacketStorage<const COUNT: usize>(PacketPoolStorage<COUNT>);

#[cfg(feature = "owned-network")]
#[allow(unsafe_code, reason = "xarxa's packet storage accepts any bytes")]
// REVIEWED-LAYOUT: xarxa-driver 9e0e3293
// SAFETY: at the pinned xarxa revision `PacketPoolStorage<COUNT>` is
// `[UnsafeCell<MaybeUninit<Data>>; COUNT]`, valid for any bytes; its `new`
// builds zero bytes, and the pool writes a slot before reading it.
unsafe impl<const COUNT: usize> bytemuck::Zeroable for ZeroedPacketStorage<COUNT> {}

oer_memory::zeroed_static! {
    #[cfg(feature = "owned-network")]
    /// Placement: network packet payloads are explicitly placed in the PSRAM ownership tier.
    static STATION_NETWORK_PACKET_STORAGE: ZeroedStatic<ZeroedPacketStorage<NETWORK_PACKET_POOL_CAPACITY>> = zeroed in ".psram.bss.open_radio_station_network_packets";
    #[cfg(feature = "owned-network")]
    /// Placement: network packet payloads are explicitly placed in the PSRAM ownership tier.
    static ACCESS_POINT_NETWORK_PACKET_STORAGE: ZeroedStatic<ZeroedPacketStorage<NETWORK_PACKET_POOL_CAPACITY>> = zeroed in ".psram.bss.open_radio_ap_network_packets";
    #[cfg(feature = "owned-network")]
    /// Placement: RX packet payloads are explicitly separated from the physical DMA staging tier.
    static STATION_RX_PACKET_STORAGE: ZeroedStatic<ZeroedPacketStorage<NETWORK_RX_PACKET_POOL_CAPACITY>> = zeroed in ".psram.bss.open_radio_station_rx_packets";
    #[cfg(feature = "owned-network")]
    /// Placement: RX packet payloads are explicitly separated from the physical DMA staging tier.
    static ACCESS_POINT_RX_PACKET_STORAGE: ZeroedStatic<ZeroedPacketStorage<NETWORK_RX_PACKET_POOL_CAPACITY>> = zeroed in ".psram.bss.open_radio_ap_rx_packets";

    #[cfg(feature = "owned-network")]
    /// Placement: hot packet-pool ownership metadata is explicitly retained in internal SRAM.
    static STATION_NETWORK_PACKET_POOL: ZeroedStatic<MaybeUninit<PacketPool<NETWORK_PACKET_POOL_CAPACITY>>> = zeroed in ".critical.bss.open_radio_station_network_pool";
    #[cfg(feature = "owned-network")]
    /// Placement: hot packet-pool ownership metadata is explicitly retained in internal SRAM.
    static ACCESS_POINT_NETWORK_PACKET_POOL: ZeroedStatic<MaybeUninit<PacketPool<NETWORK_PACKET_POOL_CAPACITY>>> = zeroed in ".critical.bss.open_radio_ap_network_pool";
    #[cfg(feature = "owned-network")]
    /// Placement: hot packet-pool ownership metadata is explicitly retained in internal SRAM.
    static STATION_RX_PACKET_POOL: ZeroedStatic<MaybeUninit<PacketPool<NETWORK_RX_PACKET_POOL_CAPACITY>>> = zeroed in ".critical.bss.open_radio_station_rx_pool";
    #[cfg(feature = "owned-network")]
    /// Placement: hot packet-pool ownership metadata is explicitly retained in internal SRAM.
    static ACCESS_POINT_RX_PACKET_POOL: ZeroedStatic<MaybeUninit<PacketPool<NETWORK_RX_PACKET_POOL_CAPACITY>>> = zeroed in ".critical.bss.open_radio_ap_rx_pool";
    /// Placement: the linker must retain production network TX backing in DMA-visible SRAM.
    static NETWORK_TX_POOL: ZeroedStatic<NetworkTxPool> = zeroed in ".dma.bss.open_radio_network_tx";
}

#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_PROBE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_PREPARES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "tx-psram-dma-probe")]
static DIRECT_PSRAM_TX_DMA_LAST_ADDRESS: AtomicU32 = AtomicU32::new(0);
oer_memory::zeroed_static! {
    #[cfg(feature = "tx-psram-dma-probe")]
    /// Placement: the diagnostic pool must occupy the cached PSRAM aperture before its explicit cache writeback.
    static PSRAM_NETWORK_TX_POOL: ZeroedStatic<NetworkTxPool> =
        zeroed in ".psram.bss.open_radio_network_tx_dma_probe";
}

/// Select a same-image experiment where Wi-Fi A-MPDU descriptors reference
/// PSRAM packet buffers directly. Descriptors remain in internal SRAM.
#[cfg(feature = "tx-psram-dma-probe")]
pub fn configure_direct_psram_tx_dma_probe(enabled: bool) {
    DIRECT_PSRAM_TX_DMA_PREPARES.store(0, Ordering::Relaxed);
    DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS.store(0, Ordering::Relaxed);
    DIRECT_PSRAM_TX_DMA_LAST_ADDRESS.store(0, Ordering::Relaxed);
    DIRECT_PSRAM_TX_DMA_PROBE.store(enabled, Ordering::Release);
}

#[cfg(feature = "tx-psram-dma-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectPsramTxDmaProbeObservation {
    pub prepares: u32,
    pub first_address: u32,
    pub last_address: u32,
}

#[cfg(feature = "tx-psram-dma-probe")]
pub fn direct_psram_tx_dma_probe_observation() -> DirectPsramTxDmaProbeObservation {
    DirectPsramTxDmaProbeObservation {
        prepares: DIRECT_PSRAM_TX_DMA_PREPARES.load(Ordering::Acquire),
        first_address: DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS.load(Ordering::Acquire),
        last_address: DIRECT_PSRAM_TX_DMA_LAST_ADDRESS.load(Ordering::Acquire),
    }
}

#[cfg(feature = "tx-psram-dma-probe")]
fn prepare_psram_for_wifi_dma_read(storage: &mut [u8]) {
    let address = storage.as_ptr() as usize;
    let end = address
        .checked_add(storage.len())
        .expect("direct Wi-Fi TX DMA probe range must not wrap");
    assert!(
        address >= 0x5000_0000 && end <= 0x5400_0000,
        "direct Wi-Fi TX DMA probe backing must reside in PSRAM"
    );
    // The diagnostic pool isolates every slot on 64-byte cache-line
    // boundaries. The ownership callback writes dirty CPU data back and
    // writes those complete lines back before they become DMA-owned.
    oer_esp32s31_soc_esp_hal::writeback_psram_for_dma_read(storage)
        .expect("direct Wi-Fi TX DMA probe cache writeback must accept its PSRAM slot");
    let address = address as u32;
    let _ = DIRECT_PSRAM_TX_DMA_FIRST_ADDRESS.compare_exchange(
        0,
        address,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    DIRECT_PSRAM_TX_DMA_LAST_ADDRESS.store(address, Ordering::Release);
    DIRECT_PSRAM_TX_DMA_PREPARES.fetch_add(1, Ordering::Relaxed);
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
}

static TX_AMPDU_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
oer_memory::zeroed_static! {
    /// Placement: the linker must retain production A-MPDU descriptors in DMA-visible SRAM.
    static TX_AMPDU_DMA_STORAGE: ZeroedStatic<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
        zeroed in ".dma.bss.open_radio_tx_ampdu_descriptors";
}
static TX_AMPDU_RETENTION: ConstStaticCell<RadioAmpduRetention> =
    ConstStaticCell::new(RetainedAmpduDmaStorage::new());

static TX_AMPDU_STANDBY_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
oer_memory::zeroed_static! {
    /// Placement: the linker must retain standby A-MPDU descriptors in DMA-visible SRAM.
    static TX_AMPDU_STANDBY_DMA_STORAGE: ZeroedStatic<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
        zeroed in ".dma.bss.open_radio_tx_ampdu_standby_descriptors";
}
static TX_AMPDU_STANDBY_RETENTION: ConstStaticCell<RadioAmpduRetention> =
    ConstStaticCell::new(RetainedAmpduDmaStorage::new());

fn initialize_physical_tx() -> PhysicalTxConsumer {
    let network_tx_resources = NETWORK_TX_RESOURCES.take();
    #[cfg(feature = "tx-psram-dma-probe")]
    let tx_pool = if DIRECT_PSRAM_TX_DMA_PROBE.load(Ordering::Acquire) {
        NetworkTxPool::pin_static_with_dma_read_prepare(
            PSRAM_NETWORK_TX_POOL.take(),
            prepare_psram_for_wifi_dma_read,
        )
    } else {
        NetworkTxPool::pin_static(NETWORK_TX_POOL.take())
    };
    #[cfg(not(feature = "tx-psram-dma-probe"))]
    let tx_pool = NetworkTxPool::pin_static(NETWORK_TX_POOL.take());
    network_tx_resources.split(tx_pool)
}

#[cfg(feature = "owned-network")]
pub(crate) fn initialize_network(
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> (WifiDevices, WifiNetworkResources) {
    let station_resources = NETWORK_RESOURCES.take();
    let access_point_resources = ACCESS_POINT_NETWORK_RESOURCES.take();
    let station_packet_allocator = STATION_NETWORK_PACKET_POOL
        .init(PacketPool::new(
            &mut STATION_NETWORK_PACKET_STORAGE.take().0,
        ))
        .allocator();
    let access_point_packet_allocator = ACCESS_POINT_NETWORK_PACKET_POOL
        .init(PacketPool::new(
            &mut ACCESS_POINT_NETWORK_PACKET_STORAGE.take().0,
        ))
        .allocator();
    let station_rx_allocator = STATION_RX_PACKET_POOL
        .init(PacketPool::new(&mut STATION_RX_PACKET_STORAGE.take().0))
        .allocator();
    let access_point_rx_allocator = ACCESS_POINT_RX_PACKET_POOL
        .init(PacketPool::new(
            &mut ACCESS_POINT_RX_PACKET_STORAGE.take().0,
        ))
        .allocator();
    let tx_consumer = initialize_physical_tx();
    let station_interface =
        oer_esp32s31_ieee80211_runtime::datapath::network::STA_NETWORK_INTERFACE_ID;
    let access_point_interface =
        oer_esp32s31_ieee80211_runtime::datapath::network::AP_NETWORK_INTERFACE_ID;
    let (station_device, station_runner) =
        station_resources.split(station_interface, station_address, station_rx_allocator);
    let (access_point_device, access_point_runner) = access_point_resources.split(
        access_point_interface,
        access_point_address,
        access_point_rx_allocator,
    );
    let runner = DualOwnedDatapathNetwork::new(
        station_runner,
        access_point_runner,
        tx_consumer,
        crate::supervisor::rx_zero_copy_origin(),
    );
    let runner = NETWORK_RUNNER.init(runner);
    (
        WifiDevices {
            station: WifiDevice {
                inner: station_device,
                packet_allocator: station_packet_allocator,
            },
            access_point: WifiDevice {
                inner: access_point_device,
                packet_allocator: access_point_packet_allocator,
            },
        },
        WifiNetworkResources::Unstarted { device: (), runner },
    )
}

pub(super) fn initialize_ampdu() -> Result<RadioAmpduStorage, HtAmpduTxError> {
    Ok(AggregateTxResources::pipelined(
        HtAmpduTxResources::pin_static(TX_AMPDU_STORAGE.take(), TX_AMPDU_DMA_STORAGE.take())?,
        TX_AMPDU_RETENTION.take(),
        HtAmpduTxResources::pin_static(
            TX_AMPDU_STANDBY_STORAGE.take(),
            TX_AMPDU_STANDBY_DMA_STORAGE.take(),
        )?,
        TX_AMPDU_STANDBY_RETENTION.take(),
    ))
}
