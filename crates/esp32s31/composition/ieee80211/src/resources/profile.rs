//! Named static-memory sizing for the ESP32-S31 Wi-Fi composition.
//!
//! These values are integration policy, not hardware capabilities. There is
//! one production profile: diagnostics may observe it, but must never
//! select a different queue, aggregation, or reorder topology. CPU-only state
//! belongs in initialized PSRAM while DMA-visible and latency-critical state
//! remains in SRAM.

use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_dma::tx_storage::TxDmaStorage;

use oer_esp32s31_ieee80211_runtime::{
    datapath::rx::dma::ReceiveDmaStorage, roles::station::StationControlResources,
};

use oer_esp32s31_ieee80211_mac::rx::PUBLIC_HEADER_SIZE;

use oer_ieee80211_mac::{
    beacon::AP_BEACON_CAPACITY, extensions::espressif::esp_now::ESP_NOW_V2_MAX_MPDU_LEN,
    scan::ScanTable,
};

use oer_memory::zeroed::{ZeroedStatic, zeroed};
use static_cell::StaticCell;

/// Physical RX descriptor capacity. The upper zero-copy owner remains capped
/// at 32, leaving 64 descriptors in the radio domain. A delayed masked-IRQ
/// epoch can still complete those descriptors faster than software drains
/// them, so this is not a minimum armed-credit guarantee.
pub const ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT: usize = 96;
pub const ESP32S31_DEFAULT_RX_BUFFER_SIZE: usize = 1_700;
pub const ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_DEFAULT_RX_BUFFER_SIZE + 4;
pub const ESP32S31_DEFAULT_TX_BUFFER_SIZE: usize = 1_700;
pub const ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY: usize = 1_700;
pub const ESP32S31_DEFAULT_SCAN_RECORD_CAPACITY: usize = 32;

pub const ESP32S31_DEFAULT_RX_STAGE_CAPACITY: usize = 1_700;

const ESP_NOW_V2_TX_MINIMUM_CAPACITY: usize = (oer_esp32s31_ieee80211::ordinary_tx::TX_METADATA_SIZE
    + ESP_NOW_V2_MAX_MPDU_LEN
    + oer_esp32s31_ieee80211::ordinary_tx::TX_FCS_SIZE
    + 3)
    & !3;
const _: () = assert!(ESP32S31_DEFAULT_TX_BUFFER_SIZE >= ESP_NOW_V2_TX_MINIMUM_CAPACITY);
// This proves the stock non-CSI public RX prefix. Enabling optional CSI or
// another dynamic descriptor tail remains a separate larger-arena policy.
const _: () =
    assert!(ESP32S31_DEFAULT_RX_BUFFER_SIZE >= PUBLIC_HEADER_SIZE + ESP_NOW_V2_MAX_MPDU_LEN);
const _: () = assert!(ESP32S31_DEFAULT_RX_STAGE_CAPACITY >= ESP_NOW_V2_MAX_MPDU_LEN);
// These SRAM slots store only affine handoff records. An admitted ordinary
// frame retains its original DMA buffer until the final network lease is
// released; payload bytes are not copied into this pool. Capping upper
// ownership at two negotiated BA-16 windows leaves 64 descriptors in the
// radio domain. Accepted-list pressure is measured independently because
// completed descriptors awaiting the next software drain are not armed
// credits. Sustained consumer backpressure must be serviced or dropped
// according to admission policy instead of being hidden by growing this
// latency-critical ownership window.
pub const ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT: usize = 32;
// Keep at least two negotiated HT BlockAck windows worth of descriptor credits
// outside the maximum retained upper-ownership window. A 32-entry agreement
// consumed the entire 32-descriptor frontier and exposed ordinary executor
// latency as RX BUFFER_FULL under sustained HT40 traffic.
pub const ESP32S31_DEFAULT_RX_REORDER_WINDOW: usize = 16;
pub const ESP32S31_DEFAULT_CONTROL_QUEUE_DEPTH: usize = 16;

pub const ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY: usize = 1_600;
// The CPU-only network endpoint queue belongs in PSRAM. Its depth is separate
// from the 32-slot retained-buffer ownership cap: an ordinary zero-copy RX
// token still retains one of those slots until Core 1 consumes it, while copied
// reorder/reassembly frames use independent queue backing.
pub const ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH: usize = 64;
// Shared admission for complete network-owned TX packets, both queued and
// retained by Core0 for scheduling or power-save. Dequeue keeps the credit;
// releasing the software owner returns it. This bound is independent of the
// 67-slot DMA execution pool and does not reserve capacity per AP peer.
// Changing it must never resize the physical SRAM pool.
pub const ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH: usize =
    oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AP_SOFTWARE_TX_CAPACITY;
// One independently polled Xarxa instance owns one general pool and one
// driver-RX pool. The general pool covers the software TX horizon plus
// neighbor/control transients. The RX pool covers the driver queue plus
// protocol/socket retention without coupling those owners to DMA staging.
pub const ESP32S31_DEFAULT_NETWORK_PACKET_POOL_CAPACITY: usize = 160;
pub const ESP32S31_DEFAULT_NETWORK_RX_PACKET_POOL_CAPACITY: usize = 96;
const ESP32S31_PERMANENT_NETWORK_ENDPOINTS: usize = 2;
const ESP32S31_NETWORK_TX_PIPELINE_CREDITS: usize = 1;
// One additional credit per permanent network endpoint is reserved for the
// TX token paired with ingress. Combined STA+AP therefore needs two reserves.
// Application egress needs two complete 32-MPDU aggregate arenas and one
// pipeline credit:
// `Driver::transmit` owns that credit while the network stack formats a frame,
// before the resulting lease becomes visible to the radio consumer. The
// optional staged architecture must prove that peer classification no longer
// consumes physical DMA credits before this pool is grown again.
pub const ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH: usize = 2
    * ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT
    + ESP32S31_PERMANENT_NETWORK_ENDPOINTS
    + ESP32S31_NETWORK_TX_PIPELINE_CREDITS;
pub const ESP32S31_DEFAULT_NETWORK_TX_TRAILER: usize = 12;
pub const ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT: usize = 32;

/// Production marker exposed to board composition and memory reporting.
pub struct DefaultWifiResourceProfile;

impl DefaultWifiResourceProfile {
    pub const RX_DESCRIPTOR_COUNT: usize = ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT;
    pub const RX_BUFFER_SIZE: usize = ESP32S31_DEFAULT_RX_BUFFER_SIZE;
    pub const TX_BUFFER_SIZE: usize = ESP32S31_DEFAULT_TX_BUFFER_SIZE;
    pub const RX_STAGE_SLOT_COUNT: usize = ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT;
    pub const RX_REORDER_WINDOW: usize = ESP32S31_DEFAULT_RX_REORDER_WINDOW;
    pub const NETWORK_RX_QUEUE_DEPTH: usize = ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH;
    pub const NETWORK_OWNER_TX_QUEUE_DEPTH: usize = ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH;
    pub const NETWORK_TX_QUEUE_DEPTH: usize = ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH;
    pub const TX_AMPDU_FRAME_COUNT: usize = ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT;
}

/// Default statically placed RX DMA arena used by a normal station/monitor
/// composition.
pub type DefaultRxDmaStorage = ReceiveDmaStorage<
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT,
    ESP32S31_DEFAULT_RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE,
>;

pub type DefaultScanTable = ScanTable<ESP32S31_DEFAULT_SCAN_RECORD_CAPACITY>;

/// Software scan records, independently placed in ordinary static memory.
///
/// These records are parsed by the CPU; they are never submitted to a DMA
/// master. Keeping their storage separate avoids charging the full scan table
/// against the internal SRAM budget of the radio buffers.
pub struct DefaultScanMemory {
    claimed: AtomicBool,
    table: StaticCell<DefaultScanTable>,
}

impl DefaultScanMemory {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            table: StaticCell::new(),
        }
    }
}

impl Default for DefaultScanMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Statically placed DMA buffers and small role-control resources.
///
/// A board places this owner in DMA-visible SRAM and the separate
/// [`DefaultScanMemory`] in ordinary memory. [`Self::claim`] reserves
/// both owners before exposing any field, so a conflicting claim cannot
/// partially consume either arena.
///
/// A board places it in a region the boot zeroes, so it is `bytemuck::Zeroable`:
/// the DMA arenas and frame buffers start as their zero state, and the
/// role-control resources, whose signals have no zero representation, are
/// written when [`Self::claim`] takes them.
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct DefaultWifiMemory<M: RawMutex> {
    claimed: AtomicBool,
    rx_dma: ZeroedStatic<DefaultRxDmaStorage>,
    tx_dma: ZeroedStatic<TxDmaStorage<ESP32S31_DEFAULT_TX_BUFFER_SIZE>>,
    ap_beacon: ZeroedStatic<[u8; AP_BEACON_CAPACITY]>,
    scan_frame: ZeroedStatic<[u8; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY]>,
    station_control: ZeroedStatic<MaybeUninit<StationControlResources<M>>>,
}

impl<M: RawMutex> DefaultWifiMemory<M> {
    /// The unclaimed memory: all zero bytes.
    pub const fn new() -> Self {
        zeroed()
    }

    /// Acquire the complete initial station owner graph exactly once.
    ///
    /// If either arena is already claimed, no cell is taken. A failed scan
    /// reservation releases the radio reservation for a later valid pair.
    #[allow(
        large_assignments,
        reason = "initializes the existing scan table in static storage; the final-image stack audit bounds actual compiler temporaries"
    )]
    pub fn claim(
        &'static self,
        scan: &'static DefaultScanMemory,
    ) -> Result<DefaultWifiMemoryLease<M>, DefaultWifiMemoryError> {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DefaultWifiMemoryError::InUse);
        }
        if scan
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.claimed.store(false, Ordering::Release);
            return Err(DefaultWifiMemoryError::InUse);
        }
        // Both reservations are now permanent. Private cells can only be
        // taken here, and no failed reservation above has consumed a cell.
        Ok(DefaultWifiMemoryLease {
            rx_dma: self.rx_dma.take(),
            tx_dma: self.tx_dma.take(),
            scan_table: scan.table.init_with(ScanTable::new),
            ap_beacon: self.ap_beacon.take(),
            scan_frame: self.scan_frame.take(),
            station_control: self.station_control.init(StationControlResources::new()),
        })
    }
}

impl<M: RawMutex> Default for DefaultWifiMemory<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact owners acquired from one default station arena.
pub struct DefaultWifiMemoryLease<M: RawMutex + 'static> {
    pub rx_dma: &'static mut DefaultRxDmaStorage,
    pub tx_dma: &'static mut TxDmaStorage<ESP32S31_DEFAULT_TX_BUFFER_SIZE>,
    pub scan_table: &'static mut DefaultScanTable,
    pub ap_beacon: &'static mut [u8; AP_BEACON_CAPACITY],
    pub scan_frame: &'static mut [u8; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY],
    pub station_control: &'static StationControlResources<M>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefaultWifiMemoryError {
    InUse,
}

const _: () =
    assert!(ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY <= ESP32S31_DEFAULT_RX_STAGE_CAPACITY);
const _: () = assert!(ESP32S31_DEFAULT_RX_REORDER_WINDOW <= ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT);
const _: () =
    assert!(ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT >= 2 * ESP32S31_DEFAULT_RX_REORDER_WINDOW);
const _: () =
    assert!(ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT < ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH);

#[cfg(test)]
mod tests;
