//! The Espressif transmit-rate schedule arenas and their record walk.
//!
//! The nine vendor arenas of `libpp.a[trc.o]` are fixed arrays of 12-byte
//! records. `rcAttach` performs only one mutation: it writes each record's
//! zero-based arena index to byte `0x0a`. The definitions below materialize
//! that post-attach state at compile time, so no runtime needs the vendor
//! tables or their initializer. Each record starts with four
//! `(rate code, attempt count)` pairs in the `wifi_phy_rate_t` code space
//! ([`crate::rate_code`]); [`schedule_rate_after_failures`] is the vendor's
//! walk of them. The bytes were recovered from the pinned ESP32-S31 `libpp.a`
//! (`chips/esp32s31/vendor/artifacts.toml`); a chip whose pinned library
//! carries other arenas needs its own reviewed copy.

#[cfg(test)]
const RATE_SCHEDULE_KINDS: [RateScheduleKind; 9] = [
    RateScheduleKind::BarOfdm,
    RateScheduleKind::BasicOfdm,
    RateScheduleKind::Dot11Ax,
    RateScheduleKind::Dot11B,
    RateScheduleKind::Dot11G,
    RateScheduleKind::Dot11N,
    RateScheduleKind::Lora,
    RateScheduleKind::P2pDot11G,
    RateScheduleKind::P2pDot11N,
];

use crate::rate_record::{RATE_SCHEDULE_RECORD_SIZE, RateScheduleRecordState, RateScheduleRef};
use oer_ieee80211_port::backend::{
    RateScheduleKind,
};

#[repr(C, align(4))]
struct ScheduleArena<const RECORDS: usize>([[u8; RATE_SCHEDULE_RECORD_SIZE]; RECORDS]);

impl<const RECORDS: usize> ScheduleArena<RECORDS> {
    const fn indexed(mut bytes: [[u8; RATE_SCHEDULE_RECORD_SIZE]; RECORDS]) -> Self {
        let mut records: &mut [[u8; RATE_SCHEDULE_RECORD_SIZE]] = &mut bytes;
        let mut index = 0_u8;
        while let Some((record, rest)) = records.split_first_mut() {
            let [_, _, _, _, _, _, _, _, _, _, tag, _] = record;
            *tag = index;
            // Only the private static arenas below call this constructor;
            // the largest has 16 records, including its final increment.
            index = index.wrapping_add(1);
            records = rest;
        }
        Self(bytes)
    }
}

static BAR_OFDM_SCHEDULE: ScheduleArena<1> = ScheduleArena::indexed([[
    0x0b, 0x02, 0x0b, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x00, 0x00, 0x00,
]]);

static BASIC_OFDM_SCHEDULE: ScheduleArena<1> = ScheduleArena::indexed([[
    0x0b, 0x02, 0x0b, 0x02, 0x0b, 0x03, 0x00, 0x19, 0x20, 0x00, 0x00, 0x00,
]]);

static DOT11_AX_SCHEDULE: ScheduleArena<16> = ScheduleArena::indexed([
    [
        0x23, 0x02, 0x22, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x19, 0x02, 0x17, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x18, 0x02, 0x16, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x17, 0x02, 0x15, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x16, 0x02, 0x14, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x15, 0x02, 0x13, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x60, 0x00, 0x00,
    ],
    [
        0x14, 0x02, 0x12, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x55, 0x00, 0x00,
    ],
    [
        0x13, 0x02, 0x12, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x4b, 0x00, 0x00,
    ],
    [
        0x12, 0x02, 0x11, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
    [
        0x11, 0x02, 0x0b, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x3b, 0x00, 0x00,
    ],
    [
        0x10, 0x02, 0x0b, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x32, 0x00, 0x00,
    ],
    [
        0x06, 0x02, 0x06, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x05, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x00, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x1e, 0x00, 0x00,
    ],
    [
        0x2a, 0x08, 0x2a, 0x08, 0x2a, 0x02, 0x29, 0x0e, 0x20, 0x14, 0x00, 0x00,
    ],
    [
        0x29, 0x02, 0x29, 0x02, 0x29, 0x03, 0x29, 0x19, 0x20, 0x00, 0x00, 0x00,
    ],
]);

static DOT11_B_SCHEDULE: ScheduleArena<6> = ScheduleArena::indexed([
    [
        0x07, 0x02, 0x06, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
    [
        0x06, 0x02, 0x06, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x05, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x00, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x1e, 0x00, 0x00,
    ],
    [
        0x2a, 0x08, 0x2a, 0x08, 0x2a, 0x02, 0x29, 0x0e, 0x20, 0x14, 0x00, 0x00,
    ],
    [
        0x29, 0x02, 0x29, 0x02, 0x29, 0x03, 0x29, 0x19, 0x20, 0x00, 0x00, 0x00,
    ],
]);

static DOT11_G_SCHEDULE: ScheduleArena<13> = ScheduleArena::indexed([
    [
        0x0c, 0x02, 0x08, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x08, 0x02, 0x0d, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x65, 0x00, 0x00,
    ],
    [
        0x0d, 0x02, 0x09, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x5c, 0x00, 0x00,
    ],
    [
        0x09, 0x02, 0x0e, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x58, 0x00, 0x00,
    ],
    [
        0x0e, 0x02, 0x0a, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x52, 0x00, 0x00,
    ],
    [
        0x0a, 0x02, 0x0b, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x4b, 0x00, 0x00,
    ],
    [
        0x0f, 0x02, 0x0b, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x38, 0x00, 0x00,
    ],
    [
        0x0b, 0x02, 0x06, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
    [
        0x06, 0x02, 0x06, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x05, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x00, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x1e, 0x00, 0x00,
    ],
    [
        0x2a, 0x08, 0x2a, 0x08, 0x2a, 0x02, 0x29, 0x0e, 0x20, 0x14, 0x00, 0x00,
    ],
    [
        0x29, 0x02, 0x29, 0x02, 0x29, 0x03, 0x29, 0x19, 0x20, 0x00, 0x00, 0x00,
    ],
]);

static DOT11_N_SCHEDULE: ScheduleArena<14> = ScheduleArena::indexed([
    [
        0x21, 0x02, 0x16, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x17, 0x02, 0x15, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x16, 0x02, 0x14, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x15, 0x02, 0x13, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x60, 0x00, 0x00,
    ],
    [
        0x14, 0x02, 0x12, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x55, 0x00, 0x00,
    ],
    [
        0x13, 0x02, 0x12, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x4b, 0x00, 0x00,
    ],
    [
        0x12, 0x02, 0x11, 0x02, 0x0b, 0x03, 0x06, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
    [
        0x11, 0x02, 0x0b, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x3b, 0x00, 0x00,
    ],
    [
        0x10, 0x02, 0x0b, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x32, 0x00, 0x00,
    ],
    [
        0x06, 0x02, 0x06, 0x02, 0x06, 0x03, 0x06, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x05, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x39, 0x00, 0x00,
    ],
    [
        0x00, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x1e, 0x00, 0x00,
    ],
    [
        0x2a, 0x08, 0x2a, 0x08, 0x2a, 0x02, 0x29, 0x0e, 0x20, 0x14, 0x00, 0x00,
    ],
    [
        0x29, 0x02, 0x29, 0x02, 0x29, 0x03, 0x29, 0x19, 0x20, 0x00, 0x00, 0x00,
    ],
]);

static LORA_SCHEDULE: ScheduleArena<2> = ScheduleArena::indexed([
    [
        0x2a, 0x08, 0x2a, 0x08, 0x2a, 0x02, 0x29, 0x0e, 0x20, 0x14, 0x00, 0x00,
    ],
    [
        0x29, 0x02, 0x29, 0x02, 0x29, 0x03, 0x29, 0x19, 0x20, 0x00, 0x00, 0x00,
    ],
]);

static P2P_DOT11_G_SCHEDULE: ScheduleArena<8> = ScheduleArena::indexed([
    [
        0x0c, 0x02, 0x08, 0x02, 0x0d, 0x03, 0x0b, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x08, 0x02, 0x0d, 0x02, 0x09, 0x03, 0x0b, 0x19, 0x20, 0x65, 0x00, 0x00,
    ],
    [
        0x0d, 0x02, 0x09, 0x02, 0x0e, 0x03, 0x0b, 0x19, 0x20, 0x5c, 0x00, 0x00,
    ],
    [
        0x09, 0x02, 0x0e, 0x02, 0x0a, 0x03, 0x0b, 0x19, 0x20, 0x58, 0x00, 0x00,
    ],
    [
        0x0e, 0x02, 0x0a, 0x02, 0x0f, 0x03, 0x0b, 0x19, 0x20, 0x52, 0x00, 0x00,
    ],
    [
        0x0a, 0x02, 0x0f, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x4b, 0x00, 0x00,
    ],
    [
        0x0f, 0x02, 0x0b, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x38, 0x00, 0x00,
    ],
    [
        0x0b, 0x02, 0x0b, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
]);

static P2P_DOT11_N_SCHEDULE: ScheduleArena<10> = ScheduleArena::indexed([
    [
        0x21, 0x02, 0x16, 0x02, 0x15, 0x03, 0x0b, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x17, 0x02, 0x15, 0x02, 0x14, 0x03, 0x0b, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x16, 0x02, 0x14, 0x02, 0x13, 0x03, 0x0b, 0x19, 0x20, 0x72, 0x00, 0x00,
    ],
    [
        0x15, 0x02, 0x13, 0x02, 0x12, 0x03, 0x0b, 0x19, 0x20, 0x60, 0x00, 0x00,
    ],
    [
        0x14, 0x02, 0x12, 0x02, 0x11, 0x03, 0x0b, 0x19, 0x20, 0x55, 0x00, 0x00,
    ],
    [
        0x13, 0x02, 0x12, 0x02, 0x11, 0x03, 0x0b, 0x19, 0x20, 0x4b, 0x00, 0x00,
    ],
    [
        0x12, 0x02, 0x11, 0x02, 0x10, 0x03, 0x0b, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
    [
        0x11, 0x02, 0x10, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x3b, 0x00, 0x00,
    ],
    [
        0x10, 0x02, 0x0b, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x32, 0x00, 0x00,
    ],
    [
        0x0b, 0x02, 0x0b, 0x02, 0x0b, 0x03, 0x0b, 0x19, 0x20, 0x40, 0x00, 0x00,
    ],
]);

pub const fn record_count(kind: RateScheduleKind) -> usize {
    match kind {
        RateScheduleKind::BarOfdm | RateScheduleKind::BasicOfdm => 1,
        RateScheduleKind::Dot11Ax => 16,
        RateScheduleKind::Dot11B => 6,
        RateScheduleKind::Dot11G => 13,
        RateScheduleKind::Dot11N => 14,
        RateScheduleKind::Lora => 2,
        RateScheduleKind::P2pDot11G => 8,
        RateScheduleKind::P2pDot11N => 10,
    }
}

/// Validate the arena position once, retaining the exact record array.
pub const fn rate_schedule(kind: RateScheduleKind, index: u8) -> Option<RateScheduleRef> {
    let mut records = arena_records(kind);
    let mut at = 0_u8;
    while let Some((record, rest)) = records.split_first() {
        if at == index {
            return Some(RateScheduleRef::from_record(kind, record));
        }
        records = rest;
        // The largest private arena has 16 records, so at never wraps.
        at = at.wrapping_add(1);
    }
    None
}

pub const fn offset_schedule(schedule: RateScheduleRef, records: u8) -> Option<RateScheduleRef> {
    match schedule.index().checked_add(records) {
        Some(index) => rate_schedule(schedule.kind(), index),
        None => None,
    }
}

const fn arena_records(kind: RateScheduleKind) -> &'static [[u8; RATE_SCHEDULE_RECORD_SIZE]] {
    match kind {
        RateScheduleKind::BarOfdm => &BAR_OFDM_SCHEDULE.0,
        RateScheduleKind::BasicOfdm => &BASIC_OFDM_SCHEDULE.0,
        RateScheduleKind::Dot11Ax => &DOT11_AX_SCHEDULE.0,
        RateScheduleKind::Dot11B => &DOT11_B_SCHEDULE.0,
        RateScheduleKind::Dot11G => &DOT11_G_SCHEDULE.0,
        RateScheduleKind::Dot11N => &DOT11_N_SCHEDULE.0,
        RateScheduleKind::Lora => &LORA_SCHEDULE.0,
        RateScheduleKind::P2pDot11G => &P2P_DOT11_G_SCHEDULE.0,
        RateScheduleKind::P2pDot11N => &P2P_DOT11_N_SCHEDULE.0,
    }
}

fn schedule_bytes(schedule: RateScheduleRef) -> &'static [u8; RATE_SCHEDULE_RECORD_SIZE] {
    schedule.record()
}

pub fn schedule_state(schedule: RateScheduleRef) -> RateScheduleRecordState {
    let [rate, retry_limit, _, _, _, _, _, _, _, _, index, adaptive] = *schedule_bytes(schedule);
    RateScheduleRecordState {
        rate,
        retry_limit,
        index,
        adaptive,
    }
}

/// Return the complete ordinary-MPDU publication budget carried by one
/// vendor rate record.
///
/// SOURCE(esp32s31): complete `libpp[trc.o]::rcReachRetryLimit`. Outside the mesh
/// special case it compares the MPDU retry counter with record byte `0x08`;
/// this is independent of byte `0x01`, which `libpp[trc.o]::rcTxUpdatePer` consumes as its
/// PER/rate-control threshold.
pub fn schedule_publication_limit(schedule: RateScheduleRef) -> u8 {
    let [_, _, _, _, _, _, _, _, limit, _, _, _] = *schedule_bytes(schedule);
    limit
}

/// Select the rate byte for a non-aggregate retry counter.
///
/// Each vendor record starts with four `(rate, attempt_count)` pairs.
/// `rcGetRate` accumulates the four counts and selects the first rate for which
/// `failed_attempts < cumulative_count`. Returning `None` means that the
/// record's complete retry budget has been consumed.
///
/// SOURCE(esp32s31): complete pinned `libpp[trc.o]::rcGetRate` (`0xf6` bytes),
/// especially the bounded four-iteration loop at offsets `0xa8..0xf4`;
/// cross-checked against `SOURCE(esp32s31)[PROMOTED_LMAC_TX]`. After the loop the
/// pinned body replaces a selected code of `0x24` or above with `0x10` when
/// descriptor byte `+0x32` bit zero marks an HT-Control MPDU (set only by
/// `libnet80211[ieee80211_he.o]::ieee80211_encap_esfbuf_htc`). No record this driver selects through the
/// loop holds such a code together with that flag, so the rewrite never
/// applies here.
pub fn schedule_rate_after_failures(schedule: RateScheduleRef, failed_attempts: u8) -> Option<u8> {
    let record = schedule_bytes(schedule);
    // Four byte-sized counts fit in u16 even for the largest record values.
    let mut cumulative = 0_u16;
    let [r0, c0, r1, c1, r2, c2, r3, c3, ..] = *record;
    for (rate, count) in [(r0, c0), (r1, c1), (r2, c2), (r3, c3)] {
        cumulative = cumulative.wrapping_add(u16::from(count));
        if u16::from(failed_attempts) < cumulative {
            return Some(rate);
        }
    }
    None
}

/// The `libpp.a[trc.o]` rate-code-to-record callbacks of `rcUpdatePhyMode`.
///
/// The numeric values are still part of the vendor ABI. Keeping the family
/// separate from the callback address prevents safe policy from
/// manufacturing or following a C function pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateIndexMap {
    Dot11B,
    Dot11G,
    Dot11N,
    Dot11Ax,
    Lora,
}

/// Pure rate-code callback policy stored temporarily at record offset 0x78.
///
/// Codes without a record in the selected family return `None`.
/// Callers must handle absence before using an arena index.
///
/// ```compile_fail,E0308
/// use oer_espressif_ieee80211_policy::rate_schedule::{RateIndexMap,rate_to_schedule_index};
/// let index: u8 = rate_to_schedule_index(RateIndexMap::Dot11N, u8::MAX);
/// ```
///
/// ```
/// use oer_espressif_ieee80211_policy::rate_schedule::{RateIndexMap,rate_to_schedule_index};
/// assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, u8::MAX), None);
/// assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x21), Some(0));
/// ```
pub const fn rate_to_schedule_index(map: RateIndexMap, rate: u8) -> Option<u8> {
    let index = match map {
        RateIndexMap::Dot11B => match rate {
            0 => Some(3),
            1 | 5 => Some(2),
            2 | 6 => Some(1),
            3 | 7 => Some(0),
            41 => Some(5),
            42 => Some(4),
            _ => None,
        },
        RateIndexMap::Dot11G => {
            // SOURCE(esp32s31): `libpp.a[trc.o]` `.rodata` switch table of
            // `rc11GRate2SchedIdx`, including the long-range codes 0x29 and
            // 0x2a that select 802.11g records 12 and 11.
            match rate {
                0 => Some(10),
                1 | 5 => Some(9),
                2 | 6 => Some(8),
                8 => Some(1),
                9 => Some(3),
                10 => Some(5),
                11 => Some(7),
                12 => Some(0),
                13 => Some(2),
                14 => Some(4),
                15 => Some(6),
                41 => Some(12),
                42 => Some(11),
                _ => None,
            }
        }
        RateIndexMap::Dot11N => match rate {
            0 => Some(11),
            1 => Some(10),
            2 => Some(9),
            5 => Some(10),
            6 => Some(9),
            0x21 => Some(0),
            0x29 => Some(13),
            0x2a => Some(12),
            _ => 24_u8.checked_sub(rate),
        },
        RateIndexMap::Dot11Ax => match rate {
            0 => Some(13),
            1 => Some(12),
            2 => Some(11),
            5 => Some(12),
            6 => Some(11),
            0x17 => Some(3),
            0x19 => Some(1),
            0x22 => Some(2),
            0x23 => Some(0),
            0x29 => Some(15),
            0x2a => Some(14),
            _ => 26_u8.checked_sub(rate),
        },
        RateIndexMap::Lora => match rate {
            0x29 => Some(1),
            0x2a => Some(0),
            _ => None,
        },
    };
    let kind = match map {
        RateIndexMap::Dot11B => RateScheduleKind::Dot11B,
        RateIndexMap::Dot11G => RateScheduleKind::Dot11G,
        RateIndexMap::Dot11N => RateScheduleKind::Dot11N,
        RateIndexMap::Dot11Ax => RateScheduleKind::Dot11Ax,
        RateIndexMap::Lora => RateScheduleKind::Lora,
    };
    match index {
        Some(index) if (index as usize) < record_count(kind) => Some(index),
        _ => None,
    }
}

/// Locate the complete vendor 802.11g retry record for a legacy rate code.
///
/// SOURCE(esp32s31): `libpp.a[trc.o]` callback table used by `rcUpdatePhyMode`, the
/// [`RateIndexMap::Dot11G`] branch of [`rate_to_schedule_index`]; the
/// pointed-to record bytes come from the arenas above.
pub const fn dot11g_schedule_for_legacy_rate(rate: u8) -> Option<RateScheduleRef> {
    match rate_to_schedule_index(RateIndexMap::Dot11G, rate) {
        Some(index) => rate_schedule(RateScheduleKind::Dot11G, index),
        None => None,
    }
}

#[cfg(test)]
mod tests;
