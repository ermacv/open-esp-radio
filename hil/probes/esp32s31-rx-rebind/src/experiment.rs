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
use crate::verdict::{ARMS, Counts, Kind, Observations, RESERVE_BUCKETS, SEGMENTS};
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

const SEGMENT_MS: u64 = 2500;
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
        if let Some(offset) = (0..compared).find(|&i| frame.get(i).copied() != spare_byte(i)) {
            clean = false;
            self.record(
                segment,
                Violation {
                    kind: Kind::Copy as u32,
                    segment: segment as u32,
                    spare: spare as u32,
                    offset: offset as u32,
                    expected: u32::from(frame.get(offset).copied().unwrap_or(0)),
                    observed: u32::from(spare_byte(offset).unwrap_or(0)),
                },
            );
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
            descriptor_word0: descriptor.word0(),
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
    if irq.mac_interrupt_raw().fatal_assertion_raw().is_some() {
        return Err(name!("fatal-raw"; 32));
    }
    let status = irq.mac_interrupt_status();
    irq.acknowledge_mac_interrupts(status.retain_rx_causes(MacRxInterrupts::all()));
    storage
        .observe_reserved(ring, access.rx_dma())
        .map_err(|_| name!("reserved"; 32))?;
    reload(ring, access)?;
    for _ in 0..UNITS_PER_SCAN {
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| name!("cursor"; 32))?;
        // A unit whose append found the doorbell re-rung stays observed at
        // the recycle frontier; it is appended before any later take.
        if let Some(credit) = storage
            .claim_deferred_recycle(ring, frozen)
            .map_err(|_| name!("deferred-claim"; 32))?
        {
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
        if next == Some(head) || behind.is_none_or(|behind| behind < arm.hold) {
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
        if copied && let Err(error) = state.check_frame(segment, storage, bank, frame, head, length)
        {
            unit.retain_for_deferred_recycle();
            return Err(error);
        }
        if copied && (arm.rebind || *action) {
            if let Err(error) = act(
                state, segment, arm.rebind, bank, &mut unit, frozen, access, reserve,
            ) {
                unit.retain_for_deferred_recycle();
                return Err(error);
            }
            *action = false;
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
    state.sample_hardware(segment, access)
}

/// Detach the taken unit in place. A rebind segment binds its next re-arm to
/// an unbound spare of the pool and retires the old one with its CRC taken
/// now, recording the armed reserve it is linked behind; a fixed segment
/// rechecks the old spare once more before the same re-arm (a short window
/// only).
#[allow(clippy::too_many_arguments)]
fn act(
    state: &mut State,
    segment: usize,
    rebind: bool,
    bank: &'static Spares,
    unit: &mut RxDmaCompletedUnit<'static, '_, '_, COUNT, BUFFER, BUFFER>,
    frozen: RxFrozenCursor,
    access: &mut RadioAccess,
    reserve: usize,
) -> Result<(), Name<32>> {
    let index = unit.head_index();
    unit.detach_before_last(access.rx_dma(), frozen)
        .map_err(|_| name!("detach"; 32))?;
    let old = *state.expected.get(index).ok_or(name!("expected"; 32))?;
    let old_spare = usize::try_from(old).map_err(|_| name!("expected"; 32))?;
    let crc = crc_spare(bank, old_spare).ok_or(name!("old-read"; 32))?;
    bump(&mut state.counts(segment)?.detached);
    if !rebind {
        bump(&mut state.counts(segment)?.window_checks);
        let again = crc_spare(bank, old_spare).ok_or(name!("old-read"; 32))?;
        if again != crc {
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
    let Some(spare) = state.free_pop() else {
        bump(&mut state.counts(segment)?.spares_exhausted);
        return Ok(());
    };
    unit.rebind_next(spare).map_err(|_| name!("rebind"; 32))?;
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
        if state
            .counts
            .first()
            .is_some_and(|counts| counts.content_ok == 0)
        {
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
    for (segment, (counts, arm)) in state.counts.iter().zip(ARMS).enumerate() {
        line.clear();
        writeln!(
            &mut line,
            "SEG {segment} arm={} hold={} services={} units={} descriptors={} chained={} frames={} \
             prefix_ok={} prefix_err={} fcs_ok={} fcs_bad={} fcs_absent={} content_ok={} \
             content_bad={} frames_on_rebound={} detached={} window_checks={} rebinds={} \
             covered_rebinds={} returned={} spares_exhausted={} held_back={} retired_checks={} \
             spare_checks={} reload_deferred={} violations={} hw_mpdu={} hw_fcs_error={} \
             hw_buffer_full={} hw_data_success={} sender_first={} sender_last={} \
             sender_lost={} reserve_min={} reserve={:?}",
            if arm.rebind { "rebind" } else { "fixed" },
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
            counts.covered_rebinds,
            counts.returned,
            counts.spares_exhausted,
            counts.held_back,
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
    Ok(())
}
