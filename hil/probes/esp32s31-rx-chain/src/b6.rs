//! B6 of `tasks/144` (`tasks/149`, `176`): when the receive interrupt comes
//! inside an aggregate. The probe receives the stand's access point sending
//! UDP to the vendor station in open promiscuous receive, acknowledging
//! nothing, and for a bounded window polls two things together, each change
//! stamped with `mcycle`:
//!
//! - the MAC's receive cause `RX_SUCCESS` (bit 14 of the MAC's interrupt
//!   STATUS, enabled alone, cleared by its CLEAR word: `HW20_RX_RAW_RETENTION_20261027`
//!   qualifies the clear of 14 independently of its neighbours), with no CPU
//!   handler bound to the MAC's source in this image;
//! - LAST, the walker's last completed descriptor.
//!
//! Each unit taken is recycled as the product does; its descriptors'
//! OFFSET and SOSF are counted, and its RX-control timestamp groups the
//! units of one PPDU. A spinning probe is this probe's choice for a bounded
//! window, no precedent for a driver.
use super::{COUNT, Storage, usb};
use core::fmt::Write as _;
use oer_chips_esp32s31_hal::{
    layouts::{WifiRxControlPrefix, WifiRxControlTail},
    radio::ieee80211::{
        dma::{descriptor::cursor_ring_index, rx_dma::RxDma, rx_ring::RxRingLive},
        registers::{MacInterruptMask, MacRxLane},
    },
    system::owner::{MacInterruptRegisters, RadioAccess},
};
use oer_chips_esp32s31_radio_ieee80211_backend::rx::receive::RELOAD_SETTLE_LIMIT;
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, settle::Settle};

/// The window: long enough for the scenario's receive levels.
const WINDOW_MS: u32 = 180_000;
/// Polls between two returns to the executor (its watchdog feed).
const SLICE: u32 = 4_000;
/// The histograms' buckets: 0 to 30, then 31 or more.
const BUCKETS: usize = 32;
const HEADER: usize = WifiRxControlPrefix::SIZE + WifiRxControlTail::SIZE;

fn bump(cell: &mut u32) {
    *cell = cell.saturating_add(1);
}

fn bucket(histogram: &mut [u32; BUCKETS], value: u32) {
    if let Some(cell) = histogram.get_mut((value as usize).min(BUCKETS - 1)) {
        bump(cell);
    }
}

/// What the window counted.
struct Counts {
    polls: u64,
    /// The largest gap between two polls, cycles.
    gap_max: u32,
    rises: u32,
    /// Descriptors LAST advanced between one rise and the next.
    per_rise: [u32; BUCKETS],
    per_rise_max: u32,
    /// LAST's changes, and the descriptors each one advanced at once.
    steps: u32,
    per_step: [u32; BUCKETS],
    units: u32,
    descriptors: u32,
    offset_set: u32,
    sosf_set: u32,
    /// Units grouped by their RX-control timestamp: a run of equal
    /// timestamps is one PPDU.
    ppdus: u32,
    per_ppdu: [u32; BUCKETS],
    /// The cycles from a PPDU's first completed descriptor to its last, and
    /// the largest of them.
    ppdu_span_max: u32,
    ppdu_span: [u32; BUCKETS],
    /// Rises seen between a PPDU's first and last completion.
    rises_inside: u32,
    buffer_full: u32,
}

/// Run the window; the ring keeps running after it.
pub(super) async fn run(
    console: &mut usb::Console,
    storage: &Storage,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    irq: &mut MacInterruptRegisters,
) -> Result<(), oer_portable_diagnostics::Name<32>> {
    let mut line = heapless::String::<512>::new();
    let _ = writeln!(line, "B6 begin v=1 count={COUNT} window_ms={WINDOW_MS}");
    usb::text(console, line.as_bytes()).await;
    irq.mask_all_mac_interrupts();
    irq.publish_mac_interrupt_mask(MacInterruptMask::NONE);
    irq.set_mac_rx_interrupt_enable(MacRxLane::Success.set());
    let stale = irq.mac_interrupt_status();
    irq.acknowledge_mac_interrupts(stale.retain_rx_causes(MacRxLane::Success.set()));
    let before = access.receive_statistics_snapshot().primary;
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(WINDOW_MS))
        .ok_or(oer_portable_diagnostics::name!("b6-deadline"; 32))?;
    let mut counts = Counts {
        polls: 0,
        gap_max: 0,
        rises: 0,
        per_rise: [0; BUCKETS],
        per_rise_max: 0,
        steps: 0,
        per_step: [0; BUCKETS],
        units: 0,
        descriptors: 0,
        offset_set: 0,
        sosf_set: 0,
        ppdus: 0,
        per_ppdu: [0; BUCKETS],
        ppdu_span_max: 0,
        ppdu_span: [0; BUCKETS],
        rises_inside: 0,
        buffer_full: 0,
    };
    let base = ring.descriptor_base();
    // The cycle each descriptor was seen completed, by LAST's steps.
    let mut completed_at = [0_u32; COUNT];
    let mut last = cursor_ring_index(access.rx_dma().last_descriptor_low(), base, COUNT);
    let mut at_last_rise = last;
    let mut previous_poll = oer_portable_riscv::cycle32();
    let mut rise_cycles = [0_u32; 64];
    let mut rise_count = 0_usize;
    // The PPDU being gathered: its timestamp, its units, its first and last
    // completion cycles.
    let mut ppdu: Option<(u32, u32, u32, u32)> = None;
    while EmbassyClock.now() < end {
        for _ in 0..SLICE {
            let now = oer_portable_riscv::cycle32();
            counts.gap_max = counts.gap_max.max(now.wrapping_sub(previous_poll));
            previous_poll = now;
            counts.polls = counts.polls.saturating_add(1);
            let status = irq.mac_interrupt_status();
            let risen = status.rx_causes().includes(MacRxLane::Success.set());
            let seen = cursor_ring_index(access.rx_dma().last_descriptor_low(), base, COUNT);
            if seen != last {
                if let (Some(from), Some(to)) = (last, seen) {
                    let advanced = (to + COUNT - from) % COUNT;
                    bump(&mut counts.steps);
                    bucket(&mut counts.per_step, advanced as u32);
                    for step in 1..=advanced {
                        if let Some(cell) = completed_at.get_mut((from + step) % COUNT) {
                            *cell = now;
                        }
                    }
                }
                last = seen;
            }
            if risen {
                irq.acknowledge_mac_interrupts(status.retain_rx_causes(MacRxLane::Success.set()));
                bump(&mut counts.rises);
                if let (Some(from), Some(to)) = (at_last_rise, last) {
                    let advanced = ((to + COUNT - from) % COUNT) as u32;
                    bucket(&mut counts.per_rise, advanced);
                    counts.per_rise_max = counts.per_rise_max.max(advanced);
                }
                at_last_rise = last;
                if let Some(cell) = rise_cycles.get_mut(rise_count % 64) {
                    *cell = now;
                }
                rise_count = rise_count.wrapping_add(1);
            }
            // Take one completed unit and recycle it.
            storage
                .observe_reserved(ring, access.rx_dma())
                .map_err(|_| oer_portable_diagnostics::name!("b6-reserved"; 32))?;
            ring.complete_pending_reload(
                access.rx_dma(),
                Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
            )
            .map_err(|_| oer_portable_diagnostics::name!("b6-reload"; 32))?;
            let frozen = ring
                .freeze_cursor(access.rx_dma())
                .map_err(|_| oer_portable_diagnostics::name!("b6-cursor"; 32))?;
            let Some(unit) = storage
                .take_completed_unit(ring, frozen)
                .map_err(|_| oer_portable_diagnostics::name!("b6-take"; 32))?
            else {
                continue;
            };
            let count = unit.descriptor_count();
            bump(&mut counts.units);
            counts.descriptors = counts.descriptors.saturating_add(count as u32);
            let mut stamp = None;
            let mut done = 0_u32;
            for step in 0..count {
                let step = unit
                    .step(step)
                    .map_err(|_| oer_portable_diagnostics::name!("b6-step"; 32))?;
                let word = step.descriptor_word0();
                counts.offset_set = counts.offset_set.saturating_add(u32::from(word.offset()));
                counts.sosf_set = counts.sosf_set.saturating_add(u32::from(word.sosf()));
                done = completed_at
                    .get(step.descriptor_index())
                    .copied()
                    .unwrap_or(0);
                if stamp.is_none() && step.bytes().len() >= HEADER {
                    stamp = step
                        .bytes()
                        .first_chunk::<{ WifiRxControlPrefix::SIZE }>()
                        .map(|prefix| WifiRxControlPrefix::new(prefix).timestamp().value());
                }
            }
            let credit = unit
                .retain_discard()
                .ok_or(oer_portable_diagnostics::name!("b6-retain"; 32))?;
            let _appended = credit
                .append(access.rx_dma())
                .map_err(|_| oer_portable_diagnostics::name!("b6-append"; 32))?;
            // Gather the PPDU by the timestamp.
            let stamp = stamp.unwrap_or(0);
            ppdu = match ppdu {
                Some((time, units, first, _)) if time == stamp => {
                    Some((time, units.saturating_add(1), first, done))
                }
                previous => {
                    if let Some((_, units, first, last_done)) = previous {
                        close(
                            &mut counts,
                            units,
                            first,
                            last_done,
                            &rise_cycles,
                            rise_count,
                        );
                    }
                    Some((stamp, 1, done, done))
                }
            };
        }
        // The executor's other tasks (the watchdog feed, the console).
        yield_once().await;
        previous_poll = oer_portable_riscv::cycle32();
    }
    if let Some((_, units, first, last_done)) = ppdu {
        close(
            &mut counts,
            units,
            first,
            last_done,
            &rise_cycles,
            rise_count,
        );
    }
    let after = access.receive_statistics_snapshot().primary;
    counts.buffer_full = u32::from(after.buffer_full.wrapping_sub(before.buffer_full));
    print(
        console,
        &mut line,
        &counts,
        &after.wrapping_delta_since(before),
    )
    .await;
    Ok(())
}

/// Close one PPDU of `units` completed between cycles `first` and `last`,
/// counting the rises among the last 64 that fell inside it.
fn close(
    counts: &mut Counts,
    units: u32,
    first: u32,
    last: u32,
    rises: &[u32; 64],
    rise_count: usize,
) {
    bump(&mut counts.ppdus);
    bucket(&mut counts.per_ppdu, units);
    let span = last.wrapping_sub(first);
    counts.ppdu_span_max = counts.ppdu_span_max.max(span);
    // Spans in buckets of 10 µs at the CPU's 320 cycles a µs.
    bucket(&mut counts.ppdu_span, span / 3_200);
    if units > 1 {
        let recorded = rise_count.min(64);
        let inside = rises
            .iter()
            .take(recorded)
            .filter(|rise| rise.wrapping_sub(first) <= span && **rise != first)
            .count();
        counts.rises_inside = counts.rises_inside.saturating_add(inside as u32);
    }
}

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

async fn print(
    console: &mut usb::Console,
    line: &mut heapless::String<512>,
    counts: &Counts,
    mac: &oer_chips_esp32s31_hal::radio::ieee80211::registers::MacRxPrimaryStatisticsDelta,
) {
    line.clear();
    let _ = writeln!(
        line,
        "B6 end polls={} gap_max_cycles={} rises={} steps={} units={} descriptors={} ppdus={} offset_set={} sosf_set={} per_rise_max={} ppdu_span_max_cycles={} rises_inside_ppdus={}",
        counts.polls,
        counts.gap_max,
        counts.rises,
        counts.steps,
        counts.units,
        counts.descriptors,
        counts.ppdus,
        counts.offset_set,
        counts.sosf_set,
        counts.per_rise_max,
        counts.ppdu_span_max,
        counts.rises_inside,
    );
    usb::text(console, line.as_bytes()).await;
    for (name, histogram) in [
        ("per_rise", &counts.per_rise),
        ("per_step", &counts.per_step),
        ("per_ppdu", &counts.per_ppdu),
        ("ppdu_span_10us", &counts.ppdu_span),
    ] {
        line.clear();
        let _ = writeln!(line, "B6 {name} {histogram:?}");
        usb::text(console, line.as_bytes()).await;
    }
    line.clear();
    let _ = writeln!(
        line,
        "B6 mac mpdu={} fcs_error={} buffer_full={} data_success={}",
        mac.mpdu_count, mac.fcs_error, mac.buffer_full, mac.data_success
    );
    usb::text(console, line.as_bytes()).await;
}
