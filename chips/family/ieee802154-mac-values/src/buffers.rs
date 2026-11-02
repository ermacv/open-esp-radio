//! Frame buffers the MAC DMA reads and writes.
//!
//! The vendor driver keeps its receive ring, enhanced-ACK frame and the
//! caller's transmit frame in ordinary memory whose addresses it publishes
//! to the MAC. Here the engine owns every buffer: the caller's frame is
//! copied in before its address is published, and every CPU access copies
//! a complete image in or out with volatile accesses, so no reference ever
//! aliases memory the DMA may write.

use oer_memory::DmaBytes;

/// Bytes in one `[PHR, PSDU...]` DMA frame image.
pub const FRAME_SIZE: usize = 128;

/// Receive buffers the upper layer may hold (`CONFIG_IEEE802154_RX_BUFFER_SIZE`).
pub const RX_BUFFER_COUNT: usize = 20;

/// One four-byte-aligned DMA frame image.
#[derive(bytemuck::Zeroable)]
#[repr(transparent)]
pub struct DmaFrame(DmaBytes<FRAME_SIZE>);

impl DmaFrame {
    const fn new() -> Self {
        Self(DmaBytes::new())
    }

    /// Copy the complete image out.
    pub fn read(&self) -> [u8; FRAME_SIZE] {
        self.0.read()
    }

    /// Copy `image` in, zero-filling the rest; the engine writes an image
    /// only while the DMA does not own it.
    pub fn write(&mut self, image: &[u8]) {
        self.0.write(image);
    }

    /// Write the bytes of `image` that differ from `original` into the
    /// frame the MAC is transmitting, as the vendor's
    /// `esp_ieee802154_transmit_sfd_done` callback edits the frame in place
    /// at its SFD: the MAC reads the frame's later bytes after its SFD.
    pub fn patch(&mut self, original: &[u8; FRAME_SIZE], image: &[u8; FRAME_SIZE]) {
        self.0.write_changed(original, image);
    }

    /// Clear bit 7 of the PHR byte, as the vendor receive path does.
    pub fn mask_length(&mut self) {
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

    /// The receive ring and its stub, projected without moving storage.
    pub fn rx_frames(&self) -> &[DmaFrame; RX_BUFFER_COUNT + 1] {
        &self.rx
    }

    /// Mutably borrow the receive ring and its stub within this storage owner.
    pub fn rx_frames_mut(&mut self) -> &mut [DmaFrame; RX_BUFFER_COUNT + 1] {
        &mut self.rx
    }

    /// Mutably borrow the transmit frame within this storage owner.
    pub fn tx_frame_mut(&mut self) -> &mut DmaFrame {
        &mut self.tx
    }

    /// Mutably borrow the enhanced ACK within this storage owner.
    pub fn enhanced_ack_frame_mut(&mut self) -> &mut DmaFrame {
        &mut self.enhanced_ack
    }

    /// The transmit frame, projected without moving storage.
    pub fn tx_frame(&self) -> &DmaFrame {
        &self.tx
    }

    /// The enhanced ACK, projected without moving storage.
    pub fn enhanced_ack_frame(&self) -> &DmaFrame {
        &self.enhanced_ack
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    reason = "tests assert frame identity and reject foreign addresses"
)]
mod tests;

/// Addresses supplied once by the HAL that owns the placed buffers.
///
/// The shared engine selects entries by slot; it never derives an address
/// from a pointer or from another address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154DmaAddresses<Address> {
    /// One address for each receive slot, followed by the stub.
    pub rx: [Address; RX_BUFFER_COUNT + 1],
    /// The transmit frame's address.
    pub tx: Address,
    /// The enhanced ACK's address.
    pub enhanced_ack: Address,
}

#[cfg(not(target_arch = "riscv32"))]
impl<Address: Copy + Eq> Ieee802154DmaAddresses<Address> {
    fn frame_index(&self, address: Address) -> Option<usize> {
        self.rx
            .iter()
            .chain([&self.tx, &self.enhanced_ack])
            .position(|candidate| *candidate == address)
    }

    /// Select the frame with this supplied opaque address, refusing foreign addresses.
    pub fn frame_at<'a>(
        &self,
        buffers: &'a Ieee802154EngineBuffers,
        address: Address,
    ) -> Option<&'a DmaFrame> {
        let index = self.frame_index(address)?;
        buffers
            .rx
            .iter()
            .chain([&buffers.tx, &buffers.enhanced_ack])
            .nth(index)
    }

    /// Mutably select a frame without deriving or manufacturing an address.
    pub fn frame_at_mut<'a>(
        &self,
        buffers: &'a mut Ieee802154EngineBuffers,
        address: Address,
    ) -> Option<&'a mut DmaFrame> {
        let index = self.frame_index(address)?;
        buffers
            .rx
            .iter_mut()
            .chain([&mut buffers.tx, &mut buffers.enhanced_ack])
            .nth(index)
    }
}
