//! TEMPORARY experiment (`tasks/165`, branch `probe/rx-rebind-2`, never
//! merged): one live RX ring run in segments that alternate a fixed binding
//! and a rebind, each under a hold-back of 8 and of 1 descriptors
//! ([`ARMS`]). A unit is taken only when at least the hold's completed
//! descriptors lie between it and the frozen LAST and NEXT does not name it;
//! LAST, the descriptor NEXT names and R are never touched. In a rebind
//! segment every such unit is rebound; in a fixed one, one detach every
//! [`ACTION_MS`] checks the window. A retired spare is held two laps, its
//! CRC checked and returned to the bank; every [`KEEP_EVERY`]th is kept to
//! the run's end, and spares from [`POOL_END`] are never bound.
//!
//! Every buffer the walker can reach is a spare of the bank; the probe
//! reads spares only by the bank's volatile copies.
use super::{BUFFER, COUNT, Spares, Storage, usb};
use crate::case_c::{Case, byte};
use crate::crc::{CRC32, crc32};
use crate::verdict::{ARMS, Act, Counts, Kind, Observations, RESERVE_BUCKETS, SEGMENTS};
use bytemuck::Zeroable;
use core::fmt::Write as _;
use oer_chips_esp32s31_hal::{
    radio::ieee80211::{
        dma::{
            descriptor::cursor_ring_index,
            rx_rebind::{RX_REBIND_SPARES, RxBinding},
            rx_ring::{RxFrozenCursor, RxRingError, RxRingLive, RxSegment},
            rx_storage::RxDmaCompletedUnit,
        },
        registers::{MacInterruptMask, MacRxInterrupts},
    },
    system::owner::{MacInterruptRegisters, RadioAccess},
};
use oer_chips_esp32s31_radio_ieee80211_backend::{
    mac::rx::{FCS_SIZE, RxIngressConfig, first_segment_layout},
    rx::receive::RELOAD_SETTLE_LIMIT,
};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::zeroed::ZeroedStatic;
use oer_portable_time::{Clock, Duration, Instant, settle::Settle};

#[cfg(not(feature = "long-segments"))]
const SEGMENT_MS: u64 = 2500;
/// `tasks/234`: c1's full run, a thousand exits a kind of air.
#[cfg(feature = "long-segments")]
const SEGMENT_MS: u64 = 20_000;
/// One detach (and, in a rebind segment, one rebind) at most this often.
const ACTION_MS: u32 = 80;
/// The longest wait for the sender's first frame.
const FIRST_UNIT_S: u32 = 120;
const UNITS_PER_SCAN: usize = 8;
const RETIRED_CHECKS_PER_SCAN: usize = 2;
const SPARE_CHECKS_PER_SCAN: usize = 1;
const VIOLATION_CAPACITY: usize = 16;
/// The pool the rebinds draw from and return to; spares from here on are
/// never bound and keep their canary to the run's end.
const POOL_END: usize = 176;
/// Every so many retirements one is kept to the run's end, until
/// [`KEPT_MAX`] are kept: the rest return, so the pool is not drained.
const KEEP_EVERY: u32 = 8;
const KEPT_MAX: usize = 32;
/// A retired spare returns after this many descriptors: two laps.
const RETURN_AFTER: u32 = 2 * COUNT as u32;
/// The Retry flag of Frame Control's second octet (IEEE 802.11-2020
/// 9.2.4.1.6), the MAC's.
const RETRY: u8 = 0x08;
/// Bytes compared between the HAL's copy and the bound spare.
const COPY_BYTES: usize = 64;
/// The words of one spare.
const WORDS: usize = BUFFER / 4;
const _: () = {
    if !BUFFER.is_multiple_of(4)
        || COPY_BYTES > BUFFER
        || COUNT > RX_REBIND_SPARES
        || POOL_END > RX_REBIND_SPARES
        || POOL_END <= COUNT
    {
        panic!("experiment geometry");
    }
};

/// `tasks/194`: the receive header's word the MAC writes late on a unit at
/// LAST (bytes 48 to 51), and the header's length (the RX-control prefix).
const LATE_WORD: usize = 12;
const HEADER_BYTES: usize = 56;
/// How long a take waits for the late words, in cycles of `mcycle` at
/// 320 MHz: 256 us (`tasks/196`: 194 saw about 114 us as a bound).
const LATE_WINDOW_CYCLES: u32 = 256 * 320;
/// Late-write delay buckets of 4 us, the last one 252 us or more.
const CYCLES_PER_BUCKET: u32 = 4 * 320;
const DELAY_BUCKETS: usize = 64;
/// A unit not at LAST is watched this long only (16 us): its late words
/// are expected written at the take, which the value at the take answers.
const NOT_LAST_WINDOW_CYCLES: u32 = 16 * 320;
/// The receive buffer's tail after the frame (`tasks/196`).
const TAIL_BYTES: usize = 8;
/// The violation kinds counted (`Kind` codes 1 to 8, and 9 for a copy
/// difference outside the known late word).
const KINDS: usize = 10;

/// `tasks/200`: the sequence numbers an exhaust event keeps, before the ring
/// ran out and after it was restored.
const EXHAUST_SNS: usize = 64;
/// The pause with the ring out while the sender goes on.
const EXHAUST_PAUSE_US: u64 = 20_000;
/// The longest hold before an event is recorded as out regardless.
const EXHAUST_HOLD_LIMIT_US: u64 = 500_000;

/// `tasks/200`: where an exhaust segment is.
#[derive(Clone, Copy, PartialEq, Eq, Zeroable)]
#[repr(u32)]
enum Exhaust {
    /// Not an exhaust segment: every unit recycles.
    Idle = 0,
    /// Every taken unit is kept; nothing is re-armed.
    Holding = 1,
    /// The ring ran out; the pause runs, still nothing re-armed.
    Out = 2,
    /// The kept units re-armed; ordinary service.
    Restored = 3,
}

/// One exhaust segment's record.
#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct ExhaustEvent {
    /// Microseconds since the segments' clock start.
    hold_us: u32,
    out_us: u32,
    restore_us: u32,
    /// Units kept when the ring was found out.
    held: u32,
    /// Ring indices of NEXT and LAST at the exhaustion; `u32::MAX` none.
    next: u32,
    last: u32,
    /// Whether the hold limit ended the hold rather than an empty ring.
    by_limit: u32,
    /// `tasks/219` c2: the accepted tail's ring index and the words of it
    /// and of the descriptor NEXT names, at the exhaustion (an exhaust
    /// segment) or at the segment's first exit (an exit segment).
    tail: u32,
    tail_words: [u32; 3],
    next_words: [u32; 3],
    /// `tasks/234`: the hole's ring index and words at the first exit.
    hole: u32,
    hole_words: [u32; 3],
    /// `tasks/219` c1: the exits of an exit segment, by what NEXT named
    /// when the walker stood: nothing, the hole, another descriptor.
    exits: u32,
    exit_next_zero: u32,
    exit_next_hole: u32,
    exit_next_other: u32,
    /// The MAC's counters (MPDU, FCS error, buffer full, data success).
    hw_at_hold: [u16; 4],
    hw_at_out: [u16; 4],
    hw_at_restore: [u16; 4],
    before: [u16; EXHAUST_SNS],
    before_len: u32,
    after: [u16; EXHAUST_SNS],
    after_len: u32,
    /// The microseconds of the first sequence number after the restore.
    after_first_us: u32,
}

/// The late word of one class of takes (at LAST or not).
#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct LateWord {
    takes: u32,
    /// Takes whose late words already read non-zero at the take.
    written_at_take: u32,
    changed: u32,
    never: u32,
    min: u32,
    max: u32,
    delay: [u32; DELAY_BUCKETS],
}

/// One act's descriptor words (control, buffer, next) before the detach
/// and after the rebind, with the frozen cursor's NEXT and LAST lows.
#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct ActSnapshot {
    before: [u32; 3],
    after: [u32; 3],
    next_low: u32,
    last_low: u32,
    valid: u32,
}

/// The canary of spare `spare`'s word `word`.
pub(super) const fn canary(spare: usize, word: usize) -> u32 {
    0x5a00_0000 | ((spare as u32 & 0xff) << 16) | (word as u32 & 0xffff)
}

#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct Violation {
    kind: u32,
    segment: u32,
    spare: u32,
    offset: u32,
    expected: u32,
    observed: u32,
}

#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct Retired {
    spare: u32,
    descriptor: u32,
    crc: u32,
    segment: u32,
    /// Descriptors completed before this spare retired.
    descriptors_at_retire: u32,
}

#[derive(Zeroable)]
pub(super) struct State {
    counts: [Counts; SEGMENTS],
    /// Retirements waiting their two laps, oldest first.
    pending: [Retired; RX_REBIND_SPARES],
    pending_head: usize,
    pending_len: usize,
    /// Retirements kept to the run's end.
    kept: [Retired; RX_REBIND_SPARES],
    kept_len: usize,
    kept_cursor: usize,
    /// The unbound spares of the pool, a ring of spare numbers.
    free: [u8; RX_REBIND_SPARES],
    free_head: usize,
    free_len: usize,
    retired_total: u32,
    returned_total: u32,
    violations: [Violation; VIOLATION_CAPACITY],
    violations_len: usize,
    violations_total: u32,
    first_violation: u32,
    /// Per descriptor, the spare the probe bound it to.
    expected: [u32; COUNT],
    /// Per descriptor, the segment + 1 of a rebind whose new spare has
    /// received no checked frame yet; zero otherwise.
    fresh: [u32; COUNT],
    spare_cursor: usize,
    descriptors_total: u32,
    hw_last: [u16; 4],
    kept_final_checks: u32,
    kept_with_lap: u32,
    kept_at_end: u32,
    min_lap_descriptors: u32,
    unbound: u32,
    unbound_final_checks: u32,
    station: [u8; 6],
    sender: [u8; 6],
    sender_known: bool,
    /// The first QoS Data sender to this station (`tasks/193`), for whom
    /// the receive agreement is programmed once.
    agreement_peer: [u8; 6],
    agreement_peer_known: bool,
    agreement: bool,
    /// `tasks/194`: violations by kind (index = `Kind` code; 9 = a copy
    /// difference outside bytes 48 to 51).
    kinds: [u32; KINDS],
    /// Frames whose copy differed from the spare in bytes 48 to 51 alone.
    known_late: u32,
    /// Every differing byte of every frame, by offset in the buffer.
    copy_offsets: [u32; BUFFER],
    /// Frames whose header (bytes 0 to 55, the late word excepted) differed,
    /// and frames whose bytes from 56 on differed.
    header_differs: u32,
    frame_differs: u32,
    late_at_last: LateWord,
    late_not_last: LateWord,
    /// The same in rebind segments, measured after the rebind.
    late_at_last_rebind: LateWord,
    late_not_last_rebind: LateWord,
    /// Frames whose bytes differed in the tail (the last 8).
    tail_differs: u32,
    /// After a rebind at LAST, which of the buffer taken out and the spare
    /// put in had the late word change: neither, old only, new only, both.
    landing: [u32; 4],
    /// The first violation stops the run; its descriptor, when known.
    stop: bool,
    stop_descriptor: u32,
    /// The stopping descriptor's words when the stop was seen.
    stop_now: [u32; 3],
    acts: [ActSnapshot; COUNT],
    /// `tasks/200`: the exhaust phase and its records.
    exhaust: Exhaust,
    exhaust_events: [ExhaustEvent; SEGMENTS],
    /// `tasks/219` c1: the hole's ring index plus one, zero while no hole
    /// is kept.
    hole_plus_one: u32,
    /// `tasks/234`: when the taker began to stall behind the hole
    /// (microseconds plus one, zero while not stalled), and whether the
    /// 20 ms record of this stall was taken.
    stall_from_plus_one: u32,
    stall_traced: bool,
    /// Microseconds since the segments' clock start, set every pass.
    now_us: u32,
    /// The run's first data sequence number from the sender and its time,
    /// to align the capture.
    first_sn: u32,
    first_sn_us: u32,
}

/// The HAL's copy of one completed frame.
pub(super) type Frame = [u8; BUFFER];

static STATE: ZeroedStatic<State> = ZeroedStatic::new();
static FRAME: ZeroedStatic<Frame> = ZeroedStatic::new();

pub(super) fn take() -> Result<(&'static mut State, &'static mut Frame), Name<32>> {
    let state = STATE.try_take().ok_or(name!("state-claimed"; 32))?;
    let frame = FRAME.try_take().ok_or(name!("frame-claimed"; 32))?;
    for (descriptor, expected) in state.expected.iter_mut().enumerate() {
        *expected = descriptor as u32;
    }
    for spare in COUNT..POOL_END {
        state.free_push(spare)?;
    }
    for counts in &mut state.counts {
        counts.reserve_min = u32::MAX;
        counts.counter_first = u32::MAX;
    }
    state.late_at_last.min = u32::MAX;
    state.late_not_last.min = u32::MAX;
    state.late_at_last_rebind.min = u32::MAX;
    state.late_not_last_rebind.min = u32::MAX;
    state.stop_descriptor = u32::MAX;
    state.first_sn = u32::MAX;
    for event in &mut state.exhaust_events {
        event.next = u32::MAX;
        event.last = u32::MAX;
        event.tail = u32::MAX;
    }
    Ok((state, frame))
}

fn bump(counter: &mut u32) {
    *counter = counter.saturating_add(1);
}

fn word_at(bytes: &[u8], word: usize) -> Option<u32> {
    let start = word.checked_mul(4)?;
    let chunk: [u8; 4] = bytes.get(start..start.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(chunk))
}

/// CRC-32 over a whole spare, one volatile word copy at a time: no
/// buffer-sized value is made.
fn crc_spare(bank: &Spares, spare: usize) -> Option<u32> {
    let mut digest = CRC32.digest();
    for word in 0..WORDS {
        digest.update(&bank.read_word(spare, word)?.to_le_bytes());
    }
    Some(digest.finalize())
}

impl State {
    fn free_push(&mut self, spare: usize) -> Result<(), Name<32>> {
        let at = (self.free_head + self.free_len) % RX_REBIND_SPARES;
        let slot = self.free.get_mut(at).ok_or(name!("free"; 32))?;
        *slot = u8::try_from(spare).map_err(|_| name!("free-spare"; 32))?;
        self.free_len = self.free_len.saturating_add(1);
        Ok(())
    }

    fn free_pop(&mut self) -> Option<usize> {
        if self.free_len == 0 {
            return None;
        }
        let spare = usize::from(*self.free.get(self.free_head)?);
        self.free_head = (self.free_head + 1) % RX_REBIND_SPARES;
        self.free_len -= 1;
        Some(spare)
    }

    fn keep(&mut self, retired: Retired) -> Result<(), Name<32>> {
        let slot = self
            .kept
            .get_mut(self.kept_len)
            .ok_or(name!("kept-full"; 32))?;
        *slot = retired;
        self.kept_len = self.kept_len.saturating_add(1);
        Ok(())
    }

    /// Return the pending retirements held two laps: each CRC checked
    /// unchanged, then given back to the bank. A changed one is a violation
    /// and is kept out.
    fn service_pending(
        &mut self,
        segment: usize,
        storage: &'static Storage,
        bank: &'static Spares,
    ) -> Result<(), Name<32>> {
        while self.pending_len != 0 {
            let retired = *self
                .pending
                .get(self.pending_head)
                .ok_or(name!("pending"; 32))?;
            if self
                .descriptors_total
                .saturating_sub(retired.descriptors_at_retire)
                < RETURN_AFTER
            {
                break;
            }
            self.pending_head = (self.pending_head + 1) % RX_REBIND_SPARES;
            self.pending_len -= 1;
            let spare = usize::try_from(retired.spare).map_err(|_| name!("pending"; 32))?;
            bump(&mut self.counts(segment)?.retired_checks);
            let crc = crc_spare(bank, spare).ok_or(name!("pending-read"; 32))?;
            if crc != retired.crc {
                self.stop_descriptor = retired.descriptor;
                self.record(
                    segment,
                    Violation {
                        kind: Kind::Retired as u32,
                        segment: segment as u32,
                        spare: retired.spare,
                        offset: retired.segment,
                        expected: retired.crc,
                        observed: crc,
                    },
                );
                self.keep(retired)?;
                continue;
            }
            if storage.rebind_spare_return(spare).is_err() {
                self.record(
                    segment,
                    Violation {
                        kind: Kind::Accounting as u32,
                        segment: segment as u32,
                        spare: retired.spare,
                        offset: retired.descriptor,
                        expected: 0,
                        observed: 1,
                    },
                );
                self.keep(retired)?;
                continue;
            }
            self.free_push(spare)?;
            bump(&mut self.returned_total);
            bump(&mut self.counts(segment)?.returned);
        }
        Ok(())
    }

    /// The sender's frame, checked by content: the counter in every 64
    /// octets one frame's, every other byte the sender's.
    fn check_content(&mut self, segment: usize, spare: usize, mpdu: &[u8]) -> Result<(), Name<32>> {
        let (Some(&[kind, _]), Some(to), Some(from)) = (
            mpdu.get(0..2)
                .map(|b| <&[u8; 2]>::try_from(b).ok())
                .flatten(),
            mpdu.get(4..10),
            mpdu.get(10..16),
        ) else {
            return Ok(());
        };
        if kind == 0x88
            && to == self.station
            && !self.agreement_peer_known
            && let Some(from) = mpdu
                .get(10..16)
                .and_then(|from| <[u8; 6]>::try_from(from).ok())
        {
            self.agreement_peer = from;
            self.agreement_peer_known = true;
        }
        if kind != 0x08 || to != self.station {
            return Ok(());
        }
        let Ok(from) = <[u8; 6]>::try_from(from) else {
            return Ok(());
        };
        if !self.sender_known {
            self.sender = from;
            self.sender_known = true;
        }
        if from != self.sender {
            return Ok(());
        }
        let counter = mpdu
            .get(Case::Provenance700.counter_at()..Case::Provenance700.counter_at() + 4)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_le_bytes);
        let first = match counter.filter(|c| Case::from_counter(*c) == Some(Case::Provenance700)) {
            None => Some(Case::Provenance700.counter_at()),
            Some(counter) => mpdu.iter().enumerate().find_map(|(offset, got)| {
                let want = byte(
                    Case::Provenance700,
                    self.sender,
                    self.station,
                    counter,
                    offset,
                );
                let same = match offset {
                    1 => got & !RETRY == want,
                    2..4 | 22..24 => true,
                    _ => *got == want,
                };
                (!same).then_some(offset)
            }),
        };
        match first {
            None => {
                let round = counter.map_or(0, Case::round);
                let counts = self.counts(segment)?;
                bump(&mut counts.content_ok);
                counts.counter_first = counts.counter_first.min(round);
                counts.counter_last = counts.counter_last.max(round);
            }
            Some(offset) => {
                bump(&mut self.counts(segment)?.content_bad);
                self.record(
                    segment,
                    Violation {
                        kind: Kind::Content as u32,
                        segment: segment as u32,
                        spare: spare as u32,
                        offset: offset as u32,
                        expected: counter.unwrap_or(0),
                        observed: u32::from(mpdu.get(offset).copied().unwrap_or(0)),
                    },
                );
            }
        }
        Ok(())
    }

    fn record(&mut self, segment: usize, violation: Violation) {
        if let Some(counts) = self.counts.get_mut(segment) {
            bump(&mut counts.violations);
        }
        if let Some(kind) = self.kinds.get_mut(violation.kind as usize) {
            bump(kind);
        }
        // `tasks/194`: the first violation that is not the known late word
        // stops the run (the known one is never recorded).
        self.stop = true;
        if self.violations_total == 0 {
            self.first_violation = violation.kind;
        }
        bump(&mut self.violations_total);
        if let Some(slot) = self.violations.get_mut(self.violations_len) {
            *slot = violation;
            self.violations_len = self.violations_len.saturating_add(1);
        }
    }

    fn counts(&mut self, segment: usize) -> Result<&mut Counts, Name<32>> {
        self.counts.get_mut(segment).ok_or(name!("segment"; 32))
    }

    /// Check one committed single-descriptor frame, copied by the HAL from
    /// the spare its accounting names, before it is detached or recycled.
    #[allow(clippy::too_many_arguments)]
    fn check_frame(
        &mut self,
        segment: usize,
        storage: &'static Storage,
        bank: &'static Spares,
        frame: &Frame,
        index: usize,
        length: usize,
        word0_at_take: oer_chips_esp32s31_hal::layouts::WifiRxDmaDescriptorControl,
    ) -> Result<(), Name<32>> {
        let descriptor = storage
            .descriptors()
            .get(index)
            .ok_or(name!("frame-descriptor"; 32))?;
        let spare = usize::try_from(*self.expected.get(index).ok_or(name!("expected"; 32))?)
            .map_err(|_| name!("expected"; 32))?;
        let address = bank.address(spare).ok_or(name!("spare-address"; 32))?;
        let published = storage.rebind_published(index);
        let fresh = *self.fresh.get(index).ok_or(name!("fresh"; 32))?;
        bump(&mut self.counts(segment)?.frames);
        if fresh != 0 {
            bump(&mut self.counts(segment)?.frames_on_rebound);
        }
        let mut clean = true;
        if descriptor.buffer_address() != address || published != Some(RxBinding::Spare(spare)) {
            clean = false;
            self.record(
                segment,
                Violation {
                    kind: Kind::Binding as u32,
                    segment: segment as u32,
                    spare: spare as u32,
                    offset: 0,
                    expected: address,
                    observed: descriptor.buffer_address(),
                },
            );
        }
        let frame = frame.get(..length).ok_or(name!("frame-length"; 32))?;
        let compared = length.min(COPY_BYTES);
        let spare_byte = |at: usize| {
            bank.read_word(spare, at / 4)
                .and_then(|word| word.to_le_bytes().get(at % 4).copied())
        };
        // `tasks/196`: every byte of the unit, after its header settled. A
        // difference in the header (below 56) or the 8-byte tail is counted
        // by offset and never stops the run; one in the frame's bytes is a
        // violation and stops it.
        let tail = length.saturating_sub(TAIL_BYTES);
        let mut header_differs = false;
        let mut tail_differs = false;
        let mut first_frame = None;
        for offset in 0..length {
            if frame.get(offset).copied() == spare_byte(offset) {
                continue;
            }
            if let Some(count) = self.copy_offsets.get_mut(offset) {
                bump(count);
            }
            if offset < HEADER_BYTES {
                header_differs = true;
            } else if offset >= tail {
                tail_differs = true;
            } else {
                first_frame.get_or_insert(offset);
            }
        }
        if header_differs {
            bump(&mut self.header_differs);
        }
        if tail_differs {
            bump(&mut self.tail_differs);
        }
        if let Some(offset) = first_frame {
            bump(&mut self.frame_differs);
            clean = false;
            self.stop_descriptor = index as u32;
            self.record(
                segment,
                Violation {
                    kind: 9,
                    segment: segment as u32,
                    spare: spare as u32,
                    offset: offset as u32,
                    expected: u32::from(frame.get(offset).copied().unwrap_or(0)),
                    observed: u32::from(spare_byte(offset).unwrap_or(0)),
                },
            );
        } else if header_differs || tail_differs {
            bump(&mut self.known_late);
        }
        let canary_words = (0..compared / 4)
            .filter(|&word| word_at(frame, word) == Some(canary(spare, word)))
            .count();
        if canary_words != 0 {
            clean = false;
            self.record(
                segment,
                Violation {
                    kind: Kind::Landing as u32,
                    segment: segment as u32,
                    spare: spare as u32,
                    offset: 0,
                    expected: 0,
                    observed: canary_words as u32,
                },
            );
        }
        let view = RxSegment {
            descriptor_address: 0,
            // `tasks/199`: the control word as completed, before the detach.
            descriptor_word0: word0_at_take,
            buffer: frame,
            next_descriptor_address: 0,
        };
        let layout = first_segment_layout(
            &view,
            RxIngressConfig {
                ring_entry_limit: COUNT,
                csi_config: 0,
                flags: 0,
            },
        );
        let Ok(layout) = layout else {
            bump(&mut self.counts(segment)?.prefix_err);
            return Ok(());
        };
        bump(&mut self.counts(segment)?.prefix_ok);
        let body_end = layout
            .frame_offset
            .checked_add(layout.expected_frame_length)
            .ok_or(name!("fcs-range"; 32))?;
        let fcs_end = body_end
            .checked_add(FCS_SIZE)
            .ok_or(name!("fcs-range"; 32))?;
        let counts = self.counts(segment)?;
        match (
            frame.get(layout.frame_offset..body_end),
            frame.get(body_end..fcs_end),
        ) {
            (Some(body), Some(fcs)) => {
                let stored =
                    u32::from_le_bytes(fcs.try_into().map_err(|_| name!("fcs-width"; 32))?);
                if crc32(body) == stored {
                    bump(&mut counts.fcs_ok);
                } else {
                    bump(&mut counts.fcs_bad);
                }
            }
            _ => bump(&mut counts.fcs_absent),
        }
        if let Some(mpdu) = frame.get(layout.frame_offset..body_end) {
            self.record_sequence(segment, mpdu);
            self.check_content(segment, spare, mpdu)?;
        }
        // The rebind is covered: the BUFFER word named the new spare, DMA
        // landed this frame in it, and every check ran on it.
        if fresh != 0 && clean {
            let rebind_segment = usize::try_from(fresh - 1).map_err(|_| name!("fresh"; 32))?;
            bump(&mut self.counts(rebind_segment)?.covered_rebinds);
            *self.fresh.get_mut(index).ok_or(name!("fresh"; 32))? = 0;
        }
        Ok(())
    }

    /// `tasks/200`: a data frame's sequence number (IEEE 802.11-2020
    /// 9.2.4.4), kept for the capture: the run's first, and an exhaust
    /// segment's before the ring ran out and after it was restored.
    fn record_sequence(&mut self, segment: usize, mpdu: &[u8]) {
        let (Some(&control), Some(sequence)) = (mpdu.first(), mpdu.get(22..24)) else {
            return;
        };
        // Frame Control: type Data (9.2.4.1.3).
        if control & 0x0c != 0x08 {
            return;
        }
        let sequence = u16::from_le_bytes([
            sequence.first().copied().unwrap_or(0),
            sequence.get(1).copied().unwrap_or(0),
        ]) >> 4;
        if self.first_sn == u32::MAX {
            self.first_sn = u32::from(sequence);
            self.first_sn_us = self.now_us;
        }
        let phase = self.exhaust;
        let now = self.now_us;
        let Some(event) = self.exhaust_events.get_mut(segment) else {
            return;
        };
        match phase {
            Exhaust::Holding | Exhaust::Out => {
                let at = event.before_len as usize % EXHAUST_SNS;
                if let Some(slot) = event.before.get_mut(at) {
                    *slot = sequence;
                }
                event.before_len = event.before_len.saturating_add(1);
            }
            Exhaust::Restored => {
                if event.after_len == 0 {
                    event.after_first_us = now;
                }
                if let Some(slot) = event.after.get_mut(event.after_len as usize) {
                    *slot = sequence;
                    event.after_len += 1;
                }
            }
            Exhaust::Idle => {}
        }
    }

    /// Recheck `limit` kept spares round-robin.
    fn check_retired(
        &mut self,
        segment: usize,
        bank: &'static Spares,
        limit: usize,
    ) -> Result<(), Name<32>> {
        for _ in 0..limit.min(self.kept_len) {
            let position = self.kept_cursor % self.kept_len.max(1);
            self.kept_cursor = position.saturating_add(1);
            let retired = *self.kept.get(position).ok_or(name!("kept"; 32))?;
            let spare = usize::try_from(retired.spare).map_err(|_| name!("kept"; 32))?;
            bump(&mut self.counts(segment)?.retired_checks);
            let crc = crc_spare(bank, spare).ok_or(name!("kept-read"; 32))?;
            if crc != retired.crc {
                self.record(
                    segment,
                    Violation {
                        kind: Kind::Retired as u32,
                        segment: segment as u32,
                        spare: retired.spare,
                        offset: retired.segment,
                        expected: retired.crc,
                        observed: crc,
                    },
                );
            }
        }
        Ok(())
    }

    /// Check never bound spare `spare`'s canary; `None` when it was bound.
    fn check_spare(
        &mut self,
        segment: usize,
        storage: &'static Storage,
        bank: &'static Spares,
        spare: usize,
    ) -> Result<Option<()>, Name<32>> {
        // A returned spare is unbound again but holds its frames: only the
        // reserved spares, never bound, keep their canary.
        if spare < POOL_END || !storage.rebind_spare_unbound(spare) {
            return Ok(None);
        }
        if let Some(word) =
            (0..WORDS).find(|&word| bank.read_word(spare, word) != Some(canary(spare, word)))
        {
            self.record(
                segment,
                Violation {
                    kind: Kind::Spare as u32,
                    segment: segment as u32,
                    spare: spare as u32,
                    offset: word as u32,
                    expected: canary(spare, word),
                    observed: bank.read_word(spare, word).unwrap_or(0),
                },
            );
        }
        Ok(Some(()))
    }

    fn check_spares(
        &mut self,
        segment: usize,
        storage: &'static Storage,
        bank: &'static Spares,
        limit: usize,
    ) -> Result<(), Name<32>> {
        for _ in 0..limit {
            let spare = self.spare_cursor % RX_REBIND_SPARES;
            self.spare_cursor = spare.saturating_add(1);
            if self.check_spare(segment, storage, bank, spare)?.is_some() {
                bump(&mut self.counts(segment)?.spare_checks);
            }
        }
        Ok(())
    }

    fn hardware_now(access: &RadioAccess) -> [u16; 4] {
        let stats = access.receive_statistics_snapshot().primary;
        [
            stats.mpdu_count,
            stats.fcs_error,
            stats.buffer_full,
            stats.data_success,
        ]
    }

    fn sample_hardware(&mut self, segment: usize, access: &RadioAccess) -> Result<(), Name<32>> {
        let stats = access.receive_statistics_snapshot().primary;
        let now = [
            stats.mpdu_count,
            stats.fcs_error,
            stats.buffer_full,
            stats.data_success,
        ];
        let delta: [u32; 4] = core::array::from_fn(|i| {
            u32::from(
                now.get(i)
                    .copied()
                    .unwrap_or(0)
                    .wrapping_sub(self.hw_last.get(i).copied().unwrap_or(0)),
            )
        });
        self.hw_last = now;
        let counts = self.counts(segment)?;
        let [mpdu, fcs, full, success] = delta;
        counts.hw_mpdu = counts.hw_mpdu.saturating_add(mpdu);
        counts.hw_fcs_error = counts.hw_fcs_error.saturating_add(fcs);
        counts.hw_buffer_full = counts.hw_buffer_full.saturating_add(full);
        counts.hw_data_success = counts.hw_data_success.saturating_add(success);
        Ok(())
    }
}

fn reload(ring: &mut RxRingLive<'_, COUNT>, access: &mut RadioAccess) -> Result<(), Name<32>> {
    ring.complete_pending_reload(
        access.rx_dma(),
        Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
    )
    .map_err(|_| name!("reload"; 32))
}

/// `tasks/234`: a stall of this long behind the hole is recorded once.
const STALL_TRACE_US: u32 = 20_000;

/// Serve one scan: up to `UNITS_PER_SCAN` units, each taken only outside
/// the segment's hold, checked and recycled through the production append; a
/// rebind segment rebinds every one, a fixed segment detaches one when
/// `action` is due.
#[allow(clippy::too_many_arguments)]
fn scan(
    state: &mut State,
    frame: &mut Frame,
    segment: usize,
    action: &mut bool,
    storage: &'static Storage,
    bank: &'static Spares,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    irq: &mut MacInterruptRegisters,
) -> Result<(), Name<32>> {
    let arm = *ARMS.get(segment).ok_or(name!("segment"; 32))?;
    bump(&mut state.counts(segment)?.services);
    // `tasks/229`: the ring at an exit segment's start.
    if arm.exit() && state.counts(segment)?.services == 1 {
        // The first exit segment's first exits, about ten of six records.
        if segment == 1 {
            oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_rebind::trace_arm(
                oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_rebind::TRACE_RECORDS as u32,
            );
        }
        ring.experiment_trace(1, access.rx_dma());
    }
    if irq.mac_interrupt_raw().fatal_assertion_raw().is_some() {
        return Err(name!("fatal-raw"; 32));
    }
    let status = irq.mac_interrupt_status();
    irq.acknowledge_mac_interrupts(status.retain_rx_causes(MacRxInterrupts::all()));
    storage
        .observe_reserved(ring, access.rx_dma())
        .map_err(|_| name!("reserved"; 32))?;
    reload(ring, access)?;
    // `tasks/200`: while an exhaust segment holds, nothing is re-armed.
    let holding = arm.exhaust() && matches!(state.exhaust, Exhaust::Holding | Exhaust::Out);
    for _ in 0..UNITS_PER_SCAN {
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| name!("cursor"; 32))?;
        // A unit whose append found the doorbell re-rung stays observed at
        // the recycle frontier; it is appended before any later take.
        if !holding
            && !arm.exit()
            && let Some(credit) = storage
                .claim_deferred_recycle(ring, frozen)
                .map_err(|_| name!("deferred-claim"; 32))?
        {
            // A hole an exit segment left is armed here.
            state.hole_plus_one = 0;
            match credit.append(access.rx_dma()) {
                Ok(_) | Err(RxRingError::ReloadPending) => {}
                Err(_) => return Err(name!("deferred-append"; 32)),
            }
            reload(ring, access)?;
            continue;
        }
        let frontier = storage
            .first_completed_unit_frontier_through_cursor(ring, frozen)
            .map_err(|_| name!("frontier"; 32))?;
        if frontier.unit_count == 0 {
            if holding && state.exhaust == Exhaust::Holding {
                let held = state
                    .exhaust_events
                    .get(segment)
                    .map_or(0, |event| event.held);
                if held as usize + 2 >= COUNT {
                    let base = ring.descriptor_base();
                    let index = |low| {
                        cursor_ring_index(low, base, COUNT).map_or(u32::MAX, |index| index as u32)
                    };
                    let (next, last) = (
                        index(frozen.next_descriptor_low()),
                        index(frozen.last_descriptor_low()),
                    );
                    let hw = State::hardware_now(access);
                    let now = state.now_us;
                    let tail = ring.topology_snapshot().tail_index;
                    let tail_words = words_of(storage, tail);
                    let next_words = words_of(storage, next as usize);
                    if let Some(event) = state.exhaust_events.get_mut(segment) {
                        event.out_us = now;
                        event.next = next;
                        event.last = last;
                        event.hw_at_out = hw;
                        event.tail = tail as u32;
                        event.tail_words = tail_words;
                        event.next_words = next_words;
                    }
                    state.exhaust = Exhaust::Out;
                }
            }
            break;
        }
        let head = storage
            .completion_frontier_head(ring)
            .map_err(|_| name!("head"; 32))?
            .ok_or(name!("head-none"; 32))?;
        // The hold: at least `arm.hold` completed descriptors between the
        // head and the frozen LAST, and NEXT not naming the head.
        let base = ring.descriptor_base();
        let last = cursor_ring_index(frozen.last_descriptor_low(), base, COUNT);
        let next = cursor_ring_index(frozen.next_descriptor_low(), base, COUNT);
        let behind = last.map(|last| (last + COUNT - head) % COUNT);
        if arm.exit() && state.hole_plus_one != 0 {
            // `tasks/219` c1: with the hole kept, units stay in the ring
            // until the walker stands: NEXT names nothing or the hole.
            let hole = state.hole_plus_one as usize - 1;
            // `tasks/234`: NEXT on the head is judged as any other NEXT, by
            // the cursor read twice: a walker that stood on the oldest
            // completed unit is the stall's state too.
            let stands = match next {
                None => Some(0),
                Some(at) if at == hole => Some(1),
                Some(_) => {
                    // NEXT on another descriptor: stands only if nothing
                    // armed lies ahead of it. `tasks/230`: the cursor, the
                    // descriptor's words, the cursor again; a frame that
                    // landed between the two reads moved the cursor, and the
                    // walker did not stand.
                    let tail = ring.topology_snapshot().tail_index;
                    let named = words_of(storage, next.unwrap_or(0));
                    let again = access.receive_dma_snapshot();
                    let still = again.next_descriptor_low == frozen.next_descriptor_low()
                        && again.last_descriptor_low == frozen.last_descriptor_low();
                    let armed = named[0] & (1 << 31) != 0 && named[0] & (1 << 30) == 0;
                    if !still {
                        bump(&mut state.counts(segment)?.cursor_moved);
                    }
                    (still && !armed && tail != next.unwrap_or(usize::MAX)).then_some(2)
                }
            };
            let Some(kind) = stands else {
                bump(&mut state.counts(segment)?.held_back);
                // The stall's clock, and one record of the ring after 20 ms
                // of it.
                if state.stall_from_plus_one == 0 {
                    state.stall_from_plus_one = state.now_us.saturating_add(1);
                } else if !state.stall_traced
                    && state.now_us.saturating_sub(state.stall_from_plus_one) >= STALL_TRACE_US
                {
                    state.stall_traced = true;
                    ring.experiment_trace(8, access.rx_dma());
                }
                break;
            };
            state.stall_from_plus_one = 0;
            // c2: what the ring holds the first time the walker stood.
            let tail = ring.topology_snapshot().tail_index;
            let tail_words = words_of(storage, tail);
            let next_words = next.map_or([0; 3], |at| words_of(storage, at));
            if let Some(event) = state.exhaust_events.get_mut(segment) {
                if event.exits == 0 {
                    event.next = next.map_or(u32::MAX, |at| at as u32);
                    event.last = last.map_or(u32::MAX, |at| at as u32);
                    event.tail = tail as u32;
                    event.tail_words = tail_words;
                    event.next_words = next_words;
                    event.hole = hole as u32;
                    event.hole_words = words_of(storage, hole);
                    event.out_us = state.now_us;
                }
                bump(&mut event.exits);
                match kind {
                    0 => bump(&mut event.exit_next_zero),
                    1 => bump(&mut event.exit_next_hole),
                    _ => bump(&mut event.exit_next_other),
                }
            }
            // `tasks/229`: the ring where the walker stands.
            ring.experiment_trace(4, access.rx_dma());
            // The exit: the hole, bound to its spare at its take, armed and
            // linked behind the tail with the walker standing (reserve 0).
            let credit = storage
                .claim_deferred_recycle(ring, frozen)
                .map_err(|_| name!("exit-claim"; 32))?
                .ok_or(name!("exit-no-hole"; 32))?;
            match credit.append(access.rx_dma()) {
                Ok(_) | Err(RxRingError::ReloadPending) => {}
                Err(_) => return Err(name!("exit-append"; 32)),
            }
            reload(ring, access)?;
            ring.experiment_trace(5, access.rx_dma());
            state.hole_plus_one = 0;
            // `tasks/221`: the cursor and the bound read anew; the oldest
            // unit is the next pass's take, the next hole.
            continue;
        } else if next == Some(head) || behind.is_none_or(|behind| behind < arm.hold) {
            bump(&mut state.counts(segment)?.held_back);
            break;
        }
        let length = usize::from(
            storage
                .descriptors()
                .get(head)
                .ok_or(name!("head-descriptor"; 32))?
                .word0()
                .length(),
        );
        // `tasks/194`: the late word of the head's buffer at the take.
        let taken_spare = usize::try_from(*state.expected.get(head).ok_or(name!("expected"; 32))?)
            .map_err(|_| name!("expected"; 32))?;
        let word0_at_take = storage
            .descriptors()
            .get(head)
            .ok_or(name!("head-descriptor"; 32))?
            .word0();
        let take_cycle = oer_portable_riscv::cycle32();
        let word_at_take = [
            bank.read_word(taken_spare, LATE_WORD),
            bank.read_word(taken_spare, LATE_WORD + 1),
        ];
        let window = if behind == Some(0) {
            LATE_WINDOW_CYCLES
        } else {
            NOT_LAST_WINDOW_CYCLES
        };
        let written_at_take = word_at_take
            .iter()
            .any(|word| word.is_some_and(|word| word != 0));
        // A fixed segment waits for the late words before the copy, so the
        // delay is measured from the take (`tasks/196`); a rebind segment
        // rebinds first and waits after, to see where the write lands.
        let settled_before_copy = if arm.rebind() {
            None
        } else {
            Some(settle_late_word(
                bank,
                taken_spare,
                word_at_take,
                take_cycle,
                window,
            ))
        };
        let copied = frontier.descriptor_count == 1
            && length != 0
            && length <= BUFFER
            && storage
                .copy_first_completed_unit_bytes_through_cursor(
                    ring,
                    frozen,
                    0,
                    frame.get_mut(..length).ok_or(name!("copy-range"; 32))?,
                )
                .map_err(|_| name!("copy"; 32))?;
        // The armed reserve a rebind would be linked behind: the descriptors
        // from NEXT through the published tail.
        let reserve = match next {
            Some(next) => (ring.topology_snapshot().tail_index + COUNT - next) % COUNT + 1,
            None => 0,
        };
        // `tasks/193`: a unit at the frozen LAST is rebound too.
        let at_last = behind == Some(0);
        let mut unit = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| name!("take"; 32))?
            .ok_or(name!("take-none"; 32))?;
        if unit.head_index() != head {
            unit.retain_for_deferred_recycle();
            return Err(name!("take-head"; 32));
        }
        let descriptors = u32::try_from(unit.descriptor_count()).unwrap_or(u32::MAX);
        state.descriptors_total = state.descriptors_total.saturating_add(descriptors);
        {
            let counts = state.counts(segment)?;
            bump(&mut counts.units);
            counts.descriptors = counts.descriptors.saturating_add(descriptors);
            if unit.descriptor_count() != 1 {
                bump(&mut counts.chained);
            }
        }
        // `tasks/194`: a rebind (or a fixed segment's detach) is made at the
        // take, before the late word; the header then settles, the copy is
        // checked against the buffer taken out, and the act is finished.
        let mut bound = None;
        if copied && (arm.rebind() || (*action && !arm.exhaust())) {
            if reserve == 0 && !arm.exit() {
                // No armed descriptor ahead of the walker: excluded.
                bump(&mut state.counts(segment)?.exhausted);
            } else {
                match act_bind(
                    state,
                    segment,
                    arm.rebind(),
                    bank,
                    storage,
                    &mut unit,
                    frozen,
                    access,
                ) {
                    Ok(made) => bound = Some(made),
                    Err(error) => {
                        unit.retain_for_deferred_recycle();
                        return Err(error);
                    }
                }
            }
            *action = false;
        }
        if copied {
            let changed = match settled_before_copy {
                Some(changed) => changed,
                None => settle_late_word(bank, taken_spare, word_at_take, take_cycle, window),
            };
            let late = match (at_last, arm.rebind()) {
                (true, false) => &mut state.late_at_last,
                (false, false) => &mut state.late_not_last,
                (true, true) => &mut state.late_at_last_rebind,
                (false, true) => &mut state.late_not_last_rebind,
            };
            late.record(changed);
            if written_at_take {
                bump(&mut late.written_at_take);
            }
            if let Some(bound) = bound
                && let Some((spare, word_at_bind)) = bound.new
                && at_last
            {
                let old_changed = changed.is_some();
                let new_changed = (
                    bank.read_word(spare, LATE_WORD),
                    bank.read_word(spare, LATE_WORD + 1),
                ) != word_at_bind;
                let outcome = usize::from(old_changed) | usize::from(new_changed) << 1;
                if let Some(count) = state.landing.get_mut(outcome) {
                    bump(count);
                }
            }
            if let Err(error) =
                state.check_frame(segment, storage, bank, frame, head, length, word0_at_take)
            {
                unit.retain_for_deferred_recycle();
                return Err(error);
            }
        }
        if let Some(bound) = bound
            && let Err(error) = act_finish(state, segment, bank, bound, reserve, at_last)
        {
            unit.retain_for_deferred_recycle();
            return Err(error);
        }
        if !state.agreement && state.agreement_peer_known {
            let peer = state.agreement_peer;
            use oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::hardware::{
                S31RxBlockAckAgreement, program,
            };
            let _programmed = program(
                &mut access.wifi_mac_hal(),
                S31RxBlockAckAgreement {
                    hardware_index: 0,
                    interface: oer_chips_esp32s31_hal::types::MacInterface::Station,
                    peer,
                    tid: 0,
                    starting_sequence: oer_protocols_ieee80211_wire::sequence::SequenceNumber::ZERO,
                    window: 64,
                },
            );
            state.agreement = true;
        }
        if arm.exit() {
            // `tasks/219` c1: the taken unit's descriptor, detached and
            // bound to a spare, is the ring's one hole until the next exit.
            state.hole_plus_one = head as u32 + 1;
            unit.retain_for_deferred_recycle();
            continue;
        }
        if holding {
            // Kept: the unit stays observed at the recycle frontier and is
            // appended by the deferred claims after the restore.
            unit.retain_for_deferred_recycle();
            if let Some(event) = state.exhaust_events.get_mut(segment) {
                event.held = event.held.saturating_add(1);
            }
            continue;
        }
        let Some(credit) = unit.retain_discard() else {
            return Err(name!("recycle-frontier"; 32));
        };
        match credit.append(access.rx_dma()) {
            Ok(_) => {}
            Err(RxRingError::ReloadPending) => {
                // The settle rang the doorbell again; the unit stays observed
                // and is appended by the next deferred claim.
                bump(&mut state.counts(segment)?.reload_deferred);
            }
            Err(_) => return Err(name!("append"; 32)),
        }
        reload(ring, access)?;
    }
    state.service_pending(segment, storage, bank)?;
    state.check_retired(segment, bank, RETIRED_CHECKS_PER_SCAN)?;
    state.check_spares(segment, storage, bank, SPARE_CHECKS_PER_SCAN)?;
    if state.stop && state.stop_now == [0; 3] {
        state.stop_now = words_of(storage, state.stop_descriptor as usize);
    }
    state.sample_hardware(segment, access)
}

/// What [`act_bind`] did: the descriptor, its old spare and, for a
/// rebind, the new spare with its late word at the bind.
#[derive(Clone, Copy)]
struct Bound {
    index: usize,
    old: u32,
    rebind: bool,
    new: Option<(usize, (Option<u32>, Option<u32>))>,
}

impl LateWord {
    /// One take's late word: changed after `Some(cycles)`, or not within the
    /// window.
    fn record(&mut self, changed: Option<u32>) {
        bump(&mut self.takes);
        match changed {
            Some(cycles) => {
                bump(&mut self.changed);
                self.min = self.min.min(cycles);
                self.max = self.max.max(cycles);
                let bucket = ((cycles / CYCLES_PER_BUCKET) as usize).min(DELAY_BUCKETS - 1);
                if let Some(count) = self.delay.get_mut(bucket) {
                    bump(count);
                }
            }
            None => bump(&mut self.never),
        }
    }
}

/// Re-read spare `spare`'s late word until it differs from `at_take` or
/// the window passed: the cycles from the take to the change.
fn settle_late_word(
    bank: &'static Spares,
    spare: usize,
    at_take: [Option<u32>; 2],
    take_cycle: u32,
    window: u32,
) -> Option<u32> {
    loop {
        let now = oer_portable_riscv::cycle32();
        let word = [
            bank.read_word(spare, LATE_WORD),
            bank.read_word(spare, LATE_WORD + 1),
        ];
        let elapsed = now.wrapping_sub(take_cycle);
        if word != at_take {
            return Some(elapsed);
        }
        if elapsed >= window {
            return None;
        }
    }
}

fn words_of(storage: &'static Storage, index: usize) -> [u32; 3] {
    storage
        .descriptors()
        .get(index)
        .map_or([0; 3], |descriptor| {
            [
                descriptor.word0().bits(),
                descriptor.buffer_address(),
                descriptor.next_address(),
            ]
        })
}

/// Detach the taken unit in place at the take and, in a rebind segment,
/// bind its next re-arm to an unbound spare of the pool, recording the
/// descriptor's words before and after with the cursor's NEXT and LAST.
#[allow(clippy::too_many_arguments)]
fn act_bind(
    state: &mut State,
    segment: usize,
    rebind: bool,
    bank: &'static Spares,
    storage: &'static Storage,
    unit: &mut RxDmaCompletedUnit<'static, '_, '_, COUNT, BUFFER, BUFFER>,
    frozen: RxFrozenCursor,
    access: &mut RadioAccess,
) -> Result<Bound, Name<32>> {
    let index = unit.head_index();
    let before = words_of(storage, index);
    unit.detach_before_last(access.rx_dma(), frozen)
        .map_err(|_| name!("detach"; 32))?;
    let old = *state.expected.get(index).ok_or(name!("expected"; 32))?;
    bump(&mut state.counts(segment)?.detached);
    let mut new = None;
    if rebind {
        match state.free_pop() {
            Some(spare) => {
                unit.rebind_next(spare).map_err(|_| name!("rebind"; 32))?;
                new = Some((
                    spare,
                    (
                        bank.read_word(spare, LATE_WORD),
                        bank.read_word(spare, LATE_WORD + 1),
                    ),
                ));
            }
            None => bump(&mut state.counts(segment)?.spares_exhausted),
        }
    }
    if let Some(snapshot) = state.acts.get_mut(index) {
        *snapshot = ActSnapshot {
            before,
            after: words_of(storage, index),
            next_low: frozen.next_descriptor_low(),
            last_low: frozen.last_descriptor_low(),
            valid: 1,
        };
    }
    Ok(Bound {
        index,
        old,
        rebind,
        new,
    })
}

/// Finish an act after the header settled: a fixed segment checks the
/// detached buffer over a short window; a rebind retires the old spare
/// with its CRC taken now and records the armed reserve.
fn act_finish(
    state: &mut State,
    segment: usize,
    bank: &'static Spares,
    bound: Bound,
    reserve: usize,
    at_last: bool,
) -> Result<(), Name<32>> {
    let Bound {
        index,
        old,
        rebind,
        new,
    } = bound;
    let old_spare = usize::try_from(old).map_err(|_| name!("expected"; 32))?;
    let crc = crc_spare(bank, old_spare).ok_or(name!("old-read"; 32))?;
    if !rebind {
        bump(&mut state.counts(segment)?.window_checks);
        let again = crc_spare(bank, old_spare).ok_or(name!("old-read"; 32))?;
        if again != crc {
            state.stop_descriptor = index as u32;
            state.record(
                segment,
                Violation {
                    kind: Kind::Window as u32,
                    segment: segment as u32,
                    spare: old,
                    offset: 0,
                    expected: crc,
                    observed: again,
                },
            );
        }
        return Ok(());
    }
    let Some((spare, _)) = new else {
        return Ok(());
    };
    let retired = Retired {
        spare: old,
        descriptor: index as u32,
        crc,
        segment: segment as u32,
        descriptors_at_retire: state.descriptors_total,
    };
    bump(&mut state.retired_total);
    if state.retired_total % KEEP_EVERY == 0 && state.kept_len < KEPT_MAX {
        state.keep(retired)?;
    } else {
        let at = (state.pending_head + state.pending_len) % RX_REBIND_SPARES;
        *state.pending.get_mut(at).ok_or(name!("pending-full"; 32))? = retired;
        state.pending_len = state.pending_len.saturating_add(1);
    }
    *state.expected.get_mut(index).ok_or(name!("expected"; 32))? = spare as u32;
    *state.fresh.get_mut(index).ok_or(name!("fresh"; 32))? = segment as u32 + 1;
    let counts = state.counts(segment)?;
    bump(&mut counts.rebinds);
    if at_last {
        bump(&mut counts.rebinds_at_last);
    }
    if let Some(bucket) = counts.reserve.get_mut(reserve.min(RESERVE_BUCKETS - 1)) {
        bump(bucket);
    }
    counts.reserve_min = counts.reserve_min.min(reserve as u32);
    Ok(())
}

/// Return to the executor once, so its other tasks (the watchdog feed, the
/// console) run between scans.
async fn yield_once() {
    let mut yielded = false;
    core::future::poll_fn(|context| {
        if yielded {
            core::task::Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        }
    })
    .await;
}

/// Run the segments on the started ring.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    state: &mut State,
    frame: &mut Frame,
    storage: &'static Storage,
    bank: &'static Spares,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    irq: &mut MacInterruptRegisters,
    station: [u8; 6],
) -> Result<(), Name<32>> {
    state.station = station;
    irq.publish_mac_interrupt_mask(MacInterruptMask::STATION);
    access.wifi_mac_hal().start_mac_runtime(ring);
    let start = EmbassyClock.now();
    state.hw_last = {
        let stats = access.receive_statistics_snapshot().primary;
        [
            stats.mpdu_count,
            stats.fcs_error,
            stats.buffer_full,
            stats.data_success,
        ]
    };
    let mut next_action: Instant = start;
    // The segments' clock starts at the sender's first frame: it boots after
    // this probe, and a segment without its frames proves nothing.
    let mut start = start;
    let first_deadline = start
        .checked_add(Duration::from_secs(FIRST_UNIT_S))
        .ok_or(name!("first-deadline"; 32))?;
    loop {
        let now = EmbassyClock.now();
        // `tasks/196`: the sender's first frame of any kind, QoS included.
        if !state.sender_known && !state.agreement_peer_known {
            if now >= first_deadline {
                return Err(name!("no-sender"; 32));
            }
            start = now;
        }
        let elapsed = now
            .checked_duration_since(start)
            .ok_or(name!("clock"; 32))?
            .as_micros()
            / 1000;
        let segment = usize::try_from(elapsed / SEGMENT_MS).map_err(|_| name!("segment"; 32))?;
        if segment >= SEGMENTS {
            break;
        }
        if state.stop {
            break;
        }
        state.now_us = u32::try_from(
            now.checked_duration_since(start)
                .ok_or(name!("clock"; 32))?
                .as_micros(),
        )
        .unwrap_or(u32::MAX);
        exhaust_phase(state, segment, access)?;
        let due = now >= next_action;
        let mut action = due;
        scan(
            state,
            frame,
            segment,
            &mut action,
            storage,
            bank,
            ring,
            access,
            irq,
        )?;
        // A fixed segment's action that found no eligible unit stays due.
        if due && !action {
            next_action = now
                .checked_add(Duration::from_millis(ACTION_MS))
                .ok_or(name!("action-deadline"; 32))?;
        }
        yield_once().await;
    }
    irq.mask_all_mac_interrupts();
    Ok(())
}

/// `tasks/200`: an exhaust segment holds from its start, is out once a scan
/// finds the ring empty, and is restored after the pause; any other segment
/// is idle, so units kept by an unfinished hold recycle there.
fn exhaust_phase(state: &mut State, segment: usize, access: &RadioAccess) -> Result<(), Name<32>> {
    let arm = *ARMS.get(segment).ok_or(name!("segment"; 32))?;
    let now = state.now_us;
    if !arm.exhaust() {
        state.exhaust = Exhaust::Idle;
        return Ok(());
    }
    let hw = State::hardware_now(access);
    let phase = state.exhaust;
    let event = state
        .exhaust_events
        .get_mut(segment)
        .ok_or(name!("exhaust-event"; 32))?;
    let since = |at: u32| u64::from(now.saturating_sub(at));
    state.exhaust = match phase {
        Exhaust::Idle => {
            event.hold_us = now;
            event.hw_at_hold = hw;
            Exhaust::Holding
        }
        Exhaust::Holding if since(event.hold_us) >= EXHAUST_HOLD_LIMIT_US => {
            event.out_us = now;
            event.by_limit = 1;
            event.hw_at_out = hw;
            Exhaust::Out
        }
        Exhaust::Out if since(event.out_us) >= EXHAUST_PAUSE_US => {
            event.restore_us = now;
            event.hw_at_restore = hw;
            Exhaust::Restored
        }
        phase => phase,
    };
    Ok(())
}

/// After the walker stopped: recheck every kept and every still pending
/// retired spare, every never bound spare, and the HAL's accounting.
pub(super) fn finish(
    state: &mut State,
    storage: &'static Storage,
    bank: &'static Spares,
) -> Result<(), Name<32>> {
    let last = SEGMENTS - 1;
    // The pending ones are kept from here: the run ended before their laps.
    while state.pending_len != 0 {
        let retired = *state
            .pending
            .get(state.pending_head)
            .ok_or(name!("pending"; 32))?;
        state.pending_head = (state.pending_head + 1) % RX_REBIND_SPARES;
        state.pending_len -= 1;
        state.keep(retired)?;
        bump(&mut state.kept_at_end);
    }
    state.min_lap_descriptors = u32::MAX;
    for position in 0..state.kept_len {
        let retired = *state.kept.get(position).ok_or(name!("kept"; 32))?;
        let spare = usize::try_from(retired.spare).map_err(|_| name!("kept"; 32))?;
        if !storage.rebind_spare_retired(spare) {
            state.record(
                last,
                Violation {
                    kind: Kind::Accounting as u32,
                    segment: 0xf,
                    spare: retired.spare,
                    offset: 0,
                    expected: 1,
                    observed: 0,
                },
            );
        }
        let held = state
            .descriptors_total
            .saturating_sub(retired.descriptors_at_retire);
        state.min_lap_descriptors = state.min_lap_descriptors.min(held);
        if held >= COUNT as u32 {
            bump(&mut state.kept_with_lap);
        }
        bump(&mut state.kept_final_checks);
        let crc = crc_spare(bank, spare).ok_or(name!("kept-read"; 32))?;
        if crc != retired.crc {
            state.record(
                last,
                Violation {
                    kind: Kind::Retired as u32,
                    segment: 0xf,
                    spare: retired.spare,
                    offset: retired.segment,
                    expected: retired.crc,
                    observed: crc,
                },
            );
        }
    }
    for spare in 0..RX_REBIND_SPARES {
        if storage.rebind_spare_unbound(spare) && spare >= POOL_END {
            bump(&mut state.unbound);
        }
        if spare >= POOL_END && state.check_spare(last, storage, bank, spare)?.is_some() {
            bump(&mut state.unbound_final_checks);
        }
    }
    Ok(())
}

fn kind(code: u32) -> Option<Kind> {
    [
        Kind::Binding,
        Kind::Copy,
        Kind::Landing,
        Kind::Window,
        Kind::Retired,
        Kind::Spare,
        Kind::Accounting,
        Kind::Content,
        Kind::CopyOther,
    ]
    .into_iter()
    .find(|kind| *kind as u32 == code)
}

/// The observations the verdict decides on.
pub(super) fn observations(state: &State) -> Observations {
    Observations {
        counts: state.counts,
        violations: state.violations_total,
        first_violation: kind(state.first_violation),
        retired: state.retired_total,
        returned: state.returned_total,
        kept: state.kept_len as u32,
        kept_final_checks: state.kept_final_checks,
        kept_with_lap: state.kept_with_lap,
        unbound: state.unbound,
        unbound_final_checks: state.unbound_final_checks,
    }
}

pub(super) async fn dump(state: &State, console: &mut usb::Console) -> Result<(), Name<32>> {
    let mut line = heapless::String::<768>::new();
    // `tasks/229`: the trace around the tails' links.
    for index in 0..oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_rebind::trace_used() {
        let Some(words) =
            oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_rebind::trace_read(index)
        else {
            continue;
        };
        let mut trace = heapless::String::<1280>::new();
        write!(&mut trace, "TRACE {index}").map_err(|_| name!("format"; 32))?;
        for word in words {
            write!(&mut trace, " {word:x}").map_err(|_| name!("format"; 32))?;
        }
        writeln!(&mut trace).map_err(|_| name!("format"; 32))?;
        usb::text(console, trace.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
    }
    for (segment, (counts, arm)) in state.counts.iter().zip(ARMS).enumerate() {
        line.clear();
        writeln!(
            &mut line,
            "SEG {segment} arm={} hold={} services={} units={} descriptors={} chained={} frames={} \
             prefix_ok={} prefix_err={} fcs_ok={} fcs_bad={} fcs_absent={} content_ok={} \
             content_bad={} frames_on_rebound={} detached={} window_checks={} rebinds={} \
             rebinds_at_last={} exhausted={} covered_rebinds={} returned={} spares_exhausted={} held_back={} cursor_moved={} retired_checks={} \
             spare_checks={} reload_deferred={} violations={} hw_mpdu={} hw_fcs_error={} \
             hw_buffer_full={} hw_data_success={} sender_first={} sender_last={} \
             sender_lost={} reserve_min={} reserve={:?}",
            match arm.act {
                Act::Fixed => "fixed",
                Act::Rebind => "rebind",
                Act::Exhaust => "exhaust",
                Act::Exit => "exit",
            },
            arm.hold,
            counts.services,
            counts.units,
            counts.descriptors,
            counts.chained,
            counts.frames,
            counts.prefix_ok,
            counts.prefix_err,
            counts.fcs_ok,
            counts.fcs_bad,
            counts.fcs_absent,
            counts.content_ok,
            counts.content_bad,
            counts.frames_on_rebound,
            counts.detached,
            counts.window_checks,
            counts.rebinds,
            counts.rebinds_at_last,
            counts.exhausted,
            counts.covered_rebinds,
            counts.returned,
            counts.spares_exhausted,
            counts.held_back,
            counts.cursor_moved,
            counts.retired_checks,
            counts.spare_checks,
            counts.reload_deferred,
            counts.violations,
            counts.hw_mpdu,
            counts.hw_fcs_error,
            counts.hw_buffer_full,
            counts.hw_data_success,
            if counts.content_ok == 0 {
                0
            } else {
                counts.counter_first
            },
            counts.counter_last,
            // The sender's frames of this segment's span it did not complete.
            if counts.content_ok == 0 {
                0
            } else {
                counts
                    .counter_last
                    .saturating_sub(counts.counter_first)
                    .saturating_add(1)
                    .saturating_sub(counts.content_ok)
            },
            if counts.rebinds == 0 {
                0
            } else {
                counts.reserve_min
            },
            counts.reserve,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
    }
    line.clear();
    writeln!(
        &mut line,
        "T200 first_sn={} first_sn_us={}",
        state.first_sn, state.first_sn_us
    )
    .map_err(|_| name!("format"; 32))?;
    usb::text(console, line.as_bytes())
        .await
        .map_err(|_| name!("console-write"; 32))?;
    for (segment, (event, arm)) in state.exhaust_events.iter().zip(ARMS).enumerate() {
        if !arm.exhaust() && !arm.exit() {
            continue;
        }
        line.clear();
        writeln!(
            &mut line,
            "C2 {segment} arm={} next={} last={} tail={} tail_words={:x?} next_words={:x?} \
             hole={} hole_words={:x?} exits={} next_zero={} next_hole={} next_other={}",
            if arm.exit() { "exit" } else { "exhaust" },
            event.next,
            event.last,
            event.tail,
            event.tail_words,
            event.next_words,
            event.hole,
            event.hole_words,
            event.exits,
            event.exit_next_zero,
            event.exit_next_hole,
            event.exit_next_other,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
        if !arm.exhaust() {
            continue;
        }
        line.clear();
        let before_len = (event.before_len as usize).min(EXHAUST_SNS);
        let first = event.before_len as usize - before_len;
        writeln!(
            &mut line,
            "EXH {segment} hold_us={} out_us={} restore_us={} after_first_us={} held={} \
             by_limit={} next={} last={} hw_hold={:?} hw_out={:?} hw_restore={:?} \
             before_total={} after_len={}",
            event.hold_us,
            event.out_us,
            event.restore_us,
            event.after_first_us,
            event.held,
            event.by_limit,
            event.next,
            event.last,
            event.hw_at_hold,
            event.hw_at_out,
            event.hw_at_restore,
            event.before_len,
            event.after_len,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
        for (tag, sequences) in [
            (
                "EXHB",
                (first..first + before_len)
                    .filter_map(|at| event.before.get(at % EXHAUST_SNS).copied())
                    .collect::<heapless::Vec<u16, EXHAUST_SNS>>(),
            ),
            (
                "EXHA",
                event
                    .after
                    .iter()
                    .take(event.after_len as usize)
                    .copied()
                    .collect(),
            ),
        ] {
            line.clear();
            writeln!(&mut line, "{tag} {segment} {:?}", sequences.as_slice())
                .map_err(|_| name!("format"; 32))?;
            usb::text(console, line.as_bytes())
                .await
                .map_err(|_| name!("console-write"; 32))?;
        }
    }
    line.clear();
    writeln!(
        &mut line,
        "FINAL retired={} returned={} kept={} kept_at_end={} kept_final_checks={} \
         kept_with_lap={} min_held_descriptors={} unbound={} unbound_final_checks={} \
         descriptors={} violations={} violations_recorded={}",
        state.retired_total,
        state.returned_total,
        state.kept_len,
        state.kept_at_end,
        state.kept_final_checks,
        state.kept_with_lap,
        if state.kept_len == 0 {
            0
        } else {
            state.min_lap_descriptors
        },
        state.unbound,
        state.unbound_final_checks,
        state.descriptors_total,
        state.violations_total,
        state.violations_len,
    )
    .map_err(|_| name!("format"; 32))?;
    usb::text(console, line.as_bytes())
        .await
        .map_err(|_| name!("console-write"; 32))?;
    for violation in state.violations.iter().take(state.violations_len) {
        line.clear();
        writeln!(
            &mut line,
            "V kind={} seg={:x} spare={:x} offset={:x} expected={:x} observed={:x}",
            kind(violation.kind).map_or("unknown", Kind::name),
            violation.segment,
            violation.spare,
            violation.offset,
            violation.expected,
            violation.observed,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
    }
    // `tasks/194`.
    line.clear();
    writeln!(
        &mut line,
        "T196 kinds={:?} header_or_tail_only={} header_differs={} tail_differs={} frame_differs={} landing(neither,old,new,both)={:?} stopped={} stop_descriptor={}",
        state.kinds,
        state.known_late,
        state.header_differs,
        state.tail_differs,
        state.frame_differs,
        state.landing,
        state.stop,
        state.stop_descriptor,
    )
    .map_err(|_| name!("format"; 32))?;
    usb::text(console, line.as_bytes())
        .await
        .map_err(|_| name!("console-write"; 32))?;
    for (tag, late) in [
        ("at_last", &state.late_at_last),
        ("not_last", &state.late_not_last),
        ("at_last_rebind", &state.late_at_last_rebind),
        ("not_last_rebind", &state.late_not_last_rebind),
    ] {
        line.clear();
        writeln!(
            &mut line,
            "LATE {tag} takes={} written_at_take={} changed={} never={} min_cycles={} max_cycles={} delay_4us={:?}",
            late.takes,
            late.written_at_take,
            late.changed,
            late.never,
            if late.changed == 0 { 0 } else { late.min },
            late.max,
            late.delay,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
    }
    // The differing bytes by offset, eight offsets a line, non-zero only.
    let mut pending = 0;
    line.clear();
    for (offset, count) in state.copy_offsets.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        if pending == 0 {
            line.clear();
            let _ = write!(&mut line, "OFFSETS");
        }
        let _ = write!(&mut line, " {offset}:{count}");
        pending += 1;
        if pending == 8 {
            let _ = writeln!(&mut line);
            usb::text(console, line.as_bytes())
                .await
                .map_err(|_| name!("console-write"; 32))?;
            pending = 0;
        }
    }
    if pending != 0 {
        let _ = writeln!(&mut line);
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
    }
    if let Some(act) = state.acts.get(state.stop_descriptor as usize) {
        line.clear();
        writeln!(
            &mut line,
            "STOP descriptor={} act_valid={} before={:08x?} after={:08x?} next_low={:x} last_low={:x} now={:08x?}",
            state.stop_descriptor,
            act.valid,
            act.before,
            act.after,
            act.next_low,
            act.last_low,
            state.stop_now,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes())
            .await
            .map_err(|_| name!("console-write"; 32))?;
    }
    Ok(())
}
