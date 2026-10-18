//! The ownership experiment of the hold: does the walker write into a
//! buffer the application holds?
//!
//! Each hold cycle fingerprints every lease it takes (a CRC-32 of the
//! frame bytes the lease shows, `segment().buffer`, its length and its
//! descriptor index), rechecks every fingerprint at the end of the hold,
//! drops one lease, dwells without taking a frame, rechecks the rest, then
//! drops them one at a time, each rechecked just before its drop. Two
//! cycles run: `forward` drops from the recycle frontier in ring order,
//! `reverse` from the accepted tail against it. Report-only but for the
//! `ownership` gate.
//!
//! A lease's descriptor index is `(descriptor_address − ring_base) / 12`,
//! with `ring_base` the DMA address of descriptor 0 that the arena's bind
//! laid out (`WifiRxParts::descriptor_base`, sent with the application's
//! ends); the ring prepared from that arena has the same base
//! (`w1.exhaust ring_base=`).

use oer_esp32s31_ieee80211::rx::counters::WifiRxExhaustion;
use oer_esp32s31_ieee80211_dma::descriptor::DESCRIPTOR_BYTES;

use crate::{
    irq::{COUNT, SLOTS},
    report::Hold,
};

/// Hold cycles per run.
pub const CYCLES: usize = 2;
/// Frames after a release whose descriptor index is listed.
pub const FIRST_AFTER: usize = 16;

// Every descriptor carries one lease when the pool is full.
const _: () = assert!(COUNT == SLOTS);

/// The order a cycle releases its leases in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Order {
    /// From the recycle frontier (the first lease taken) in ring order;
    /// the accepted tail's lease drops last.
    #[default]
    Forward,
    /// From the accepted tail (the last lease taken) against ring order.
    Reverse,
}

impl Order {
    /// The cycle's order: forward first, then reverse.
    pub const fn of(cycle: usize) -> Self {
        if cycle == 0 {
            Self::Forward
        } else {
            Self::Reverse
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
        }
    }
}

/// The CRC-32 (IEEE, reflected) table.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0_u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

/// The CRC-32 of `bytes`, each byte read volatile: the bytes are a DMA
/// buffer the hardware must not write, and a write is what the experiment
/// looks for, so no read may be folded into an earlier one.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        // SAFETY: `byte` is a shared borrow of an initialized byte of the
        // lease's frame, valid and aligned for the read; a volatile read
        // only reads it.
        let byte = unsafe { core::ptr::read_volatile(byte) };
        crc = CRC_TABLE[usize::from(crc as u8 ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// The ring index of the descriptor at DMA address `address` in the ring
/// at `ring_base`; `None` outside the ring or between descriptors.
pub fn descriptor_index(address: u32, ring_base: u32) -> Option<usize> {
    let offset = address.checked_sub(ring_base)?;
    if offset % DESCRIPTOR_BYTES != 0 {
        return None;
    }
    let index = usize::try_from(offset / DESCRIPTOR_BYTES).ok()?;
    (index < COUNT).then_some(index)
}

/// One lease as it was taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fingerprint {
    pub crc: u32,
    pub len: usize,
    pub index: Option<usize>,
}

impl Fingerprint {
    /// The fingerprint of a lease showing `bytes`, staged from the
    /// descriptor at `descriptor_address`.
    pub fn of(bytes: &[u8], descriptor_address: u32, ring_base: u32) -> Self {
        Self {
            crc: crc32(bytes),
            len: bytes.len(),
            index: descriptor_index(descriptor_address, ring_base),
        }
    }

    /// Whether `bytes`, the lease's bytes now, differ from the fingerprint.
    pub fn changed(&self, bytes: &[u8]) -> bool {
        bytes.len() != self.len || crc32(bytes) != self.crc
    }
}

/// The positions (in the order the leases were taken) of a cycle's leases
/// in release order, when their descriptor indices are every ring index
/// once; the first lease taken is taken for the recycle frontier and the
/// last for the accepted tail. `None` otherwise.
pub fn release_order(indices: &[Option<usize>], order: Order) -> Option<[usize; COUNT]> {
    if indices.len() != COUNT {
        return None;
    }
    let mut position_of = [None; COUNT];
    for (position, index) in indices.iter().enumerate() {
        let slot = position_of.get_mut((*index)?)?;
        if slot.is_some() {
            return None;
        }
        *slot = Some(position);
    }
    let start = (*indices.first()?)?;
    let mut positions = [0; COUNT];
    for (step, position) in positions.iter_mut().enumerate() {
        let index = match order {
            Order::Forward => (start + step) % COUNT,
            Order::Reverse => (start + 2 * COUNT - 1 - step) % COUNT,
        };
        *position = (*position_of.get(index)?)?;
    }
    Some(positions)
}

/// Pairs of consecutive frames whose descriptors are not consecutive in the
/// ring (an index outside the ring breaks both of its pairs).
pub fn order_breaks(indices: &[Option<usize>]) -> u32 {
    let breaks = indices
        .windows(2)
        .filter(|pair| match pair {
            [Some(first), Some(second)] => *second != (first + 1) % COUNT,
            _ => true,
        })
        .count();
    u32::try_from(breaks).unwrap_or(u32::MAX)
}

/// Where a lease's fingerprint was rechecked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Check {
    /// At the end of the hold, before any drop.
    Hold,
    /// After the dwell that followed the first drop.
    Dwell,
    /// Just before the lease's own drop.
    Release,
}

/// One hold cycle.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Cycle {
    pub order: Order,
    /// Leases held when the hold began.
    pub held: usize,
    pub hold: Hold,
    /// The descriptor index of the first lease dropped; `None` when the
    /// leases' indices were not every ring index once (then the leases
    /// dropped in the order they were taken, or its reverse).
    pub first_drop: Option<usize>,
    pub hold_mismatch: u32,
    pub dwell_mismatch: u32,
    pub release_mismatch: u32,
    /// The descriptor index of the first changed lease; `Some(None)` when
    /// its index is unknown.
    pub first_mismatch: Option<Option<usize>>,
    /// The descriptor indices of the first frames after the release.
    pub first_after_release: heapless::Vec<Option<usize>, FIRST_AFTER>,
    pub order_breaks: u32,
    /// Chained and oversized drops from the first drop to the end of the
    /// frames after the release.
    pub chained: u32,
    pub oversized: u32,
    /// Frames taken after the last drop.
    pub frames_after_release: u32,
    /// The exhaustion snapshots counted at the end of the dwell, and the
    /// latest one when the first drop added one.
    pub snapshots: u32,
    pub exhaust: Option<WifiRxExhaustion>,
}

impl Cycle {
    pub fn new(order: Order) -> Self {
        Self {
            order,
            ..Self::default()
        }
    }

    /// Count a changed lease of descriptor `index` found at `check`.
    pub fn mismatch(&mut self, check: Check, index: Option<usize>) {
        let count = match check {
            Check::Hold => &mut self.hold_mismatch,
            Check::Dwell => &mut self.dwell_mismatch,
            Check::Release => &mut self.release_mismatch,
        };
        *count = count.saturating_add(1);
        if self.first_mismatch.is_none() {
            self.first_mismatch = Some(index);
        }
    }

    /// Whether the cycle held every lease.
    pub const fn exercised(&self) -> bool {
        self.held >= SLOTS
    }
}
