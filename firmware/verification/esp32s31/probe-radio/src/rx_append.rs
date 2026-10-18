//! Production RX-ring recycle compared with `wDev_AppendRxBlocks`.
//!
//! One probe entry prepares and starts the production ring over this image's
//! DMA arena, completes a prefix of it as the DMA walker would, then returns
//! that completed unit to the walker through the production recycle and the
//! reload doorbell with its base repair. Only the recycle touches the MAC: the
//! preparation and start run against [`StartedWalker`], a fixture that stands
//! for a walker the vendor's own initialization already started. The layout
//! entry reports the arena's addresses, so the vendor side receives the same
//! descriptors at the same addresses.

use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_dma::{
    descriptor::{BIT_30, DESCRIPTOR_BYTES, Descriptor, LENGTH_MASK, LENGTH_SHIFT},
    rx_dma::{
        RxDma, RxDmaBinding, RxDmaCursorObservation, RxDmaNextDescriptor, RxDmaReloadSettled,
        RxDmaWalkerEnabled, RxDmaWalkerStopped,
    },
    rx_ring::RxDmaBufferAddresses,
    rx_storage::RxDmaStorage,
};

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

struct ProbeCell<T>(core::cell::UnsafeCell<T>);

// SAFETY: Blobray executes this probe image on one thread and invokes its
// exported entries serially.
unsafe impl<T> Sync for ProbeCell<T> {}

oer_memory::zeroed_static! {
    static STORAGE: RxDmaStorage<RX_APPEND_COUNT, RX_APPEND_CAPACITY, RX_APPEND_STORAGE> =
        zeroed in ".dma.bss.rx_append";
}
static ADDRESSES: ProbeCell<RxDmaBufferAddresses<RX_APPEND_COUNT>> =
    ProbeCell(core::cell::UnsafeCell::new([0; RX_APPEND_COUNT]));

/// The walker the vendor initialization already started: every enable,
/// stop and reload request is accepted without a register access, and its
/// cursor rests at the ring's end.
struct StartedWalker;

impl RxDma for StartedWalker {
    fn last_descriptor_low(&mut self) -> u32 {
        0
    }

    fn next_descriptor_low(&mut self) -> u32 {
        0
    }

    fn next_descriptor(&mut self) -> RxDmaNextDescriptor {
        RxDmaNextDescriptor::validation(0, false)
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        observed(RxDmaCursorObservation::validation(0, 0))
    }

    fn walker_enabled(&mut self) -> bool {
        false
    }

    fn reload_pending(&mut self) -> bool {
        false
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        Some(settled(RxDmaReloadSettled::validation()))
    }

    fn configure_descriptor_window(&mut self, _binding: &RxDmaBinding<'_>) {}

    fn write_descriptor_base(&mut self, _binding: &RxDmaBinding<'_>, _address: u32) {}

    fn publish_walker_enable(&mut self, _binding: &RxDmaBinding<'_>) {}

    fn request_reload(&mut self, _binding: &RxDmaBinding<'_>) {}

    fn try_with_walker_enabled<R>(
        &mut self,
        _binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        Some(enabled(RxDmaWalkerEnabled::validation()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        Some(stopped(RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {}
}

/// The arena's descriptor base; its buffer addresses in `ADDRESSES`.
fn layout() -> Option<u32> {
    // SAFETY: the single-threaded image binds the layout before any ring
    // exists, and no other reference to the address table is live.
    STORAGE.dma_layout(unsafe { &mut *ADDRESSES.0.get() }).ok()
}

oer_probe_macros::probe! {
    /// The ring's descriptor count, buffer capacity and descriptor base,
    /// then each buffer address, written to `output`.
    ///
    /// # Safety
    /// `output` must point to `LAYOUT_HEADER_WORDS + RX_APPEND_COUNT`
    /// writable words.
    pub unsafe fn open_libpp_rx_append_trace_layout(output: *mut u32) -> u32 {
        let Some(base) = layout() else {
            return PREPARE_FAILED;
        };
        // SAFETY: the address table is only read here, and the caller
        // provides the output words.
        unsafe {
            output.write(RX_APPEND_COUNT as u32);
            output.add(1).write(RX_APPEND_CAPACITY as u32);
            output.add(2).write(base);
            for (index, address) in (*ADDRESSES.0.get()).iter().enumerate() {
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
        let Some(base) = layout() else {
            return PREPARE_FAILED;
        };
        // SAFETY: the table was bound above and is not mutated again.
        let addresses: &'static RxDmaBufferAddresses<RX_APPEND_COUNT> =
            unsafe { &*ADDRESSES.0.get() };
        let Ok(stopped) = STORAGE.prepare_ring(&mut StartedWalker, base, addresses) else {
            return PREPARE_FAILED;
        };
        let Ok(mut ring) = stopped.try_start(&mut StartedWalker) else {
            return START_FAILED;
        };
        // The walker fills the units: every buffer's leading guard is
        // overwritten by frame bytes, and each unit's last descriptor is
        // marked done with its length.
        for (index, &address) in addresses.iter().enumerate().take(completed) {
            let buffer = address as *mut u32;
            // SAFETY: the walker owns every descriptor of the started ring;
            // this fixture acts as that walker on the arena it reported.
            unsafe { buffer.write_volatile(length) };
            if index + 1 >= count {
                let done = (base + index as u32 * DESCRIPTOR_BYTES) as *const Descriptor;
                // SAFETY: as above, the descriptor lies in the reported arena.
                let done = unsafe { &*done };
                done.write_word0(
                    (done.word0() & !LENGTH_MASK) | (length << LENGTH_SHIFT) | BIT_30,
                );
            }
        }
        let mut owner = RadioRuntimeOwner::claim_for_validation();
        let recycled = match STORAGE.take_completed_unit(&mut ring, count) {
            Ok(Some(unit)) => match unit.recycle(&mut owner) {
                Ok(Some(_append)) => 0,
                Ok(None) => NOT_APPENDED,
                Err(_) => RECYCLE_FAILED,
            },
            Ok(None) => NO_UNIT,
            Err(_) => UNIT_FAILED,
        };
        let result = if recycled != 0 {
            recycled
        } else if ring.complete_pending_reload(&mut owner).is_ok() {
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
