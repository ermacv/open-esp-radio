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
//! A hold is full when it holds every staging lease, or when it is stuck:
//! one lease short, every other buffer staged and detached, every
//! descriptor observed, NEXT zero (no armed descriptor) and LAST the
//! descriptor without a lease at the end of the runner's latest drain, and
//! no unit received since the last lease, for at least the fill timeout.
//! That descriptor's buffer is the walker's frozen LAST unit, still owned
//! by the ring (no buffer released), or released but not yet rearmable
//! because it is LAST (one released; see the receive runner's drain).
//!
//! A lease's descriptor index is `(descriptor_address − ring_base) / 12`,
//! with `ring_base` the DMA address of descriptor 0 that the arena's bind
//! laid out (`WifiRxParts::descriptor_base`, sent with the application's
//! ends); the ring prepared from that arena has the same base
//! (`w1.exhaust ring_base=`).

use oer_esp32s31_hal::ieee80211::dma::descriptor::{
    RxDescriptor, cursor_ring_index, descriptor_ring_index,
};
use oer_esp32s31_ieee80211_driver::rx::counters::{WifiRxDrainEnd, WifiRxExhaustion, WifiRxReport};

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
        let byte = oer_memory::read_volatile(byte);
        crc = CRC_TABLE[usize::from(crc as u8 ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// The ring index of the descriptor at DMA address `address` in the ring
/// at `ring_base`; `None` outside the ring or between descriptors.
pub fn descriptor_index(address: u32, ring_base: u32) -> Option<usize> {
    descriptor_ring_index(address, ring_base, COUNT)
}

/// The ring index of the descriptor whose low address a cursor register
/// read as `low`, in the ring at `ring_base`.
pub fn cursor_index(low: u32, ring_base: u32) -> Option<usize> {
    cursor_ring_index(low, ring_base, COUNT)
}

/// Whether descriptor `index` of `descriptors` carries the owner bit clear,
/// as a detached descriptor must while its lease is held; `false` for an
/// unknown index.
pub fn owner_clear(descriptors: &[RxDescriptor], index: Option<usize>) -> bool {
    index
        .and_then(|index| descriptors.get(index))
        .is_some_and(|descriptor| !descriptor.word0().owner())
}

/// Whether a hold is full, and why.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Full {
    /// Short of every lease, and not stuck.
    #[default]
    No,
    /// Every staging lease held.
    All,
    /// One lease short, the missing buffer at LAST, no armed descriptor.
    StuckAtLast,
}

impl Full {
    pub const fn held(self) -> bool {
        !matches!(self, Self::No)
    }

    /// The reason in `w1.hold`.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::No => "-",
            Self::All => "all",
            Self::StuckAtLast => "stuck-at-last",
        }
    }
}

/// Whether the leases of descriptors `indices` make a full hold. `end` is
/// the runner's latest drain end and `quiet` whether no unit was received
/// since the last lease, for at least the fill timeout.
pub fn fullness(
    indices: &[Option<usize>],
    end: Option<&WifiRxDrainEnd>,
    quiet: bool,
    ring_base: u32,
) -> Full {
    if indices.len() == SLOTS {
        return Full::All;
    }
    let (Some(end), true) = (end, quiet && indices.len() + 1 == SLOTS) else {
        return Full::No;
    };
    // The one ring index without a lease.
    let mut leased = [false; COUNT];
    for index in indices {
        match index.and_then(|index| leased.get_mut(index)) {
            Some(slot) if !*slot => *slot = true,
            _ => return Full::No,
        }
    }
    let missing = leased.iter().position(|leased| !leased);
    let one_short = |count: u32| usize::try_from(count).is_ok_and(|count| count + 1 == SLOTS);
    if one_short(end.staged)
        && one_short(end.detached)
        && end.released <= 1
        && usize::try_from(end.observed).is_ok_and(|observed| observed == COUNT)
        && end.next_low == 0
        && missing.is_some()
        && cursor_index(end.last_low, ring_base) == missing
    {
        Full::StuckAtLast
    } else {
        Full::No
    }
}

/// The reasons a unit was dropped or refused: the runner's drops first
/// (their unit never reaches the application, so its descriptor index is
/// missing from the frames taken), then the application's RX-control
/// prefix refusals (the frame was taken; its index is listed).
pub const DISCARDS: [&str; 15] = [
    "empty",
    "too_long",
    "chained",
    "oversized",
    "overload",
    "rx_failure",
    "mic_failure",
    "quarantined",
    "invalid",
    "prefix_chain",
    "bounds",
    "output_small",
    "metadata",
    "ignored",
    "unsupported",
];

/// The counts of [`DISCARDS`] in `report`.
pub fn discards(report: &WifiRxReport) -> [u32; 15] {
    let prefix = &report.prefix;
    [
        report.empty_drops,
        report.too_long_drops,
        report.chained_drops,
        report.oversized_drops,
        report.overload_drops,
        prefix.rx_failure,
        prefix.mic_failure,
        prefix.quarantined,
        prefix.invalid,
        prefix.chain,
        prefix.bounds,
        prefix.output_small,
        prefix.metadata,
        prefix.ignored,
        prefix.unsupported,
    ]
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
/// once, or every one but one (a stuck hold: the missing descriptor is
/// skipped); the first lease taken is taken for the recycle frontier and
/// the last for the accepted tail. `None` otherwise.
pub fn release_order(
    indices: &[Option<usize>],
    order: Order,
) -> Option<heapless::Vec<usize, COUNT>> {
    if indices.len() + 1 < COUNT || indices.len() > COUNT {
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
    let mut positions = heapless::Vec::new();
    for step in 0..COUNT {
        let index = match order {
            Order::Forward => (start + step) % COUNT,
            Order::Reverse => (start + 2 * COUNT - 1 - step) % COUNT,
        };
        if let Some(position) = position_of.get(index).copied().flatten() {
            let _full = positions.push(position);
        }
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
    /// The fill window the hold had, in ms; zero before the hold.
    pub fill_ms: u32,
    /// Leases held when the hold began.
    pub held: usize,
    /// Whether the hold was full, and the runner's drain end as the fill
    /// ended.
    pub full: Full,
    pub end: Option<WifiRxDrainEnd>,
    pub hold: Hold,
    /// The descriptor index of the first lease dropped; `None` when the
    /// leases' indices were not every ring index (or every one but one)
    /// once (then the leases
    /// dropped in the order they were taken, or its reverse).
    pub first_drop: Option<usize>,
    /// Held leases whose descriptor word0 carried the owner bit clear at
    /// the end of the hold, as every detached descriptor must.
    pub owner_clear: u32,
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
    /// frames after the release, and every [`DISCARDS`] count over the
    /// same window.
    pub chained: u32,
    pub oversized: u32,
    pub discards: [u32; 15],
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

    /// Whether the cycle's hold was full.
    pub const fn exercised(&self) -> bool {
        self.full.held()
    }
}
