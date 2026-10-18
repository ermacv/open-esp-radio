//! ESP32-S31 Wi-Fi DMA descriptors: the generated RX and TX layouts
//! (`registers/esp32s31/model/layouts/`) and the words this crate publishes
//! in them.

#![forbid(unsafe_code)]

use crate::layouts::{
    DmaRef, DmaWindowAddress, WifiRxDmaDescriptor, WifiRxDmaDescriptorBuffer,
    WifiRxDmaDescriptorControl, WifiRxDmaDescriptorNext, WifiTxDmaDescriptor,
    WifiTxDmaDescriptorBuffer, WifiTxDmaDescriptorControl, WifiTxDmaDescriptorNext,
};

/// The bytes of one descriptor, RX or TX: the layouts share their geometry.
pub const DESCRIPTOR_BYTES: u32 = WifiRxDmaDescriptor::SIZE as u32;

/// The largest MPDU length a descriptor's 14-bit `LENGTH` field (bits 27:14
/// of the control word) encodes; the vendor encodes no longer unit.
pub const MAX_ENCODED_MPDU_LEN: usize = 0x3fff;

const _: () = {
    assert!(WifiTxDmaDescriptor::SIZE == WifiRxDmaDescriptor::SIZE);
    assert!(WifiTxDmaDescriptor::ALIGN == WifiRxDmaDescriptor::ALIGN);
};

/// The exact three-word RX descriptor the Wi-Fi MAC's RX walker follows.
///
/// This is deliberately not `esp_hal::dma::DmaDescriptor`: that type is
/// padded to 16 bytes on ESP32-S31 for AXI-GDMA, while Wi-Fi walks 12-byte
/// nodes. Its words are the generated layout's volatile cells, which can
/// change outside Rust through DMA without leaking raw pointers into the MAC
/// backend. Its zero bytes are the empty, software-owned descriptor `new`
/// builds.
#[repr(transparent)]
#[derive(bytemuck::Zeroable)]
pub struct RxDescriptor(WifiRxDmaDescriptor);

const _: () = {
    assert!(core::mem::size_of::<RxDescriptor>() == DESCRIPTOR_BYTES as usize);
    assert!(core::mem::align_of::<RxDescriptor>() == WifiRxDmaDescriptor::ALIGN);
};

impl RxDescriptor {
    pub const fn new() -> Self {
        Self(WifiRxDmaDescriptor::zeroed())
    }

    /// The control word (word 0).
    #[inline]
    pub fn word0(&self) -> WifiRxDmaDescriptorControl {
        self.0.control().read()
    }

    #[inline]
    pub fn buffer_address(&self) -> u32 {
        self.0.buffer().read().address()
    }

    #[inline]
    pub fn next_address(&self) -> u32 {
        self.0.next().read().address()
    }

    /// Writes address/link first and publishes the ownership word last.
    #[inline]
    pub(crate) fn publish_owned(
        &self,
        word0: WifiRxDmaDescriptorControl,
        buffer: DmaWindowAddress,
        next: Option<DmaRef<WifiRxDmaDescriptor>>,
    ) {
        self.0
            .buffer()
            .write(WifiRxDmaDescriptorBuffer::ZERO.with_address(buffer));
        self.publish_next(next);
        self.0.control().write(word0);
    }

    /// Fault model: memory corruption or a foreign write overwrites all three
    /// descriptor words. It bypasses none of the product's checks.
    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn inject_corrupt_words(
        &self,
        word0: WifiRxDmaDescriptorControl,
        buffer_address: u32,
        next_address: u32,
    ) {
        self.0
            .buffer()
            .write(WifiRxDmaDescriptorBuffer::from_bits(buffer_address));
        self.0
            .next()
            .write(WifiRxDmaDescriptorNext::from_bits(next_address));
        self.0.control().write(word0);
    }

    /// Model one hardware ownership-word update.
    ///
    /// Production target code cannot call this safe method: live descriptor
    /// mutation belongs exclusively to the ring owner and the DMA actor. It
    /// exists only for native models and the explicit raw-DMA validation
    /// feature used by compiled vendor probes.
    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn write_word0(&self, word0: WifiRxDmaDescriptorControl) {
        self.0.control().write(word0);
    }

    /// Links a prepared chain after the current hardware-visible tail.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: the complete ROM body at
    /// `0x2f838a7e` stores the new head through the old tail's `next` word
    /// before calling `rom[]::hal_mac_rx_set_dscr_reload`; the production RX-ring
    /// owner retains the same ownership order.
    ///
    /// This crate-private edge is called only by the live RX-ring owner after
    /// it has validated the accepted tail and prepared an unreachable append
    /// chain. Publication is completed by the MAC append-reload doorbell.
    #[inline]
    pub(crate) fn publish_next(&self, next: Option<DmaRef<WifiRxDmaDescriptor>>) {
        self.0
            .next()
            .write(WifiRxDmaDescriptorNext::ZERO.with_address(next));
    }

    /// Rewrites only the ownership word, keeping the buffer and the link.
    ///
    /// The live RX-ring owner uses it to rearm the reserved descriptor after
    /// a frame landed in it; that descriptor's link is the walker's and is
    /// changed separately, only after the walker left it.
    #[inline]
    pub(crate) fn publish_word0(&self, word0: WifiRxDmaDescriptorControl) {
        self.0.control().write(word0);
    }
}

impl Default for RxDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

/// The exact three-word TX descriptor the Wi-Fi MAC's TX engine follows; the
/// RX descriptor's geometry with the TX control bits.
#[repr(transparent)]
#[derive(bytemuck::Zeroable)]
pub struct TxDescriptor(WifiTxDmaDescriptor);

const _: () = {
    assert!(core::mem::size_of::<TxDescriptor>() == DESCRIPTOR_BYTES as usize);
    assert!(core::mem::align_of::<TxDescriptor>() == WifiTxDmaDescriptor::ALIGN);
};

impl TxDescriptor {
    pub const fn new() -> Self {
        Self(WifiTxDmaDescriptor::zeroed())
    }

    /// The control word (word 0).
    #[inline]
    pub fn word0(&self) -> WifiTxDmaDescriptorControl {
        self.0.control().read()
    }

    #[inline]
    pub fn buffer_address(&self) -> u32 {
        self.0.buffer().read().address()
    }

    #[inline]
    pub fn next_address(&self) -> u32 {
        self.0.next().read().address()
    }

    /// Writes address/link first and publishes the ownership word last.
    #[inline]
    pub(crate) fn publish_owned(
        &self,
        word0: WifiTxDmaDescriptorControl,
        buffer: DmaWindowAddress,
        next: Option<DmaRef<WifiTxDmaDescriptor>>,
    ) {
        self.0
            .buffer()
            .write(WifiTxDmaDescriptorBuffer::ZERO.with_address(buffer));
        self.0
            .next()
            .write(WifiTxDmaDescriptorNext::ZERO.with_address(next));
        self.0.control().write(word0);
    }

    /// Returns all three words to zero: no buffer, no link, not owned.
    #[inline]
    pub(crate) fn clear(&self) {
        self.0.buffer().write(WifiTxDmaDescriptorBuffer::ZERO);
        self.0.next().write(WifiTxDmaDescriptorNext::ZERO);
        self.0.control().write(WifiTxDmaDescriptorControl::ZERO);
    }
}

impl Default for TxDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

/// A capacity both size fields hold, or `None`.
const fn capacity_field(capacity: u32) -> Option<u16> {
    if capacity == 0 || capacity > u16::MAX as u32 {
        None
    } else {
        Some(capacity as u16)
    }
}

/// A completed RX word as it stays while its buffer is detached: the same
/// image with `OWNER` clear, so a walker that reached it would stop on it
/// instead of writing the leased buffer (SOURCE\[HIL_W1_OWNER_BIT_2026_10_18]).
/// `RX_DONE`, the length and the size stay.
#[inline]
pub const fn rx_detached_word(word0: WifiRxDmaDescriptorControl) -> WifiRxDmaDescriptorControl {
    word0.with_owner(false)
}

/// Fresh/recycled RX word: `OWNER` set, `RX_DONE` and `BIT_29_OPAQUE`
/// clear, length equals capacity.
///
/// S31 keeps `OWNER` set when it completes RX and sets `RX_DONE` as the
/// completion marker (SOURCE\[HIL_W1_OWNER_BIT_2026_10_18]). Software
/// ownership must therefore be tested with `rx_done`, not by waiting for
/// the owner bit to clear.
pub const fn rx_armed_word(capacity: u32) -> Option<WifiRxDmaDescriptorControl> {
    let Some(capacity) = capacity_field(capacity) else {
        return None;
    };
    match WifiRxDmaDescriptorControl::ZERO.with_size(capacity) {
        Some(word0) => match word0.with_length(capacity) {
            Some(word0) => Some(word0.with_owner(true)),
            None => None,
        },
        None => None,
    }
}

/// Rearms a hardware-completed or detached RX word while preserving unrelated
/// bits: `OWNER` is set again, `RX_DONE` and `BIT_29_OPAQUE` cleared, the
/// length set back to the capacity.
pub const fn rx_rearm_word(
    word0: WifiRxDmaDescriptorControl,
) -> Option<WifiRxDmaDescriptorControl> {
    let capacity = word0.size();
    if capacity == 0 {
        return None;
    }
    match word0.with_length(capacity) {
        Some(word0) => Some(
            word0
                .with_bit_29_opaque(false)
                .with_rx_done(false)
                .with_owner(true),
        ),
        None => None,
    }
}

/// The word the hardware leaves on a descriptor it filled with `length`
/// bytes: the armed `word0` with that length, `OWNER` still set and
/// `RX_DONE` on the unit's terminal descriptor
/// (SOURCE\[HIL_W1_OWNER_BIT_2026_10_18]). A model of the hardware, for
/// native models and tests.
#[cfg(any(test, feature = "test-support"))]
pub const fn rx_completed_word(
    word0: WifiRxDmaDescriptorControl,
    length: u32,
    terminal: bool,
) -> Option<WifiRxDmaDescriptorControl> {
    if length > u16::MAX as u32 {
        return None;
    }
    match word0.with_length(length as u16) {
        Some(word0) => Some(word0.with_rx_done(terminal).with_owner(true)),
        None => None,
    }
}

/// The word the hardware leaves on an armed descriptor of `capacity` bytes
/// it filled with `length` bytes, terminal or not. A model of the hardware,
/// for native models and tests.
#[cfg(any(test, feature = "test-support"))]
pub const fn rx_landed_word(
    capacity: u32,
    length: u32,
    terminal: bool,
) -> WifiRxDmaDescriptorControl {
    match rx_armed_word(capacity) {
        Some(armed) => match rx_completed_word(armed, length, terminal) {
            Some(word0) => word0,
            None => panic!("a modelled length fits the length field"),
        },
        None => panic!("a modelled capacity fits the size field"),
    }
}

/// Fresh single-node TX storage word: `TX_OWNED` and `TX_LAST` set, bit 29
/// clear.
///
/// `capacity` is the complete DMA-visible allocation, the size field.
/// `transfer_length` is the populated source range, the length field. A live
/// vendor q0 observation confirms that TX retains the same
/// capacity/used-length distinction as RX.
pub const fn tx_owned_word(
    capacity: u32,
    transfer_length: u32,
) -> Option<WifiTxDmaDescriptorControl> {
    let Some(capacity) = capacity_field(capacity) else {
        return None;
    };
    if transfer_length == 0 || transfer_length > capacity as u32 {
        return None;
    }
    match WifiTxDmaDescriptorControl::ZERO.with_size(capacity) {
        Some(word0) => match word0.with_length(transfer_length as u16) {
            Some(word0) => Some(word0.with_tx_last(true).with_tx_owned(true)),
            None => None,
        },
        None => None,
    }
}

#[cfg(test)]
mod tests;
