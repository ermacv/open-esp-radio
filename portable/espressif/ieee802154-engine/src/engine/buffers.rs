//! Frame buffers the MAC DMA reads and writes.
//!
//! The vendor driver keeps its receive ring, enhanced-ACK frame and the
//! caller's transmit frame in ordinary memory whose addresses it publishes
//! to the MAC. Here the engine owns every buffer: the caller's frame is
//! copied in before its address is published, and every CPU access copies
//! a complete image in or out with volatile accesses, so no reference ever
//! aliases memory the DMA may write.

use oer_base_memory::DmaBytes;

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
#[derive(bytemuck::Zeroable)]
#[repr(transparent)]
pub(crate) struct DmaFrame(DmaBytes<FRAME_SIZE>);

impl DmaFrame {
    const fn new() -> Self {
        Self(DmaBytes::new())
    }

    /// The address published to the MAC DMA.
    pub(crate) fn address(&self) -> u32 {
        self.0.address() as u32
    }

    /// Copy the complete image out.
    pub(crate) fn read(&self) -> [u8; FRAME_SIZE] {
        self.0.read()
    }

    /// Copy `image` in, zero-filling the rest; the engine writes an image
    /// only while the DMA does not own it.
    pub(crate) fn write(&mut self, image: &[u8]) {
        self.0.write(image);
    }

    /// Write the bytes of `image` that differ from `original` into the
    /// frame the MAC is transmitting, as the vendor's
    /// `esp_ieee802154_transmit_sfd_done` callback edits the frame in place
    /// at its SFD: the MAC reads the frame's later bytes after its SFD.
    pub(crate) fn patch(&mut self, original: &[u8; FRAME_SIZE], image: &[u8; FRAME_SIZE]) {
        self.0.write_changed(original, image);
    }

    /// Clear bit 7 of the PHR byte, as the vendor receive path does.
    pub(crate) fn mask_length(&mut self) {
        self.0.update_byte(0, |phr| phr & 0x7f);
    }
}

/// Storage for one engine: the receive ring, its stub buffer, the transmit
/// frame and the enhanced-ACK frame.
///
/// The engine borrows it exclusively for its whole life, so the published
/// addresses stay valid and no second engine can publish them.
/// Its zero bytes are the zeroed buffers `new` builds (`bytemuck::Zeroable`).
#[derive(bytemuck::Zeroable)]
pub struct Ieee802154EngineBuffers {
    pub(crate) rx: [DmaFrame; RX_BUFFER_COUNT + 1],
    pub(crate) tx: DmaFrame,
    pub(crate) enhanced_ack: DmaFrame,
}

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

    /// The buffer whose published address is `address`, to write.
    #[cfg(not(target_arch = "riscv32"))]
    pub(crate) fn frame_at_mut(&mut self, address: u32) -> Option<&mut DmaFrame> {
        self.rx
            .iter_mut()
            .chain([&mut self.tx, &mut self.enhanced_ack])
            .find(|frame| frame.address() == address)
    }
}

#[cfg(test)]
mod tests;
