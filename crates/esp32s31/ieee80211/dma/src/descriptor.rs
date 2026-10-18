//! ESP32-S31 Wi-Fi DMA descriptor geometry and ownership words.

#![forbid(unsafe_code)]

use vcell::VolatileCell;

pub const DMA_LOW: u32 = 0x2f00_0000;
pub const DMA_HIGH: u32 = 0x2f08_0000;
pub const DESCRIPTOR_BYTES: u32 = 12;

pub const SIZE_MASK: u32 = 0x0000_3fff;
pub const LENGTH_MASK: u32 = 0x0fff_c000;
pub const LENGTH_SHIFT: u32 = 14;
/// RX word0 bit 29. Its meaning is not established: software clears it on
/// rearm ([`rx_rearm_word`]) and an armed stopped-ring descriptor must carry
/// it clear; what the hardware does with it is unknown.
pub const BIT_29_OPAQUE: u32 = 0x2000_0000;
/// RX word0 bit 30: set by hardware on a receive unit's terminal descriptor
/// when the unit completes; a walk is delimited by it through the saved
/// LAST. SOURCE: ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR,
/// `libpp[wdev.o]::wdevProcessRxSucDataAll`
/// (`knowledge/esp32s31/wifi/rx-walker.md`, "Vendor behaviour").
pub const RX_DONE: u32 = 0x4000_0000;
/// RX word0 bit 31, the owner bit: the walker checks it before writing a
/// descriptor and stops on a clear one, without writing it; restoring it
/// and ringing the reload doorbell resumes the walker there. Hardware
/// leaves it set on completion, so completion is [`RX_DONE`], never a
/// cleared owner bit. Software arms with it set and clears it on a
/// detached descriptor ([`rx_detached_word`]).
/// SOURCE\[HIL_W1_OWNER_BIT_2026_10_18] (`knowledge/esp32s31/wifi/rx-walker.md`,
/// "Owner bit").
pub const OWNER: u32 = 0x8000_0000;
/// TX word0 bit 30 as this crate builds TX chains: set on the last
/// descriptor of a chain and cleared on the others
/// ([`crate::tx_ampdu_storage`]). Not the RX [`RX_DONE`]: software sets it.
pub const TX_LAST: u32 = 0x4000_0000;
/// TX word0 bit 31: set by software to hand a TX descriptor to the MAC
/// ([`tx_owned_word`]). Whether the TX engine checks it as the RX walker
/// checks [`OWNER`] is not verified.
pub const TX_OWNED: u32 = 0x8000_0000;

/// The exact three-word descriptor consumed by the Wi-Fi MAC.
///
/// This is deliberately not `esp_hal::dma::DmaDescriptor`: that type is
/// padded to 16 bytes on ESP32-S31 for AXI-GDMA, while Wi-Fi walks 12-byte
/// nodes. `VolatileCell` models words that can change outside Rust through
/// DMA without leaking raw pointers into the MAC backend.
/// Its zero bytes are the empty, software-owned descriptor `new` builds
/// (`bytemuck::Zeroable`, implemented in `rx_storage.rs`).
#[repr(C, align(4))]
pub struct Descriptor {
    word0: VolatileCell<u32>,
    buffer_address: VolatileCell<u32>,
    next_address: VolatileCell<u32>,
}

const _: () = {
    assert!(core::mem::size_of::<Descriptor>() == DESCRIPTOR_BYTES as usize);
    assert!(core::mem::align_of::<Descriptor>() == 4);
};

impl Descriptor {
    pub const fn new() -> Self {
        Self {
            word0: VolatileCell::new(0),
            buffer_address: VolatileCell::new(0),
            next_address: VolatileCell::new(0),
        }
    }

    #[inline]
    pub fn word0(&self) -> u32 {
        self.word0.get()
    }

    #[inline]
    pub fn buffer_address(&self) -> u32 {
        self.buffer_address.get()
    }

    #[inline]
    pub fn next_address(&self) -> u32 {
        self.next_address.get()
    }

    /// Writes address/link first and publishes the ownership word last.
    #[inline]
    pub(crate) fn publish_owned(&self, word0: u32, buffer_address: u32, next_address: u32) {
        self.buffer_address.set(buffer_address);
        self.next_address.set(next_address);
        self.word0.set(word0);
    }

    /// Fault model: memory corruption or a foreign write overwrites all three
    /// descriptor words. It bypasses none of the product's checks.
    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn inject_corrupt_words(&self, word0: u32, buffer_address: u32, next_address: u32) {
        self.publish_owned(word0, buffer_address, next_address);
    }

    /// Model one hardware ownership-word update.
    ///
    /// Production target code cannot call this safe method: live descriptor
    /// mutation belongs exclusively to the ring owner and the DMA actor. It
    /// exists only for native models and the explicit raw-DMA validation
    /// feature used by compiled vendor probes.
    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn write_word0(&self, word0: u32) {
        self.word0.set(word0);
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
    pub(crate) fn publish_next_address(&self, next_address: u32) {
        self.next_address.set(next_address)
    }

    /// Rewrites only the ownership word, keeping the buffer and the link.
    ///
    /// The live RX-ring owner uses it to rearm the reserved descriptor after
    /// a frame landed in it; that descriptor's link is the walker's and is
    /// changed separately, only after the walker left it.
    #[inline]
    pub(crate) fn publish_word0(&self, word0: u32) {
        self.word0.set(word0);
    }
}

impl Default for Descriptor {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
pub const fn dma_range_valid(address: u32, size: u32) -> bool {
    size != 0 && address >= DMA_LOW && address < DMA_HIGH && size <= DMA_HIGH - address
}

#[inline]
pub const fn descriptor_address_valid(address: u32) -> bool {
    address & 3 == 0 && dma_range_valid(address, DESCRIPTOR_BYTES)
}

#[inline]
pub const fn size(word0: u32) -> u32 {
    word0 & SIZE_MASK
}

#[inline]
pub const fn length(word0: u32) -> u32 {
    (word0 & LENGTH_MASK) >> LENGTH_SHIFT
}

#[inline]
pub const fn rx_done(word0: u32) -> bool {
    word0 & RX_DONE != 0
}

/// Whether an RX word carries the [`OWNER`] bit the walker checks.
#[inline]
pub const fn rx_owner(word0: u32) -> bool {
    word0 & OWNER != 0
}

/// A completed RX word as it stays while its buffer is detached: the same
/// image with [`OWNER`] clear, so a walker that reached it would stop on it
/// instead of writing the leased buffer (SOURCE\[HIL_W1_OWNER_BIT_2026_10_18]).
/// [`RX_DONE`], the length and the size stay.
#[inline]
pub const fn rx_detached_word(word0: u32) -> u32 {
    word0 & !OWNER
}

/// Fresh/recycled RX word: [`OWNER`] set, [`RX_DONE`] and [`BIT_29_OPAQUE`]
/// clear, length equals capacity.
///
/// S31 keeps [`OWNER`] set when it completes RX and sets [`RX_DONE`] as the
/// completion marker (SOURCE\[HIL_W1_OWNER_BIT_2026_10_18]). Software
/// ownership must therefore be tested with [`rx_done`], not by waiting for
/// the owner bit to clear.
pub const fn rx_armed_word(capacity: u32) -> Option<u32> {
    if capacity == 0 || capacity > SIZE_MASK {
        None
    } else {
        Some(capacity | (capacity << LENGTH_SHIFT) | OWNER)
    }
}

/// Rearms a hardware-completed or detached RX word while preserving unrelated
/// bits: [`OWNER`] is set again, [`RX_DONE`] and [`BIT_29_OPAQUE`] cleared.
pub const fn rx_rearm_word(word0: u32) -> Option<u32> {
    let capacity = size(word0);
    if capacity == 0 {
        None
    } else {
        Some(
            (word0 & !(LENGTH_MASK | BIT_29_OPAQUE | RX_DONE)) | OWNER | (capacity << LENGTH_SHIFT),
        )
    }
}

/// Fresh single-node TX storage word: [`TX_OWNED`] and [`TX_LAST`] set,
/// bit 29 clear.
///
/// `capacity` is the complete DMA-visible allocation encoded in the low 14
/// bits. `transfer_length` is the populated source range encoded in the high
/// length field. A live vendor q0 observation confirms that TX retains the
/// same capacity/used-length distinction as RX.
pub const fn tx_owned_word(capacity: u32, transfer_length: u32) -> Option<u32> {
    if capacity == 0
        || capacity > SIZE_MASK
        || transfer_length == 0
        || transfer_length > capacity
        || transfer_length > SIZE_MASK
    {
        None
    } else {
        Some(capacity | (transfer_length << LENGTH_SHIFT) | TX_LAST | TX_OWNED)
    }
}
