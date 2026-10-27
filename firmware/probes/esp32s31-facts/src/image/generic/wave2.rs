//! Short ordered expiry/acknowledgement probes; no timing sweep.
use super::*;

pub(in crate::image) fn usb_setup(s: &mut Session<'_>) {
    super::super::contain(s);
    // First USB observation in this loader session, before endpoint changes.
    usb_state(s, 0x22500);
    begin(s, pac::Interrupt::USB_SERIAL_JTAG);
    usb_mask_all(s);
    s.generic.usb.modify_int_ena(|w| {
        w.set_serial_out_recv_pkt_int_ena(true)
            .set_serial_in_empty_int_ena(true);
    });
    let sample = s.generic.usb_irq.sample_usb_serial_jtag_interrupt();
    s.generic
        .usb_irq
        .acknowledge_usb_serial_jtag_interrupt(sample);
    usb_state(s, 0x22510);
    // Host sends one OUT byte while the hart is halted between 225 and 226.
}
pub(in crate::image) fn usb_packet(s: &mut Session<'_>) {
    feed(s);
    usb_state(s, 0x22600);
    admit(s, 0x22603);
    let sample = s.generic.usb_irq.sample_usb_serial_jtag_interrupt();
    usb_mask_all(s);
    usb_state(s, 0x22610);
    s.generic
        .usb_irq
        .acknowledge_usb_serial_jtag_interrupt(sample);
    oer_riscv::fence();
    usb_state(s, 0x22620);
    wait_window();
    usb_state(s, 0x22630); // OUT FIFO deliberately still contains the byte.
    s.generic.usb.modify_int_ena(|w| {
        w.set_serial_out_recv_pkt_int_ena(true)
            .set_serial_in_empty_int_ena(true);
    });
    usb_state(s, 0x22640);
    // Consume at most one received byte, then publish exactly one IN byte.
    if s.generic.usb.read_ep1_conf().serial_out_ep_data_avail() {
        emit(0x22650, u32::from(s.generic.usb.pop_ep1()), 0, 0);
    }
    usb_state(s, 0x22660);
    if usb_empty(s, 0x22670) {
        usb_state(s, 0x22680);
        let sample = s.generic.usb_irq.sample_usb_serial_jtag_interrupt();
        s.generic
            .usb_irq
            .acknowledge_usb_serial_jtag_interrupt(sample);
        wait_window();
        usb_state(s, 0x22690);
        // A second consumed publication tests a new IN_EMPTY after CLR.
        if usb_empty(s, 0x226a0) {
            usb_state(s, 0x226b0);
        }
    }
    // SOURCE: HAL UsbEndpoint::enable_events preserves unrelated enables.
    // Emulate an inherited SOF enable explicitly; this is not a boot claim.
    s.generic.usb.modify_int_ena(|w| {
        w.set_sof_int_ena(true);
    });
    s.generic.usb.modify_int_ena(|w| {
        w.set_serial_out_recv_pkt_int_ena(true)
            .set_serial_in_empty_int_ena(true);
    });
    usb_state(s, 0x226c0);
    let sample = s.generic.usb_irq.sample_usb_serial_jtag_interrupt();
    s.generic
        .usb_irq
        .acknowledge_usb_serial_jtag_interrupt(sample);
    wait_window();
    usb_state(s, 0x226d0);
    admit(s, 0x226d3);
    admit(s, 0x226d4);
    usb_mask_all(s);
    route(s, pac::Interrupt::USB_SERIAL_JTAG, false);
}

fn init(s: &mut Session<'_>) {
    begin(s, pac::Interrupt::SYSTIMER_TARGET0);
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_apb_clk_en(true);
    });
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(true);
    });
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(false)
            .set_reg_systimer_clk_en(true);
    });
    s.generic.systimer.systimer.modify_conf(|w| {
        w.set_timer_unit0_work_en(true)
            .set_timer_unit0_core0_stall_en(false)
            .set_timer_unit0_core1_stall_en(false);
    });
    for n in 0..2 {
        let Ok(i) = Index::new(n) else { return };
        s.generic.systimer.systimer.modify_target_conf(i, |w| {
            w.set_period_mode(false).set_timer_unit_sel(false);
        });
        sy_work(s, n, false);
        sy_ena(s, n, true);
        sy_clear(s, n);
    }
}
fn wait(s: &Session<'_>, n: usize, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    while !sy_raw(s, n) && oer_riscv::cycle32().wrapping_sub(start) < LIMIT {}
    let ready = sy_raw(s, n);
    emit(
        tag,
        u32::from(ready),
        oer_riscv::cycle32().wrapping_sub(start),
        0,
    );
    ready
}
fn count(s: &Session<'_>, tag: u32) -> Option<u64> {
    let n = sy_now(s);
    if let Some(n) = n {
        emit(tag, n as u32, (n >> 32) as u32, 0);
    } else {
        emit(tag, 0, 0, 1);
    }
    n
}
fn stop(s: &mut Session<'_>) {
    for n in 0..2 {
        sy_work(s, n, false);
        sy_ena(s, n, false);
        sy_clear(s, n);
    }
    route(s, pac::Interrupt::SYSTIMER_TARGET0, false);
}
pub(in crate::image) fn expiry(s: &mut Session<'_>) {
    init(s);
    if let Some(now) = count(s, 0x22700) {
        // CLR before the fresh expiry; zero RAW witnesses a future arm.
        sy_program(s, 0, now.wrapping_add(16_000));
        sy_clear(s, 0);
        sy_state(s, 0x22710);
        if wait(s, 0, 0x22720) {
            sy_state(s, 0x22730);
            admit(s, 0x22733);
            // An acknowledgement after an observed expiry consumes it.
            sy_clear(s, 0);
            wait_window();
            sy_state(s, 0x22740);
            admit(s, 0x22743);
        }
    }
    // CLR/rearm a new future target without stopping the counter.
    if let Some(now) = count(s, 0x22750) {
        sy_clear(s, 0);
        sy_program(s, 0, now.wrapping_add(16_000));
        sy_state(s, 0x22760);
        if wait(s, 0, 0x22770) {
            sy_state(s, 0x22780);
            admit(s, 0x22783);
        }
    }
    stop(s);
}
pub(in crate::image) fn simultaneous(s: &mut Session<'_>) {
    init(s);
    if let Some(now) = count(s, 0x22800) {
        let target = now.wrapping_add(16_000);
        sy_program(s, 0, target);
        sy_program(s, 1, target);
        sy_state(s, 0x22810);
        wait(s, 0, 0x22820);
        wait(s, 1, 0x22821);
        sy_state(s, 0x22830);
        sy_clear(s, 0);
        sy_state(s, 0x22840); // Target1 must remain pending.
        admit(s, 0x22843); // Target0 route must be quiet.
        sy_clear(s, 1);
        sy_state(s, 0x22850);
    }
    stop(s);
}
fn seed(s: &mut Session<'_>, value: u64) {
    let timer = &mut s.generic.systimer.systimer;
    timer.modify_conf(|w| {
        w.set_timer_unit0_work_en(false);
    });
    timer.modify_unit0_load_hi(|w| {
        w.set_timer_unit0_load_hi(Bits::truncate((value >> 32) as u32));
    });
    timer.modify_unit0_load_lo(|w| {
        w.set_timer_unit0_load_lo(value as u32);
    });
    timer.trigger_unit0_load_timer_unit0_load();
    oer_riscv::fence();
}
pub(in crate::image) fn wrap(s: &mut Session<'_>) {
    init(s);
    // Derived from the modeled counter high-field width, not a local MMIO mask.
    const MODULUS: u64 = 1u64 << (32 + pac::systimer::unit0_value_hi::TIMER_UNIT0_VALUE_HI_WIDTH);
    for (tag, boundary) in [(0x22900, 1u64 << 32), (0x22980, MODULUS)] {
        sy_work(s, 0, false);
        sy_clear(s, 0);
        seed(s, boundary - 16_000);
        count(s, tag);
        let target = if boundary == MODULUS {
            16_000
        } else {
            boundary + 16_000
        };
        sy_program(s, 0, target);
        sy_state(s, tag + 0x10); // Detect an early assertion before count wrap.
        sy_clear(s, 0); // Clear while seeded/stopped, before crossing the boundary.
        s.generic.systimer.systimer.modify_conf(|w| {
            w.set_timer_unit0_work_en(true);
        });
        let start = oer_riscv::cycle32();
        let mut crossed = false;
        while oer_riscv::cycle32().wrapping_sub(start) < LIMIT {
            let Some(now) = sy_now(s) else { break };
            if (boundary == MODULUS && (16_000..32_000).contains(&now))
                || (boundary != MODULUS && now >= target)
            {
                crossed = true;
                break;
            }
        }
        emit(
            tag + 0x18,
            u32::from(crossed),
            oer_riscv::cycle32().wrapping_sub(start),
            0,
        );
        wait(s, 0, tag + 0x20);
        count(s, tag + 0x30);
        sy_state(s, tag + 0x40);
        admit(s, tag + 0x43);
        sy_clear(s, 0);
        sy_state(s, tag + 0x50);
    }
    stop(s);
}
