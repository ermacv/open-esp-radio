//! TEMPORARY experiment: one live RX ring run in four time segments, fixed
//! binding, rebind, rebind, fixed (A–B–B–A), with the same detach cadence in
//! every segment. Only completed single-descriptor units strictly before the
//! frozen LAST are detached; LAST, the descriptor NEXT names and R are never
//! touched by the experiment.
use super::{BUFFER, COUNT, Spares, Storage, usb};
use bytemuck::Zeroable;
use core::fmt::Write as _;
use oer_chips_esp32s31_hal::{
    radio::ieee80211::{
        dma::{
            descriptor::cursor_ring_index,
            rx_rebind::{RX_REBIND_SPARES, RxBinding},
            rx_ring::{RxRingError, RxRingLive, RxSegment},
            rx_storage::RxDmaBuffer,
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

pub(super) const SEGMENTS: usize = 4;
/// The arm of each segment: A–B–B–A.
const REBIND_SEGMENT: [bool; SEGMENTS] = [false, true, true, false];
const SEGMENT_MS: u64 = 2500;
/// One detach (and, in a rebind segment, one rebind) at most this often.
const ACTION_MS: u32 = 80;
const UNITS_PER_SCAN: usize = 8;
const RETIRED_CHECKS_PER_SCAN: usize = 2;
const SPARE_CHECKS_PER_SCAN: usize = 1;
const RETIRED_CAPACITY: usize = COUNT + RX_REBIND_SPARES;
const VIOLATION_CAPACITY: usize = 16;
const WORDS: usize = BUFFER / 4;
/// Words compared between the HAL's copy and the bound buffer itself.
const COPY_WORDS: usize = 16;
const _: () = {
    if !BUFFER.is_multiple_of(4) || COPY_WORDS > WORDS {
        panic!("buffer geometry");
    }
};

/// The canary of spare `spare`'s word `word`.
pub(super) const fn canary(spare: usize, word: usize) -> u32 {
    0x5a00_0000 | ((spare as u32 & 0xff) << 16) | (word as u32 & 0xffff)
}

#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct Counts {
    services: u32,
    units: u32,
    chained: u32,
    frames: u32,
    prefix_ok: u32,
    prefix_err: u32,
    fcs_ok: u32,
    fcs_bad: u32,
    fcs_absent: u32,
    frames_on_spare: u32,
    first_landings: u32,
    detached: u32,
    rebound: u32,
    spares_exhausted: u32,
    not_before_last: u32,
    window_checks: u32,
    retired_checks: u32,
    spare_checks: u32,
    violations: u32,
    reload_deferred: u32,
    hw_mpdu: u32,
    hw_fcs_error: u32,
    hw_buffer_full: u32,
    hw_data_success: u32,
}

const KIND_BINDING: u32 = 1;
const KIND_COPY: u32 = 2;
const KIND_LANDING: u32 = 3;
const KIND_WINDOW: u32 = 4;
const KIND_RETIRED: u32 = 5;
const KIND_SPARE: u32 = 6;
const KIND_ACCOUNTING: u32 = 7;

#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct Violation {
    kind: u32,
    segment: u32,
    /// Placed buffer `i` as `i`, spare `k` as `0x100 + k`.
    buffer: u32,
    word: u32,
    expected: u32,
    observed: u32,
}

#[derive(Clone, Copy, Zeroable)]
#[repr(C)]
struct Retired {
    /// Binding code: 0 placed buffer of `descriptor`, k + 1 spare k.
    binding: u32,
    descriptor: u32,
    crc: u32,
    segment: u32,
}

#[derive(Zeroable)]
pub(super) struct State {
    counts: [Counts; SEGMENTS],
    retired: [Retired; RETIRED_CAPACITY],
    retired_len: usize,
    retired_cursor: usize,
    violations: [Violation; VIOLATION_CAPACITY],
    violations_len: usize,
    /// Per descriptor, the binding the probe expects: 0 placed, k + 1 spare k.
    expected: [u32; COUNT],
    /// Per descriptor, whether its spare has received no frame yet.
    fresh: [bool; COUNT],
    next_spare: usize,
    spare_cursor: usize,
    hw_last: [u16; 4],
    final_retired: u32,
    final_retired_violations: u32,
    final_spares: u32,
    final_spare_violations: u32,
}

/// The HAL's copy of one completed frame.
pub(super) type Frame = [u8; BUFFER];

static STATE: ZeroedStatic<State> = ZeroedStatic::new();
static FRAME: ZeroedStatic<Frame> = ZeroedStatic::new();

pub(super) fn take() -> Result<(&'static mut State, &'static mut Frame), Name<32>> {
    let state = STATE.try_take().ok_or(name!("state-claimed"; 32))?;
    let frame = FRAME.try_take().ok_or(name!("frame-claimed"; 32))?;
    Ok((state, frame))
}

fn bump(counter: &mut u32) {
    *counter = counter.saturating_add(1);
}

fn binding_code(binding: RxBinding) -> u32 {
    match binding {
        RxBinding::Placed => 0,
        RxBinding::Spare(spare) => spare as u32 + 1,
    }
}

fn buffer_label(code: u32, descriptor: usize) -> u32 {
    if code == 0 {
        descriptor as u32
    } else {
        0x100 + code - 1
    }
}

/// The buffer a binding code names.
fn buffer_of(
    storage: &'static Storage,
    bank: &'static Spares,
    descriptor: usize,
    code: u32,
) -> Option<&'static RxDmaBuffer<BUFFER, BUFFER>> {
    if code == 0 {
        storage
            .buffers()
            .get(storage.descriptor_buffer_id(descriptor)?)
    } else {
        bank.buffers().get(usize::try_from(code - 1).ok()?)
    }
}

fn address_of(buffer: &RxDmaBuffer<BUFFER, BUFFER>) -> u32 {
    // ChipDma: a field's address is its DMA address.
    core::ptr::from_ref(buffer).addr() as u32
}

// The experiment observes DMA memory the hardware may write at any time:
// value-only volatile loads, never a byte reference.
#[allow(
    unsafe_code,
    reason = "probe-only volatile observation of a placed RX DMA buffer"
)]
fn read_word(buffer: &RxDmaBuffer<BUFFER, BUFFER>, word: usize) -> Option<u32> {
    if word >= WORDS {
        return None;
    }
    // SAFETY: `RxDmaBuffer` is `repr(C, align(4))` with its
    // `UnsafeCell<[u8; BUFFER]>` byte cell at offset zero, so `word * 4 + 4
    // <= BUFFER` lies inside that cell and is u32 aligned. The buffer is a
    // live `'static` DMA allocation of this image. The load creates no
    // reference to the bytes and transfers no ownership; DMA may write the
    // cell concurrently, which is what the observation records.
    Some(unsafe {
        core::ptr::from_ref(buffer)
            .cast::<u32>()
            .add(word)
            .read_volatile()
    })
}

use crate::crc::{CRC32, crc32};

/// CRC-32 over a whole buffer by volatile word loads.
fn crc_buffer(buffer: &RxDmaBuffer<BUFFER, BUFFER>) -> u32 {
    let mut digest = CRC32.digest();
    for word in 0..WORDS {
        digest.update(&read_word(buffer, word).unwrap_or(0).to_le_bytes());
    }
    digest.finalize()
}

impl State {
    fn record(&mut self, segment: usize, violation: Violation) {
        if let Some(counts) = self.counts.get_mut(segment) {
            counts.violations = counts.violations.saturating_add(1);
        }
        if let Some(slot) = self.violations.get_mut(self.violations_len) {
            *slot = violation;
            self.violations_len = self.violations_len.saturating_add(1);
        }
    }

    fn counts(&mut self, segment: usize) -> Result<&mut Counts, Name<32>> {
        self.counts.get_mut(segment).ok_or(name!("segment"; 32))
    }

    /// Check one committed single-descriptor frame, copied by the HAL from
    /// the buffer its accounting names, before it is detached or recycled.
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
        let code = *self.expected.get(index).ok_or(name!("expected"; 32))?;
        let buffer = buffer_of(storage, bank, index, code).ok_or(name!("frame-buffer"; 32))?;
        let published = storage
            .rebind_published(index)
            .map(binding_code)
            .ok_or(name!("published"; 32))?;
        let counts = self.counts(segment)?;
        counts.frames = counts.frames.saturating_add(1);
        if code != 0 {
            counts.frames_on_spare = counts.frames_on_spare.saturating_add(1);
        }
        if descriptor.buffer_address() != address_of(buffer) || published != code {
            self.record(
                segment,
                Violation {
                    kind: KIND_BINDING,
                    segment: segment as u32,
                    buffer: buffer_label(code, index),
                    word: published,
                    expected: address_of(buffer),
                    observed: descriptor.buffer_address(),
                },
            );
        }
        let frame = frame.get(..length).ok_or(name!("frame-length"; 32))?;
        let mut copy_mismatch = None;
        let mut canary_words = 0u32;
        let spare = usize::try_from(code).ok().and_then(|c| c.checked_sub(1));
        for (word, chunk) in frame.chunks_exact(4).take(COPY_WORDS).enumerate() {
            let copied = u32::from_le_bytes([
                *chunk.first().ok_or(name!("chunk"; 32))?,
                *chunk.get(1).ok_or(name!("chunk"; 32))?,
                *chunk.get(2).ok_or(name!("chunk"; 32))?,
                *chunk.get(3).ok_or(name!("chunk"; 32))?,
            ]);
            let observed = read_word(buffer, word).ok_or(name!("copy-word"; 32))?;
            if copied != observed && copy_mismatch.is_none() {
                copy_mismatch = Some((word, copied, observed));
            }
            if spare.is_some_and(|spare| copied == canary(spare, word)) {
                canary_words = canary_words.saturating_add(1);
            }
        }
        if let Some((word, copied, observed)) = copy_mismatch {
            self.record(
                segment,
                Violation {
                    kind: KIND_COPY,
                    segment: segment as u32,
                    buffer: buffer_label(code, index),
                    word: word as u32,
                    expected: copied,
                    observed,
                },
            );
        }
        let fresh = self.fresh.get_mut(index).ok_or(name!("fresh"; 32))?;
        let first = core::mem::replace(fresh, false);
        if first {
            let counts = self.counts(segment)?;
            counts.first_landings = counts.first_landings.saturating_add(1);
        }
        if canary_words != 0 {
            self.record(
                segment,
                Violation {
                    kind: KIND_LANDING,
                    segment: segment as u32,
                    buffer: buffer_label(code, index),
                    word: u32::from(first),
                    expected: 0,
                    observed: canary_words,
                },
            );
        }
        let segment_view = RxSegment {
            descriptor_address: 0,
            descriptor_word0: descriptor.word0(),
            buffer: frame,
            next_descriptor_address: 0,
        };
        let layout = first_segment_layout(
            &segment_view,
            RxIngressConfig {
                ring_entry_limit: COUNT,
                csi_config: 0,
                flags: 0,
            },
        );
        let counts = self.counts(segment)?;
        let Ok(layout) = layout else {
            counts.prefix_err = counts.prefix_err.saturating_add(1);
            return Ok(());
        };
        counts.prefix_ok = counts.prefix_ok.saturating_add(1);
        let body_end = layout
            .frame_offset
            .checked_add(layout.expected_frame_length)
            .ok_or(name!("fcs-range"; 32))?;
        let fcs_end = body_end
            .checked_add(FCS_SIZE)
            .ok_or(name!("fcs-range"; 32))?;
        match (
            frame.get(layout.frame_offset..body_end),
            frame.get(body_end..fcs_end),
        ) {
            (Some(body), Some(fcs)) => {
                let stored =
                    u32::from_le_bytes(fcs.try_into().map_err(|_| name!("fcs-width"; 32))?);
                if crc32(body) == stored {
                    counts.fcs_ok = counts.fcs_ok.saturating_add(1);
                } else {
                    counts.fcs_bad = counts.fcs_bad.saturating_add(1);
                }
            }
            _ => counts.fcs_absent = counts.fcs_absent.saturating_add(1),
        }
        Ok(())
    }

    /// Recheck `limit` retired buffers round-robin.
    fn check_retired(
        &mut self,
        segment: usize,
        storage: &'static Storage,
        bank: &'static Spares,
        limit: usize,
    ) -> Result<(), Name<32>> {
        for _ in 0..limit.min(self.retired_len) {
            let position = self.retired_cursor % self.retired_len.max(1);
            self.retired_cursor = position.saturating_add(1);
            let retired = *self.retired.get(position).ok_or(name!("retired"; 32))?;
            let descriptor =
                usize::try_from(retired.descriptor).map_err(|_| name!("retired"; 32))?;
            let buffer = buffer_of(storage, bank, descriptor, retired.binding)
                .ok_or(name!("retired-buffer"; 32))?;
            let counts = self.counts(segment)?;
            counts.retired_checks = counts.retired_checks.saturating_add(1);
            let crc = crc_buffer(buffer);
            if crc != retired.crc {
                self.record(
                    segment,
                    Violation {
                        kind: KIND_RETIRED,
                        segment: segment as u32,
                        buffer: buffer_label(retired.binding, descriptor),
                        word: retired.segment,
                        expected: retired.crc,
                        observed: crc,
                    },
                );
            }
        }
        Ok(())
    }

    /// Check spare `spare`'s canary when it was never bound; the first
    /// differing word is recorded.
    fn check_spare(
        &mut self,
        segment: usize,
        storage: &'static Storage,
        bank: &'static Spares,
        spare: usize,
    ) -> Result<Option<bool>, Name<32>> {
        if !storage.rebind_spare_unbound(spare) {
            return Ok(None);
        }
        let buffer = bank.buffers().get(spare).ok_or(name!("spare"; 32))?;
        for word in 0..WORDS {
            let observed = read_word(buffer, word).ok_or(name!("spare-word"; 32))?;
            if observed != canary(spare, word) {
                self.record(
                    segment,
                    Violation {
                        kind: KIND_SPARE,
                        segment: segment as u32,
                        buffer: 0x100 + spare as u32,
                        word: word as u32,
                        expected: canary(spare, word),
                        observed,
                    },
                );
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
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
                let counts = self.counts(segment)?;
                counts.spare_checks = counts.spare_checks.saturating_add(1);
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

/// Serve one scan: up to `UNITS_PER_SCAN` units, each checked and recycled
/// through the production append; at most one detach when `action` is due.
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
    let rebind = *REBIND_SEGMENT.get(segment).ok_or(name!("segment"; 32))?;
    bump(&mut state.counts(segment)?.services);
    if irq.mac_interrupt_raw().fatal_assertion_raw().is_some() {
        return Err(name!("fatal-raw"; 32));
    }
    let status = irq.mac_interrupt_status();
    irq.acknowledge_mac_interrupts(status.retain_rx_causes(MacRxInterrupts::ALL));
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
        // Strictly before the frozen LAST and not the descriptor NEXT names.
        let before_last = {
            let base = ring.descriptor_base();
            let last = cursor_ring_index(frozen.last_descriptor_low(), base, COUNT);
            let next = cursor_ring_index(frozen.next_descriptor_low(), base, COUNT);
            last.is_some() && last != Some(head) && next != Some(head)
        };
        let mut unit = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| name!("take"; 32))?
            .ok_or(name!("take-none"; 32))?;
        if unit.head_index() != head {
            unit.retain_for_deferred_recycle();
            return Err(name!("take-head"; 32));
        }
        {
            let counts = state.counts(segment)?;
            counts.units = counts.units.saturating_add(1);
            if unit.descriptor_count() != 1 {
                counts.chained = counts.chained.saturating_add(1);
            }
        }
        if copied && let Err(error) = state.check_frame(segment, storage, bank, frame, head, length)
        {
            unit.retain_for_deferred_recycle();
            return Err(error);
        }
        if copied && *action {
            if !before_last {
                bump(&mut state.counts(segment)?.not_before_last);
            } else if let Err(error) = act(
                state, segment, rebind, storage, bank, &mut unit, frozen, access,
            ) {
                unit.retain_for_deferred_recycle();
                return Err(error);
            } else {
                *action = false;
            }
        }
        let Some(credit) = unit.retain_discard() else {
            return Err(name!("recycle-frontier"; 32));
        };
        match credit.append(access.rx_dma()) {
            Ok(_) => {}
            Err(RxRingError::ReloadPending) => {
                // The settle rang the doorbell again; the unit stays observed
                // and is appended by a later deferred claim.
                bump(&mut state.counts(segment)?.reload_deferred);
            }
            Err(_) => return Err(name!("append"; 32)),
        }
        reload(ring, access)?;
    }
    state.check_retired(segment, storage, bank, RETIRED_CHECKS_PER_SCAN)?;
    state.check_spares(segment, storage, bank, SPARE_CHECKS_PER_SCAN)?;
    state.sample_hardware(segment, access)
}

/// Detach the taken unit in place; in a rebind segment bind its next re-arm
/// to the next unbound spare and retire the old buffer with its CRC; in a
/// fixed segment recheck the old buffer before the same re-arm.
#[allow(clippy::too_many_arguments)]
fn act(
    state: &mut State,
    segment: usize,
    rebind: bool,
    storage: &'static Storage,
    bank: &'static Spares,
    unit: &mut oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_storage::RxDmaCompletedUnit<
        'static,
        '_,
        '_,
        COUNT,
        BUFFER,
        BUFFER,
    >,
    frozen: oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxFrozenCursor,
    access: &mut RadioAccess,
) -> Result<(), Name<32>> {
    let index = unit.head_index();
    unit.detach_before_last(access.rx_dma(), frozen)
        .map_err(|_| name!("detach"; 32))?;
    let code = *state.expected.get(index).ok_or(name!("expected"; 32))?;
    let old = buffer_of(storage, bank, index, code).ok_or(name!("old-buffer"; 32))?;
    let crc = crc_buffer(old);
    bump(&mut state.counts(segment)?.detached);
    if !rebind {
        bump(&mut state.counts(segment)?.window_checks);
        let again = crc_buffer(old);
        if again != crc {
            state.record(
                segment,
                Violation {
                    kind: KIND_WINDOW,
                    segment: segment as u32,
                    buffer: buffer_label(code, index),
                    word: 0,
                    expected: crc,
                    observed: again,
                },
            );
        }
        return Ok(());
    }
    if state.next_spare >= RX_REBIND_SPARES {
        bump(&mut state.counts(segment)?.spares_exhausted);
        return Ok(());
    }
    let spare = state.next_spare;
    unit.rebind_next(spare).map_err(|_| name!("rebind"; 32))?;
    state.next_spare = spare.saturating_add(1);
    let slot = state
        .retired
        .get_mut(state.retired_len)
        .ok_or(name!("retired-full"; 32))?;
    *slot = Retired {
        binding: code,
        descriptor: index as u32,
        crc,
        segment: segment as u32,
    };
    state.retired_len = state.retired_len.saturating_add(1);
    *state.expected.get_mut(index).ok_or(name!("expected"; 32))? = spare as u32 + 1;
    *state.fresh.get_mut(index).ok_or(name!("fresh"; 32))? = true;
    bump(&mut state.counts(segment)?.rebound);
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

/// Run the four segments on the started ring.
pub(super) async fn run(
    state: &mut State,
    frame: &mut Frame,
    storage: &'static Storage,
    bank: &'static Spares,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    irq: &mut MacInterruptRegisters,
) -> Result<(), Name<32>> {
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
    loop {
        let now = EmbassyClock.now();
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
        yield_once().await;
        // An action that found no eligible unit stays due for the next scan.
        if due && !action {
            next_action = now
                .checked_add(Duration::from_millis(ACTION_MS))
                .ok_or(name!("action-deadline"; 32))?;
        }
    }
    irq.mask_all_mac_interrupts();
    Ok(())
}

/// After the walker stopped: recheck every retired buffer and every never
/// bound spare, and the HAL's retirement accounting.
pub(super) fn finish(
    state: &mut State,
    storage: &'static Storage,
    bank: &'static Spares,
) -> Result<(), Name<32>> {
    let last = SEGMENTS - 1;
    for position in 0..state.retired_len {
        let retired = *state.retired.get(position).ok_or(name!("retired"; 32))?;
        let descriptor = usize::try_from(retired.descriptor).map_err(|_| name!("retired"; 32))?;
        let buffer = buffer_of(storage, bank, descriptor, retired.binding)
            .ok_or(name!("retired-buffer"; 32))?;
        let accounted = if retired.binding == 0 {
            storage.rebind_placed_retired(descriptor)
        } else {
            storage.rebind_spare_retired(
                usize::try_from(retired.binding - 1).map_err(|_| name!("retired"; 32))?,
            )
        };
        if !accounted {
            state.record(
                last,
                Violation {
                    kind: KIND_ACCOUNTING,
                    segment: last as u32,
                    buffer: buffer_label(retired.binding, descriptor),
                    word: 0,
                    expected: 1,
                    observed: 0,
                },
            );
        }
        state.final_retired = state.final_retired.saturating_add(1);
        let crc = crc_buffer(buffer);
        if crc != retired.crc {
            state.final_retired_violations = state.final_retired_violations.saturating_add(1);
            state.record(
                last,
                Violation {
                    kind: KIND_RETIRED,
                    segment: 0xf,
                    buffer: buffer_label(retired.binding, descriptor),
                    word: retired.segment,
                    expected: retired.crc,
                    observed: crc,
                },
            );
        }
    }
    for spare in 0..RX_REBIND_SPARES {
        match state.check_spare(last, storage, bank, spare)? {
            None => {}
            Some(clean) => {
                state.final_spares = state.final_spares.saturating_add(1);
                if !clean {
                    state.final_spare_violations = state.final_spare_violations.saturating_add(1);
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn dump(state: &State, console: &mut usb::Console) -> Result<(), Name<32>> {
    let mut line = heapless::String::<512>::new();
    for (segment, counts) in state.counts.iter().enumerate() {
        line.clear();
        let arm = if REBIND_SEGMENT.get(segment).copied().unwrap_or(false) {
            "rebind"
        } else {
            "fixed"
        };
        writeln!(
            &mut line,
            "SEG {segment} arm={arm} services={} units={} chained={} frames={} prefix_ok={} \
             prefix_err={} fcs_ok={} fcs_bad={} fcs_absent={} frames_on_spare={} \
             first_landings={} detached={} rebound={} spares_exhausted={} not_before_last={} \
             window_checks={} retired_checks={} spare_checks={} violations={} \
             reload_deferred={} hw_mpdu={} hw_fcs_error={} hw_buffer_full={} hw_data_success={}",
            counts.services,
            counts.units,
            counts.chained,
            counts.frames,
            counts.prefix_ok,
            counts.prefix_err,
            counts.fcs_ok,
            counts.fcs_bad,
            counts.fcs_absent,
            counts.frames_on_spare,
            counts.first_landings,
            counts.detached,
            counts.rebound,
            counts.spares_exhausted,
            counts.not_before_last,
            counts.window_checks,
            counts.retired_checks,
            counts.spare_checks,
            counts.violations,
            counts.reload_deferred,
            counts.hw_mpdu,
            counts.hw_fcs_error,
            counts.hw_buffer_full,
            counts.hw_data_success,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes()).await;
    }
    line.clear();
    writeln!(
        &mut line,
        "FINAL retired={} retired_violations={} unbound_spares={} spare_violations={} \
         spares_used={} violations_recorded={}",
        state.final_retired,
        state.final_retired_violations,
        state.final_spares,
        state.final_spare_violations,
        state.next_spare,
        state.violations_len,
    )
    .map_err(|_| name!("format"; 32))?;
    usb::text(console, line.as_bytes()).await;
    for violation in state.violations.iter().take(state.violations_len) {
        line.clear();
        writeln!(
            &mut line,
            "V kind={} seg={:x} buffer={:x} word={:x} expected={:x} observed={:x}",
            violation.kind,
            violation.segment,
            violation.buffer,
            violation.word,
            violation.expected,
            violation.observed,
        )
        .map_err(|_| name!("format"; 32))?;
        usb::text(console, line.as_bytes()).await;
    }
    Ok(())
}
