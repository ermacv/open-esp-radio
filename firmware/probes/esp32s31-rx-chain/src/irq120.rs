//! One bounded source120 register/DMA visibility capture, with stopped W1C phases.
use super::{BUFFER, NODES, Storage, WORDS, node, read_word, usb};
use core::fmt::Write as _;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    ieee80211::registers::{MacInterruptEnableState, MacRxInterrupts},
    owner::{MacInterruptRegisters, RadioAccess},
};
use oer_memory::zeroed::ZeroedStatic;
use oer_time::{Clock, Duration};

#[path = "irq120/journal.rs"]
mod journal;
pub(super) use journal::Journal;
use journal::WIDTH;

const CAPTURE_MS: u32 = 700;
const MASK_AFTER_MS: u32 = 250;

oer_memory::zeroed_static! {
    static JOURNAL: ZeroedStatic<Journal> = zeroed in ".critical.bss.rx_irq120";
}

fn done(storage: &Storage) -> Result<[u32; 2], &'static str> {
    let mut bitmap = 0_u32;
    for (i, descriptor) in storage.descriptors().iter().enumerate() {
        if descriptor.word0().rx_done() {
            bitmap |= 1_u32.checked_shl(i as u32).ok_or("done-index")?;
        }
    }
    Ok([
        bitmap,
        u32::from(storage.reserved_descriptor().word0().rx_done()),
    ])
}

fn enables(value: MacRxInterrupts) -> u32 {
    // Software result encoding, not register geometry.
    u32::from(value.auxiliary_5) + 2 * u32::from(value.success) + 4 * u32::from(value.auxiliary_24)
}

fn observe(
    storage: &Storage,
    access: &RadioAccess,
    irq: &MacInterruptRegisters,
) -> Result<[u32; WIDTH], &'static str> {
    let begin = oer_riscv::cycle32();
    let pre = access.receive_dma_snapshot();
    let [done_pre, reserved_pre] = done(storage)?;
    let pre_end = oer_riscv::cycle32();
    let raw = irq.mac_interrupt_raw();
    let raw_end = oer_riscv::cycle32();
    if raw.fatal_assertion_raw().is_some() {
        return Err("fatal-rx-raw");
    }
    let status = irq.mac_interrupt_status();
    let status_end = oer_riscv::cycle32();
    let post = access.receive_dma_snapshot();
    let [done_post, reserved_post] = done(storage)?;
    let post_end = oer_riscv::cycle32();
    let raw_tail = irq.mac_interrupt_raw();
    let raw_tail_end = oer_riscv::cycle32();
    if raw_tail.fatal_assertion_raw().is_some() {
        return Err("fatal-rx-tail");
    }
    let tail = irq.mac_interrupt_status();
    let tail_end = oer_riscv::cycle32();
    let enable = enables(irq.mac_rx_interrupt_enable());
    let disabled = u32::from(irq.mac_interrupt_enable() == MacInterruptEnableState::Disabled);
    let stats = access.receive_statistics_snapshot();
    let p = stats.primary;
    let d = stats.decode_errors;
    Ok([
        0,
        0,
        begin,
        pre.descriptor_base,
        pre.last_descriptor_low,
        pre.next_descriptor_low,
        done_pre,
        reserved_pre,
        pre_end,
        raw.bits(),
        raw_end,
        status.bits(),
        status_end,
        post.last_descriptor_low,
        post.next_descriptor_low,
        done_post,
        reserved_post,
        post_end,
        raw_tail.bits(),
        raw_tail_end,
        tail.bits(),
        tail_end,
        enable,
        disabled,
        u32::from(p.mpdu_count),
        u32::from(p.end),
        u32::from(p.fcs_error),
        u32::from(p.abort),
        u32::from(p.abort_fcs_pass),
        u32::from(p.power_drop_error),
        u32::from(p.same_bm_error),
        u32::from(p.other_unicast),
        u32::from(p.last_unmatched_error),
        u32::from(p.buffer_full),
        u32::from(p.fifo_overflow),
        u32::from(d.nrx_abort),
        u32::from(d.nrx_restart),
        u32::from(d.nrx_ht_sig),
        u32::from(d.nrx_he_sig_a_crc),
        u32::from(p.data_success),
        oer_riscv::cycle32(),
        u32::from(pre.next_descriptor_upper_unknown),
        u32::from(post.next_descriptor_upper_unknown),
        0,
        0,
        0,
        0,
        0,
    ])
}

pub(super) fn take() -> Result<&'static mut Journal, &'static str> {
    JOURNAL.try_take().ok_or("irq120-journal-claimed")
}

pub(super) fn checkpoint(
    trace: &mut Journal,
    phase: u32,
    storage: &Storage,
    access: &RadioAccess,
    irq: &MacInterruptRegisters,
) -> Result<(), &'static str> {
    let mut row = observe(storage, access, irq)?;
    *row.get_mut(47).ok_or("checkpoint-end")? = oer_riscv::cycle32();
    trace.record(row, phase, true)
}

pub(super) fn collect(
    trace: &mut Journal,
    storage: &Storage,
    access: &RadioAccess,
    irq: &mut MacInterruptRegisters,
) -> Result<(), &'static str> {
    let start = EmbassyClock.now();
    let mask_at = start
        .checked_add(Duration::from_millis(MASK_AFTER_MS))
        .ok_or("mask-deadline")?;
    let end = start
        .checked_add(Duration::from_millis(CAPTURE_MS))
        .ok_or("capture-deadline")?;
    let mut masked = false;
    loop {
        // Preserve room for the stopped mask/unmask/selected-ack observations,
        // so a full live journal still gets dumped after stopping the producer.
        trace.check_live_budget()?;
        let now = EmbassyClock.now();
        if !masked && now >= mask_at {
            irq.mask_all_mac_interrupts();
            masked = true;
        }
        let mut row = observe(storage, access, irq)?;
        if !masked {
            let sample = irq
                .mac_interrupt_status()
                .retain_rx_causes(MacRxInterrupts::ALL);
            *row.get_mut(43).ok_or("ack-time")? = oer_riscv::cycle32();
            irq.acknowledge_mac_interrupts(sample);
        }
        let raw_after = irq.mac_interrupt_raw();
        *row.get_mut(44).ok_or("ack-raw")? = raw_after.bits();
        *row.get_mut(45).ok_or("ack-status")? = irq.mac_interrupt_status().bits();
        *row.get_mut(46).ok_or("ack-enable")? = enables(irq.mac_rx_interrupt_enable());
        *row.get_mut(47).ok_or("ack-end")? = oer_riscv::cycle32();
        trace.record(row, if masked { 161 } else { 160 }, false)?;
        if now >= end {
            break;
        }
    }
    Ok(())
}

pub(super) async fn finish(
    trace: &mut Journal,
    storage: &Storage,
    access: &RadioAccess,
    irq: &mut MacInterruptRegisters,
    console: &mut usb::Console,
) -> Result<(), &'static str> {
    // The outer MAC and descriptor walker have stopped; no source is republished.
    irq.mask_all_mac_interrupts();
    checkpoint(trace, 162, storage, access, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::ALL);
    checkpoint(trace, 163, storage, access, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::NONE);
    checkpoint(trace, 164, storage, access, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::ALL);
    checkpoint(trace, 165, storage, access, irq)?;
    // Retain a positive STATUS24 sample across the subsequent mask transition.
    let held24 = irq
        .mac_interrupt_status()
        .retain_rx_causes(MacRxInterrupts::AUXILIARY_24);
    let held5 = irq
        .mac_interrupt_status()
        .retain_rx_causes(MacRxInterrupts::AUXILIARY_5);
    irq.acknowledge_mac_interrupts(held5);
    checkpoint(trace, 166, storage, access, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::NONE);
    irq.acknowledge_mac_interrupts(held24);
    checkpoint(trace, 167, storage, access, irq)?;
    irq.set_mac_rx_interrupt_enable(MacRxInterrupts::ALL);
    let success = irq
        .mac_interrupt_status()
        .retain_rx_causes(MacRxInterrupts::SUCCESS);
    irq.acknowledge_mac_interrupts(success);
    checkpoint(trace, 168, storage, access, irq)?;
    irq.mask_all_mac_interrupts();
    checkpoint(trace, 169, storage, access, irq)?;
    let mut line = heapless::String::<768>::new();
    writeln!(
        &mut line,
        "IRQ120 end records={} scans={} max_gap={} cap_ms={} mask_ms={} width={} journal={:x}",
        trace.used,
        trace.scans,
        trace.max_gap,
        CAPTURE_MS,
        MASK_AFTER_MS,
        WIDTH,
        core::ptr::from_ref(trace).addr()
    )
    .map_err(|_| "format")?;
    usb::text(console, line.as_bytes()).await;
    for row in trace.rows.iter().take(trace.used) {
        line.clear();
        line.push_str("I").map_err(|_| "format")?;
        for value in row {
            write!(&mut line, " {value:x}").map_err(|_| "format")?;
        }
        line.push('\n').map_err(|_| "format")?;
        usb::text(console, line.as_bytes()).await;
    }
    // Stopped controls/links and complete retained bytes establish which DONE
    // nodes contain accepted frames. These copies do not prove payload bus order.
    for index in 0..NODES {
        let descriptor = node(storage, index).ok_or("node")?;
        let address = descriptor.buffer_address();
        line.clear();
        write!(
            &mut line,
            "N {index:x} {:x} {:x} {:x}",
            descriptor.word0().bits(),
            address,
            descriptor.next_address()
        )
        .map_err(|_| "format")?;
        for offset in (0..BUFFER).step_by(4).take(WORDS) {
            write!(
                &mut line,
                " {:08x}",
                read_word(storage, descriptor, address, offset)?
            )
            .map_err(|_| "format")?;
        }
        line.push('\n').map_err(|_| "format")?;
        usb::text(console, line.as_bytes()).await;
    }
    Ok(())
}
