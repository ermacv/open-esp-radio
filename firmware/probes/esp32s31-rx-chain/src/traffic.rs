//! Selections200–209: one finite publication with real take/recycle and saved clears.
use super::{COUNT, NODES, Storage, WORDS, node, read_word, usb};
use core::fmt::Write as _;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    ieee80211::{
        dma::rx_ring::RxRingLive,
        registers::{MacInterruptMask, MacRxInterrupts},
    },
    owner::{MacInterruptRegisters, RadioAccess},
};
use oer_esp32s31_ieee80211_driver::rx::receive::RELOAD_SETTLE_LIMIT;
use oer_memory::zeroed::ZeroedStatic;
use oer_time::{Clock, Duration, settle::Settle};
#[path = "traffic/journal.rs"]
mod journal;
use journal::{Journal, UNIT_CAPACITY, WIDTH};
pub(super) type Trace = Journal<NODES, WORDS>;
static TRACE: ZeroedStatic<Trace> = ZeroedStatic::new();

pub(super) fn take() -> Result<&'static mut Trace, oer_diagnostics::Name<32>> {
    TRACE
        .try_take()
        .ok_or(oer_diagnostics::name!("traffic-claimed"; 32))
}

fn bank(irq: &MacInterruptRegisters) -> Result<[u32; WIDTH], oer_diagnostics::Name<32>> {
    let begin = oer_riscv::cycle32();
    let raw = irq.mac_interrupt_raw();
    let raw_end = oer_riscv::cycle32();
    if raw.fatal_assertion_raw().is_some() {
        return Err(oer_diagnostics::name!("traffic-fatal-raw"; 32));
    }
    let st = irq.mac_interrupt_status();
    let st_end = oer_riscv::cycle32();
    let rx = irq.mac_rx_interrupt_enable();
    let opaque = irq.mac_opaque_interrupt_enable();
    let mut row = [0; WIDTH];
    *row.get_mut(2).ok_or(oer_diagnostics::name!("row"; 32))? = begin;
    *row.get_mut(4).ok_or(oer_diagnostics::name!("row"; 32))? = raw.bits();
    *row.get_mut(5).ok_or(oer_diagnostics::name!("row"; 32))? = raw_end;
    *row.get_mut(6).ok_or(oer_diagnostics::name!("row"; 32))? = st.bits();
    *row.get_mut(7).ok_or(oer_diagnostics::name!("row"; 32))? = st_end;
    // Compact software output codes, not peripheral masks.
    *row.get_mut(12).ok_or(oer_diagnostics::name!("row"; 32))? =
        u32::from(rx.auxiliary_5) + 2 * u32::from(rx.success) + 4 * u32::from(rx.auxiliary_24);
    *row.get_mut(13).ok_or(oer_diagnostics::name!("row"; 32))? = u32::from(opaque.lane_6)
        + 2 * u32::from(opaque.lane_12)
        + 4 * u32::from(opaque.lane_13)
        + 8 * u32::from(opaque.lane_23);
    *row.get_mut(28).ok_or(oer_diagnostics::name!("row"; 32))? = oer_riscv::cycle32();
    Ok(row)
}

pub(super) fn checkpoint(
    trace: &mut Trace,
    phase: u32,
    step: u32,
    irq: &MacInterruptRegisters,
) -> Result<(), oer_diagnostics::Name<32>> {
    let mut row = bank(irq)?;
    *row.get_mut(29).ok_or(oer_diagnostics::name!("row"; 32))? = step;
    trace.record(row, phase, true)
}

fn scan(
    trace: &mut Trace,
    phase: u32,
    storage: &Storage,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    irq: &mut MacInterruptRegisters,
) -> Result<(), oer_diagnostics::Name<32>> {
    trace.admit_live()?;
    let begin = oer_riscv::cycle32();
    // Value-only observation of the permanent DMA allocation, before either RAW/ST.
    for (i, words) in trace.pre.iter_mut().enumerate() {
        let d = node(storage, i).ok_or(oer_diagnostics::name!("traffic-node"; 32))?;
        let binding = *trace
            .bindings
            .get(i)
            .ok_or(oer_diagnostics::name!("traffic-binding"; 32))?;
        for (w, out) in words.iter_mut().enumerate() {
            *trace
                .previous_pre
                .get_mut(i)
                .and_then(|words| words.get_mut(w))
                .ok_or(oer_diagnostics::name!("traffic-previous-word"; 32))? = *out;
            *out = read_word(storage, d, binding, w * 4)?;
        }
    }
    let copy_end = oer_riscv::cycle32();
    let raw = irq.mac_interrupt_raw();
    let raw_end = oer_riscv::cycle32();
    if raw.fatal_assertion_raw().is_some() {
        return Err(oer_diagnostics::name!("traffic-fatal-live"; 32));
    }
    let status = irq.mac_interrupt_status();
    let status_end = oer_riscv::cycle32();
    let post_begin = oer_riscv::cycle32();
    for (i, words) in trace.post.iter_mut().enumerate() {
        let d = node(storage, i).ok_or(oer_diagnostics::name!("traffic-post-node"; 32))?;
        let binding = *trace
            .bindings
            .get(i)
            .ok_or(oer_diagnostics::name!("traffic-binding"; 32))?;
        for (w, out) in words.iter_mut().enumerate() {
            *out = read_word(storage, d, binding, w * 4)?;
        }
    }
    let post_end = oer_riscv::cycle32();
    let cursor = access.receive_dma_snapshot();
    let mut done = 0u32;
    for (i, d) in storage.descriptors().iter().enumerate() {
        if d.word0().rx_done() {
            done |= 1u32
                .checked_shl(i as u32)
                .ok_or(oer_diagnostics::name!("done-index"; 32))?;
        }
    }
    let mut row = bank(irq)?;
    *row.get_mut(2).ok_or(oer_diagnostics::name!("row"; 32))? = begin;
    *row.get_mut(3).ok_or(oer_diagnostics::name!("row"; 32))? = copy_end;
    *row.get_mut(4).ok_or(oer_diagnostics::name!("row"; 32))? = raw.bits();
    *row.get_mut(5).ok_or(oer_diagnostics::name!("row"; 32))? = raw_end;
    *row.get_mut(6).ok_or(oer_diagnostics::name!("row"; 32))? = status.bits();
    *row.get_mut(7).ok_or(oer_diagnostics::name!("row"; 32))? = status_end;
    *row.get_mut(8).ok_or(oer_diagnostics::name!("row"; 32))? = cursor.last_descriptor_low;
    *row.get_mut(9).ok_or(oer_diagnostics::name!("row"; 32))? = cursor.next_descriptor_low;
    *row.get_mut(10).ok_or(oer_diagnostics::name!("row"; 32))? = done;
    *row.get_mut(11).ok_or(oer_diagnostics::name!("row"; 32))? =
        u32::from(storage.reserved_descriptor().word0().rx_done());
    let stats = access.receive_statistics_snapshot().primary;
    for (column, value) in [
        (14, stats.mpdu_count),
        (15, stats.end),
        (16, stats.fcs_error),
        (17, stats.buffer_full),
        (18, stats.data_success),
    ] {
        *row.get_mut(column)
            .ok_or(oer_diagnostics::name!("row"; 32))? = u32::from(value);
    }
    storage
        .observe_reserved(ring, access.rx_dma())
        .map_err(|_| oer_diagnostics::name!("traffic-reserved"; 32))?;
    ring.complete_pending_reload(
        access.rx_dma(),
        Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
    )
    .map_err(|_| oer_diagnostics::name!("traffic-reload"; 32))?;
    {
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| oer_diagnostics::name!("traffic-cursor"; 32))?;
        let take_begin = oer_riscv::cycle32();
        if let Some(unit) = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| oer_diagnostics::name!("traffic-take"; 32))?
        {
            let result = (|| {
                let saved = trace
                    .units
                    .get_mut(trace.unit_used)
                    .ok_or(oer_diagnostics::name!("traffic-unit-full"; 32))?;
                let head = unit.head_index();
                let count = unit.descriptor_count();
                let length = unit.total_length();
                let reserved = unit.metadata().continues_reserved_landing();
                let mut offset = 0usize;
                let mut mismatch = 0u32;
                let mut previous_mismatch = 0u32;
                let mut post_mismatch = 0u32;
                let mut unstable = 0u32;
                for step in 0..count {
                    let index = head.wrapping_add(step) % COUNT;
                    let early = trace
                        .pre
                        .get(index)
                        .ok_or(oer_diagnostics::name!("traffic-pre"; 32))?;
                    let previous = trace
                        .previous_pre
                        .get(index)
                        .ok_or(oer_diagnostics::name!("traffic-previous"; 32))?;
                    let post = trace
                        .post
                        .get(index)
                        .ok_or(oer_diagnostics::name!("traffic-post"; 32))?;
                    let bytes = unit
                        .segment(step)
                        .ok_or(oer_diagnostics::name!("traffic-segment"; 32))?;
                    for (i, byte) in bytes.iter().copied().enumerate() {
                        let observed = early
                            .get(i / 4)
                            .ok_or(oer_diagnostics::name!("traffic-word"; 32))?
                            .to_le_bytes();
                        let before = *observed
                            .get(i % 4)
                            .ok_or(oer_diagnostics::name!("traffic-byte"; 32))?;
                        let old = previous
                            .get(i / 4)
                            .ok_or(oer_diagnostics::name!("traffic-previous-word"; 32))?
                            .to_le_bytes();
                        let old = *old
                            .get(i % 4)
                            .ok_or(oer_diagnostics::name!("traffic-previous-byte"; 32))?;
                        let after = post
                            .get(i / 4)
                            .ok_or(oer_diagnostics::name!("traffic-post-word"; 32))?
                            .to_le_bytes();
                        let after = *after
                            .get(i % 4)
                            .ok_or(oer_diagnostics::name!("traffic-post-byte"; 32))?;
                        *saved
                            .after_irq
                            .get_mut(offset)
                            .ok_or(oer_diagnostics::name!("traffic-length"; 32))? = after;
                        *saved
                            .previous
                            .get_mut(offset)
                            .ok_or(oer_diagnostics::name!("traffic-length"; 32))? = old;
                        *saved
                            .early
                            .get_mut(offset)
                            .ok_or(oer_diagnostics::name!("traffic-length"; 32))? = before;
                        *saved
                            .taken
                            .get_mut(offset)
                            .ok_or(oer_diagnostics::name!("traffic-length"; 32))? = byte;
                        mismatch = mismatch.saturating_add(u32::from(before != byte));
                        previous_mismatch =
                            previous_mismatch.saturating_add(u32::from(old != byte));
                        post_mismatch = post_mismatch.saturating_add(u32::from(after != byte));
                        offset = offset
                            .checked_add(1)
                            .ok_or(oer_diagnostics::name!("traffic-offset"; 32))?;
                    }
                }
                if offset != length {
                    return Err(oer_diagnostics::name!("traffic-unit-length"; 32));
                }
                let mut offset = 0usize;
                for step in 0..count {
                    for byte in unit
                        .segment(step)
                        .ok_or(oer_diagnostics::name!("traffic-segment-reread"; 32))?
                    {
                        unstable = unstable
                            .saturating_add(u32::from(saved.taken.get(offset) != Some(byte)));
                        offset = offset
                            .checked_add(1)
                            .ok_or(oer_diagnostics::name!("traffic-offset"; 32))?;
                    }
                }
                let take_end = oer_riscv::cycle32();
                saved.header = [
                    trace.scans,
                    phase,
                    raw.bits(),
                    status.bits(),
                    head as u32,
                    count as u32,
                    length as u32,
                    u32::from(reserved),
                    begin,
                    copy_end,
                    raw_end,
                    status_end,
                    take_begin,
                    take_end,
                    mismatch,
                    unstable,
                    trace.scans.saturating_sub(1),
                    trace.previous_begin,
                    trace.previous_end,
                    previous_mismatch,
                    post_begin,
                    post_end,
                    post_mismatch,
                ];
                trace.unit_used = trace.unit_used.saturating_add(1);
                for (column, value) in [
                    (19, head as u32 + 1),
                    (20, count as u32),
                    (21, length as u32),
                    (22, u32::from(reserved)),
                    (23, mismatch),
                    (24, take_end),
                ] {
                    *row.get_mut(column)
                        .ok_or(oer_diagnostics::name!("row"; 32))? = value;
                }
                Ok(())
            })();
            // On a recording failure retain ownership until the normal stopped
            // diagnostic path; never accidentally run Drop's reset-required edge.
            if let Err(error) = result {
                unit.retain_for_deferred_recycle();
                return Err(error);
            }
            let credit = unit
                .retain_discard()
                .ok_or(oer_diagnostics::name!("traffic-recycle-frontier"; 32))?;
            let _appended = credit
                .append(access.rx_dma())
                .map_err(|_| oer_diagnostics::name!("traffic-recycle"; 32))?;
            *row.get_mut(25).ok_or(oer_diagnostics::name!("row"; 32))? = 1;
        }
        ring.complete_pending_reload(
            access.rx_dma(),
            Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
        )
        .map_err(|_| oer_diagnostics::name!("traffic-recycle-settle"; 32))?;
    }
    // This exact sample is the logged pre-ack STATUS, closing HW20's sampling gap.
    if phase == 203 {
        irq.acknowledge_mac_interrupts(status.retain_rx_causes(MacRxInterrupts::ALL));
    }
    *row.get_mut(26).ok_or(oer_diagnostics::name!("row"; 32))? = irq.mac_interrupt_raw().bits();
    *row.get_mut(27).ok_or(oer_diagnostics::name!("row"; 32))? = irq.mac_interrupt_status().bits();
    *row.get_mut(28).ok_or(oer_diagnostics::name!("row"; 32))? = oer_riscv::cycle32();
    trace.previous_begin = begin;
    trace.previous_end = copy_end;
    trace.record(row, phase, false)
}

pub(super) fn collect(
    trace: &mut Trace,
    storage: &Storage,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    irq: &mut MacInterruptRegisters,
) -> Result<(), oer_diagnostics::Name<32>> {
    for (index, binding) in trace.bindings.iter_mut().enumerate() {
        *binding = node(storage, index)
            .ok_or(oer_diagnostics::name!("traffic-binding"; 32))?
            .buffer_address();
    }
    let start = EmbassyClock.now();
    let mask = start
        .checked_add(Duration::from_millis(350))
        .ok_or(oer_diagnostics::name!("traffic-deadline"; 32))?;
    let end = start
        .checked_add(Duration::from_millis(700))
        .ok_or(oer_diagnostics::name!("traffic-deadline"; 32))?;
    irq.publish_mac_interrupt_mask(MacInterruptMask::STATION);
    access.wifi_mac_hal().start_mac_runtime(ring);
    let mut masked = false;
    while EmbassyClock.now() < end && trace.unit_used < UNIT_CAPACITY {
        if !masked && EmbassyClock.now() >= mask {
            irq.mask_all_mac_interrupts();
            masked = true;
        }
        scan(
            trace,
            if masked { 204 } else { 203 },
            storage,
            ring,
            access,
            irq,
        )?;
    }
    Ok(())
}

pub(super) fn clears(
    trace: &mut Trace,
    irq: &mut MacInterruptRegisters,
) -> Result<(), oer_diagnostics::Name<32>> {
    irq.mask_all_mac_interrupts();
    checkpoint(trace, 205, 0, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::ALL);
    checkpoint(trace, 206, 0, irq)?;
    let held5 = irq
        .mac_interrupt_status()
        .retain_rx_causes(MacRxInterrupts::AUXILIARY_5);
    let held14 = irq
        .mac_interrupt_status()
        .retain_rx_causes(MacRxInterrupts::SUCCESS);
    let held24 = irq
        .mac_interrupt_status()
        .retain_rx_causes(MacRxInterrupts::AUXILIARY_24);
    // Emit each exact clear authority before consuming it, including zero samples.
    for (step, bits) in [(5, held5.bits()), (14, held14.bits()), (24, held24.bits())] {
        let mut row = bank(irq)?;
        *row.get_mut(29).ok_or(oer_diagnostics::name!("row"; 32))? = step;
        *row.get_mut(30).ok_or(oer_diagnostics::name!("row"; 32))? = bits;
        trace.record(row, 206, true)?;
    }
    irq.mask_all_mac_interrupts();
    checkpoint(trace, 207, 0, irq)?;
    irq.acknowledge_mac_interrupts(held5);
    checkpoint(trace, 207, 1, irq)?;
    irq.acknowledge_mac_interrupts(held14);
    checkpoint(trace, 208, 1, irq)?;
    irq.acknowledge_mac_interrupts(held24);
    checkpoint(trace, 209, 1, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::ALL);
    checkpoint(trace, 209, 2, irq)?;
    irq.mask_all_mac_interrupts();
    checkpoint(trace, 209, 3, irq)
}

pub(super) async fn dump(
    trace: &Trace,
    console: &mut usb::Console,
) -> Result<(), oer_diagnostics::Name<32>> {
    let mut line = heapless::String::<768>::new();
    writeln!(
        &mut line,
        "TRAFFIC end rows={} scans={} units={} cap_ms=700",
        trace.used, trace.scans, trace.unit_used
    )
    .map_err(|_| oer_diagnostics::name!("format"; 32))?;
    usb::text(console, line.as_bytes()).await;
    for row in trace.rows.iter().take(trace.used) {
        line.clear();
        line.push_str("M")
            .map_err(|_| oer_diagnostics::name!("format"; 32))?;
        for v in row {
            write!(&mut line, " {v:x}").map_err(|_| oer_diagnostics::name!("format"; 32))?;
        }
        line.push('\n')
            .map_err(|_| oer_diagnostics::name!("format"; 32))?;
        usb::text(console, line.as_bytes()).await;
    }
    for (id, unit) in trace.units.iter().take(trace.unit_used).enumerate() {
        line.clear();
        write!(&mut line, "T {id:x}").map_err(|_| oer_diagnostics::name!("format"; 32))?;
        for v in unit.header {
            write!(&mut line, " {v:x}").map_err(|_| oer_diagnostics::name!("format"; 32))?;
        }
        line.push('\n')
            .map_err(|_| oer_diagnostics::name!("format"; 32))?;
        usb::text(console, line.as_bytes()).await;
        let length = *unit
            .header
            .get(6)
            .ok_or(oer_diagnostics::name!("header"; 32))? as usize;
        for offset in (0..length).step_by(16) {
            line.clear();
            write!(&mut line, "P {id:x} {offset:x}")
                .map_err(|_| oer_diagnostics::name!("format"; 32))?;
            for bytes in [&unit.previous, &unit.early, &unit.after_irq, &unit.taken] {
                for byte in bytes
                    .get(offset..offset.saturating_add(16).min(length))
                    .ok_or(oer_diagnostics::name!("dump-range"; 32))?
                {
                    write!(&mut line, " {byte:02x}")
                        .map_err(|_| oer_diagnostics::name!("format"; 32))?;
                }
            }
            line.push('\n')
                .map_err(|_| oer_diagnostics::name!("format"; 32))?;
            usb::text(console, line.as_bytes()).await;
        }
    }
    Ok(())
}
