//! Frame buffers the MAC DMA reads and writes.
//!
//! The vendor driver keeps its receive ring, enhanced-ACK frame and the
//! caller's transmit frame in ordinary memory whose addresses it publishes
//! to the MAC. Here the engine owns every buffer: the caller's frame is
//! copied in before its address is published, and every CPU access copies
//! a complete image in or out with volatile accesses, so no reference ever
//! aliases memory the DMA may write.

use core::cell::UnsafeCell;

/// Bytes in one `[PHR, PSDU...]` DMA frame image.
pub const FRAME_SIZE: usize = 128;

/// Receive buffers the upper layer may hold (`CONFIG_IEEE802154_RX_BUFFER_SIZE`).
pub const RX_BUFFER_COUNT: usize = 20;

/// Whether the `len` bytes at `start` lie in `window`, the memory a chip's
/// MAC DMA reaches.
const fn dma_window_contains(window: &core::ops::Range<usize>, start: usize, len: usize) -> bool {
    match start.checked_add(len) {
        Some(end) => start >= window.start && end <= window.end,
        None => false,
    }
}

/// One four-byte-aligned DMA frame image.
#[repr(C, align(4))]
pub(crate) struct DmaFrame(UnsafeCell<[u8; FRAME_SIZE]>);

#[allow(unsafe_code, reason = "a frame's zero bytes are the empty frame")]
// SAFETY: a cell of bytes; zero is the frame `new` builds. Written by hand,
// not derived: this crate is in the source-only PHY build, which runs no
// proc macro.
unsafe impl bytemuck::Zeroable for DmaFrame {}

impl DmaFrame {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; FRAME_SIZE]))
    }

    /// The address published to the MAC DMA.
    pub(crate) fn address(&self) -> u32 {
        self.0.get() as usize as u32
    }

    /// Copy the complete image out.
    #[allow(
        unsafe_code,
        reason = "a volatile whole-image read never forms a reference to DMA-owned memory"
    )]
    pub(crate) fn read(&self) -> [u8; FRAME_SIZE] {
        // SAFETY: the pointer comes from this live `UnsafeCell` and is
        // aligned; the volatile read tolerates a concurrent DMA writer.
        unsafe { self.0.get().read_volatile() }
    }

    /// Copy `image` in, zero-filling the rest.
    #[allow(
        unsafe_code,
        reason = "a volatile whole-image write never forms a reference to DMA-owned memory"
    )]
    pub(crate) fn write(&self, image: &[u8]) {
        let mut frame = [0; FRAME_SIZE];
        let length = image.len().min(FRAME_SIZE);
        frame[..length].copy_from_slice(&image[..length]);
        // SAFETY: as for `read`; the engine writes an image only while the
        // DMA does not own it.
        unsafe { self.0.get().write_volatile(frame) }
    }

    /// Write the bytes of `image` that differ from `original` into the
    /// frame the MAC is transmitting, one volatile byte each, as the
    /// vendor's `esp_ieee802154_transmit_sfd_done` callback edits the frame
    /// in place at its SFD.
    #[allow(
        unsafe_code,
        reason = "volatile byte writes into a live DMA frame never form a reference to it"
    )]
    pub(crate) fn patch(&self, original: &[u8; FRAME_SIZE], image: &[u8; FRAME_SIZE]) {
        let base = self.0.get().cast::<u8>();
        for (index, (&old, &new)) in original.iter().zip(image).enumerate() {
            if old != new {
                // SAFETY: `index` is below `FRAME_SIZE`, so the byte lies
                // inside this live, aligned `UnsafeCell`; the MAC reads the
                // frame's later bytes after its SFD, as in the vendor driver.
                unsafe { base.add(index).write_volatile(new) }
            }
        }
    }

    /// Clear bit 7 of the PHR byte, as the vendor receive path does.
    #[allow(
        unsafe_code,
        reason = "one volatile byte access to the PHR of a completed frame"
    )]
    pub(crate) fn mask_length(&self) {
        let phr = self.0.get().cast::<u8>();
        // SAFETY: byte zero lies inside this live, aligned `UnsafeCell`.
        unsafe { phr.write_volatile(phr.read_volatile() & 0x7f) }
    }
}

/// Storage for one engine: the receive ring, its stub buffer, the transmit
/// frame and the enhanced-ACK frame.
///
/// The engine borrows it exclusively for its whole life, so the published
/// addresses stay valid and no second engine can publish them.
/// Its zero bytes are the zeroed buffers `new` builds (`bytemuck::Zeroable`).
pub struct Ieee802154EngineBuffers {
    pub(crate) rx: [DmaFrame; RX_BUFFER_COUNT + 1],
    pub(crate) tx: DmaFrame,
    pub(crate) enhanced_ack: DmaFrame,
}

#[allow(unsafe_code, reason = "zeroed frames are the zeroed buffers")]
// SAFETY: every field is a `DmaFrame` array or frame, zero-valid above.
unsafe impl bytemuck::Zeroable for Ieee802154EngineBuffers {}

impl Default for Ieee802154EngineBuffers {
    fn default() -> Self {
        Self::new()
    }
}

impl Ieee802154EngineBuffers {
    /// Zeroed buffers, suitable for a `static`.
    pub const fn new() -> Self {
        Self {
            rx: [const { DmaFrame::new() }; RX_BUFFER_COUNT + 1],
            tx: DmaFrame::new(),
            enhanced_ack: DmaFrame::new(),
        }
    }

    /// Whether every frame lies in `window`, the memory the chip's MAC DMA
    /// reaches. Each chip's HAL names its window.
    pub fn is_dma_visible(&self, window: &core::ops::Range<usize>) -> bool {
        dma_window_contains(
            window,
            core::ptr::from_ref(self) as usize,
            core::mem::size_of::<Self>(),
        )
    }

    /// The buffer whose published address is `address`.
    #[cfg(not(target_arch = "riscv32"))]
    pub(crate) fn frame_at(&self, address: u32) -> Option<&DmaFrame> {
        self.rx
            .iter()
            .chain([&self.tx, &self.enhanced_ack])
            .find(|frame| frame.address() == address)
    }
}

#[allow(
    unsafe_code,
    reason = "every buffer access goes through the exclusively borrowed engine"
)]
// SAFETY: the buffers are reachable only through the engine's exclusive
// borrow, and every access is a whole-image volatile copy.
unsafe impl Sync for Ieee802154EngineBuffers {}

#[cfg(test)]
mod tests;
