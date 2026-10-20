//! Production RX-ring recycle compared with `wDev_AppendRxBlocks`.
//!
//! One probe entry prepares and starts the production ring over this image's
//! placed DMA storage, completes a prefix of it as the DMA walker would, then
//! returns that completed unit to the walker through the production recycle
//! and the reload doorbell with its completion. Only the recycle touches
//! the MAC, through the HAL's RX DMA engine: preparation and start run
//! against the HAL's walker model, which stands for a walker the vendor's
//! own initialization already started. The layout entry reports the
//! storage's addresses, so the vendor side receives the same descriptors at
//! the same addresses.

use oer_esp32s31_hal::ieee80211::dma::{
    descriptor::rx_completed_word,
    rx_dma::model::RxWalkerModel,
    rx_storage::{PlacedRxDma, RxDmaStorage},
};
use oer_esp32s31_hal::settle::{Settle, SettleLimit};
use oer_time::{Clock, Duration, Instant};

/// Descriptors of the compared ring.
const RX_APPEND_COUNT: usize = 4;
/// Capacity of each ring buffer, and its storage with the trailing guard.
const RX_APPEND_CAPACITY: usize = 64;
const RX_APPEND_STORAGE: usize = RX_APPEND_CAPACITY + core::mem::size_of::<u32>();
/// Words of the layout entry's output before the buffer addresses.
const LAYOUT_HEADER_WORDS: usize = 3;

/// Failure codes of the recycle entry: preparation, start, the completed unit
/// and the recycle with its reload.
const PREPARE_FAILED: u32 = 1;
const START_FAILED: u32 = 2;
const NO_UNIT: u32 = 3;
const UNIT_FAILED: u32 = 4;
const RECYCLE_FAILED: u32 = 5;
const NOT_APPENDED: u32 = 6;
const RELOAD_FAILED: u32 = 7;
const INVALID_UNIT: u32 = 8;
/// A completed unit covers the ring's accepted tail and cannot be appended
/// behind itself. Keep Corrupt distinct from other recycle failures.
const CORRUPT_UNIT: u32 = 9;

/// The reload settle's clock: this image has no timer, so each read is one
/// microsecond later, which bounds the settle at its limit's observations.
struct OneMicrosecondPerRead(core::cell::Cell<u64>);

impl Clock for OneMicrosecondPerRead {
    fn now(&self) -> Instant {
        let now = self.0.get();
        self.0.set(now.saturating_add(1));
        Instant::from_micros(now)
    }
}

oer_esp32s31_hal::dma_static! {
    static STORAGE: RxDmaStorage<RX_APPEND_COUNT, RX_APPEND_CAPACITY, RX_APPEND_STORAGE> =
        zeroed in ".dma.bss.rx_append";
}

type Placed = PlacedRxDma<RX_APPEND_COUNT, RX_APPEND_CAPACITY, RX_APPEND_STORAGE>;

/// The storage, placed on first use; later entries of the session see the
/// same placement.
fn placed() -> Option<&'static mut Option<Placed>> {
    static mut PLACED: Option<Placed> = None;
    // SAFETY: Blobray executes this probe image on one thread and invokes
    // its exported entries serially, so no two references are alive.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(PLACED) };
    if slot.is_none() {
        *slot = RxDmaStorage::place(STORAGE.take()?).ok();
    }
    Some(slot)
}

oer_probe_macros::probe! {
    /// The ring's descriptor count, buffer capacity and descriptor base,
    /// then each buffer address, written to `output`.
    ///
    /// # Safety
    /// `output` must point to `LAYOUT_HEADER_WORDS + RX_APPEND_COUNT`
    /// writable words.
    pub unsafe fn open_libpp_rx_append_trace_layout(output: *mut u32) -> u32 {
        let Some(placed) = placed().and_then(|slot| slot.as_ref()) else {
            return PREPARE_FAILED;
        };
        // SAFETY: the caller provides the output words.
        unsafe {
            output.write(RX_APPEND_COUNT as u32);
            output.add(1).write(RX_APPEND_CAPACITY as u32);
            output.add(2).write(placed.descriptor_base());
            for (index, address) in placed.storage().buffer_addresses().iter().enumerate() {
                output.add(LAYOUT_HEADER_WORDS + index).write(*address);
            }
        }
        0
    }
}

oer_probe_macros::probe! {
    /// Complete the ring's first `descriptors` as one received unit of
    /// `length` bytes in its last descriptor, and the `later` descriptors
    /// after it as single-descriptor units, then return the first unit to
    /// the walker through the production recycle and its reload: zero, or
    /// the step that failed.
    pub fn open_libpp_rx_append_trace_recycle(descriptors: u32, later: u32, length: u32) -> u32 {
        let count = descriptors as usize;
        let completed = count + later as usize;
        if count == 0 || completed > RX_APPEND_COUNT || length as usize > RX_APPEND_CAPACITY {
            return INVALID_UNIT;
        }
        let Some(placed) = placed().and_then(Option::take) else {
            return PREPARE_FAILED;
        };
        let (storage, base) = (placed.storage(), placed.descriptor_base());
        let mut walker = RxWalkerModel::new();
        let Ok(stopped) = placed.prepare_ring(&mut walker) else {
            return PREPARE_FAILED;
        };
        let Ok(mut ring) = stopped.try_start(&mut walker) else {
            return START_FAILED;
        };
        // The walker fills the units: every buffer's leading guard is
        // overwritten by frame bytes, and each unit's last descriptor is
        // marked done with its length.
        for (index, &address) in storage.buffer_addresses().iter().enumerate().take(completed) {
            // SAFETY: the walker owns every buffer of the started ring; this
            // probe acts as that walker on the storage it reported.
            unsafe { (address as *mut u32).write_volatile(length) };
            let descriptor = &storage.descriptors()[index];
            let Some(word0) = rx_completed_word(descriptor.word0(), length, index + 1 >= count)
            else {
                return INVALID_UNIT;
            };
            descriptor.write_word0(word0);
        }
        // LAST rests on the unit's last descriptor, NEXT after it.
        walker.set_cursor(
            base,
            count - 1,
            (count < RX_APPEND_COUNT).then_some(count),
        );
        let Ok(cursor) = ring.freeze_cursor(&mut walker) else {
            return UNIT_FAILED;
        };
        let mut owner = oer_esp32s31_hal::validation::runtime_owner();
        let engine = owner.rx_dma();
        let recycled = match storage.take_completed_unit(&mut ring, cursor) {
            Ok(Some(unit)) => match unit.recycle(engine) {
                Ok(Some(_append)) => 0,
                Ok(None) => NOT_APPENDED,
                Err(oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingError::Corrupt) => CORRUPT_UNIT,
                Err(_) => RECYCLE_FAILED,
            },
            Ok(None) => NO_UNIT,
            Err(_) => UNIT_FAILED,
        };
        let clock = OneMicrosecondPerRead(core::cell::Cell::new(0));
        let Some(settle_limit) = SettleLimit::new(Duration::from_micros(20)) else {
            return RELOAD_FAILED;
        };
        let result = if recycled != 0 {
            recycled
        } else if ring
            .complete_pending_reload(engine, Settle::new(settle_limit, &clock))
            .is_ok()
        {
            0
        } else {
            RELOAD_FAILED
        };
        // The comparison ends with the walker live: stopping it is not part
        // of the compared transaction.
        core::mem::forget(ring);
        result
    }
}
