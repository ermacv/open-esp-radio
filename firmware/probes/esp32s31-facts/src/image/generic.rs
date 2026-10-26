//! Bounded generic IRQ facts. No RF, DMA, group reset or watchdog programming.
use super::{
    CONTROLS, CPU_LINE, IRQ_COUNT, LINE, SOFTWARE_SOURCE, Session, emit, line, setup, wait_window,
};
use core::{
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use oer_esp32s31_pac as pac;
use oer_mmio::{Bits, Index};

pub(super) struct Hardware {
    pub usb: pac::usb_serial_jtag::UsbSerialJtagTask,
    pub usb_irq: pac::usb_serial_jtag::UsbSerialJtagIrq,
    pub systimer: pac::ownership::SystemTimerPeripherals,
    pub systimer_clock: pac::hp_sys_clkrst::HpSysClkrstSystimer,
    pub timer: pac::ownership::TimerGroup0Peripherals,
    pub watchdog: pac::timg::Timg0Wdt,
    pub watchdog_clock: pac::hp_sys_clkrst::HpSysClkrstTimg0,
}
// Only the masked thread publishes/revokes this exclusive handler loan.
static CLIC: AtomicPtr<pac::clic::Clic> = AtomicPtr::new(ptr::null_mut());
const LIMIT: u32 = 2_000_000;

pub(super) fn interrupt() -> bool {
    let clic = CLIC.load(Ordering::Acquire);
    if clic.is_null() {
        return false;
    }
    // SAFETY: admit lends the live CLIC owner only while the masked thread
    // enables IRQs and waits without touching that owner. Hart1 is contained,
    // the handler is non-reentrant, and the loan ends after MIE is cleared.
    unsafe { &mut *clic }.modify_ctrl(CPU_LINE, |w| {
        w.set_ie(false);
    });
    true
}
fn window(cycles: u32) {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < cycles {}
}
fn pack<const N: usize>(values: [bool; N]) -> u32 {
    const {
        assert!(N <= u32::BITS as usize);
    }
    values
        .into_iter()
        .enumerate()
        .fold(0, |v, (i, b)| v | (u32::from(b) << i))
}
fn feed(s: &mut Session<'_>) {
    let wdt = &mut s.ble.watchdog;
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    wdt.trigger_feed_feed();
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
}
fn route(s: &mut Session<'_>, source: pac::Interrupt, bound: bool) {
    if let Ok(i) = Index::new(source as usize) {
        s.interrupt_matrix.interrupt_core0.modify_route(i, |w| {
            if bound {
                w.set_map(Bits::constant::<{ LINE as u32 }>());
            } else {
                w.set_map(Bits::constant::<0>());
            }
        });
    }
    oer_riscv::fence();
}
fn matrix_state(s: &Session<'_>, tag: u32) {
    emit(
        tag + 0x10000,
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_2()
            .int_status_2(),
        u32::from(
            s.source
                .read_hp_cpu_int_from_cpu_2()
                .hp_cpu_int_from_cpu_2(),
        ),
        s.interrupt_matrix
            .interrupt_core0
            .read_route(SOFTWARE_SOURCE)
            .pass_level()
            .get(),
    );
    emit(
        tag,
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_0()
            .int_status_0(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_1()
            .int_status_1(),
        pack([
            s.interrupt_matrix.clic.read_ctrl(CPU_LINE).ip(),
            s.interrupt_matrix.clic.read_ctrl(CPU_LINE).ie(),
            s.interrupt_matrix
                .clic
                .read_ctrl(Index::constant::<0>())
                .ip(),
        ]),
    );
}
fn admit(s: &mut Session<'_>, tag: u32) {
    IRQ_COUNT.store(0, Ordering::Relaxed);
    CLIC.store(
        ptr::addr_of_mut!(s.interrupt_matrix.clic),
        Ordering::Release,
    );
    s.interrupt_matrix.clic.modify_ctrl(CPU_LINE, |w| {
        w.set_ie(true);
    });
    oer_riscv::restore_interrupt_threshold(0);
    oer_riscv::enable_interrupts();
    wait_window();
    oer_riscv::disable_interrupts();
    CLIC.store(ptr::null_mut(), Ordering::Release);
    emit(
        tag,
        IRQ_COUNT.load(Ordering::Relaxed),
        u32::from(s.interrupt_matrix.clic.read_ctrl(CPU_LINE).ie()),
        u32::from(s.interrupt_matrix.clic.read_ctrl(CPU_LINE).ip()),
    );
}
fn begin(s: &mut Session<'_>, source: pac::Interrupt) {
    setup(s);
    feed(s);
    line(s, CONTROLS[1], false);
    route(s, source, true);
}

pub(super) fn matrix(s: &mut Session<'_>) {
    super::contain(s);
    feed(s);
    let inherited = s
        .interrupt_matrix
        .interrupt_core0
        .read_route(SOFTWARE_SOURCE)
        .pass_level();
    for source in [
        pac::Interrupt::USB_SERIAL_JTAG,
        pac::Interrupt::TIMERGRP0_T0,
        pac::Interrupt::SYSTIMER_TARGET0,
        pac::Interrupt::SYSTIMER_TARGET1,
        pac::Interrupt::CPU_INTR_FROM_CPU_2,
    ] {
        if let Ok(i) = Index::new(source as usize) {
            let r = s.interrupt_matrix.interrupt_core0.read_route(i);
            emit(0x4000, source as u32, r.map().get(), r.pass_level().get());
        }
    }
    setup(s);
    line(s, CONTROLS[1], false);
    s.source.store_hp_cpu_int_from_cpu_2(false);
    wait_window();
    matrix_state(s, 0x4010);
    s.source.store_hp_cpu_int_from_cpu_2(true);
    wait_window();
    matrix_state(s, 0x4020);
    admit(s, 0x4021);
    route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, true);
    s.interrupt_matrix.clic.modify_ctrl(CPU_LINE, |w| {
        w.set_ie(false);
    });
    wait_window();
    matrix_state(s, 0x4030);
    admit(s, 0x4031); // Intentionally leaves the peripheral high.
    matrix_state(s, 0x4032);
    admit(s, 0x4033);
    route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, false);
    wait_window();
    matrix_state(s, 0x4040);
    admit(s, 0x4041);
    route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, true);
    wait_window();
    matrix_state(s, 0x4050);
    admit(s, 0x4051);
    s.source.store_hp_cpu_int_from_cpu_2(false);
    wait_window();
    matrix_state(s, 0x4060);
    admit(s, 0x4061);
    // Four explicit single-source machine-mode routing observations, not a
    // qualification series. Restore the inherited remap after the sweep.
    for pass in 0..4 {
        s.interrupt_matrix
            .interrupt_core0
            .modify_route(SOFTWARE_SOURCE, |w| {
                w.set_pass_level(Bits::truncate(pass));
            });
        s.source.store_hp_cpu_int_from_cpu_2(true);
        wait_window();
        matrix_state(s, 0x4100 + pass * 4);
        admit(s, 0x4101 + pass * 4);
        s.source.store_hp_cpu_int_from_cpu_2(false);
        wait_window();
    }
    s.interrupt_matrix
        .interrupt_core0
        .modify_route(SOFTWARE_SOURCE, |w| {
            w.set_pass_level(inherited);
        });
    route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, false);
    matrix_state(s, 0x41ff);
}

fn usb_mask_all(s: &mut Session<'_>) {
    s.generic.usb.modify_int_ena(|w| {
        w.set_jtag_in_flush_int_ena(false);
        w.set_sof_int_ena(false);
        w.set_serial_out_recv_pkt_int_ena(false);
        w.set_serial_in_empty_int_ena(false);
        w.set_pid_err_int_ena(false);
        w.set_crc5_err_int_ena(false);
        w.set_crc16_err_int_ena(false);
        w.set_stuff_err_int_ena(false);
        w.set_in_token_rec_in_ep1_int_ena(false);
        w.set_usb_bus_reset_int_ena(false);
        w.set_out_ep1_zero_payload_int_ena(false);
        w.set_out_ep2_zero_payload_int_ena(false);
        w.set_rts_chg_int_ena(false);
        w.set_dtr_chg_int_ena(false);
        w.set_get_line_code_int_ena(false);
        w.set_set_line_code_int_ena(false);
    });
}
fn usb_state(s: &Session<'_>, tag: u32) {
    let r = s.generic.usb.read_int_raw();
    let t = s.generic.usb.read_int_st();
    let e = s.generic.usb.read_int_ena();
    emit(
        tag,
        pack([
            r.jtag_in_flush_int_raw(),
            r.sof_int_raw(),
            r.serial_out_recv_pkt_int_raw(),
            r.serial_in_empty_int_raw(),
            r.pid_err_int_raw(),
            r.crc5_err_int_raw(),
            r.crc16_err_int_raw(),
            r.stuff_err_int_raw(),
            r.in_token_rec_in_ep1_int_raw(),
            r.usb_bus_reset_int_raw(),
            r.out_ep1_zero_payload_int_raw(),
            r.out_ep2_zero_payload_int_raw(),
            r.rts_chg_int_raw(),
            r.dtr_chg_int_raw(),
            r.get_line_code_int_raw(),
            r.set_line_code_int_raw(),
        ]),
        pack([
            t.jtag_in_flush_int_st(),
            t.sof_int_st(),
            t.serial_out_recv_pkt_int_st(),
            t.serial_in_empty_int_st(),
            t.pid_err_int_st(),
            t.crc5_err_int_st(),
            t.crc16_err_int_st(),
            t.stuff_err_int_st(),
            t.in_token_rec_in_ep1_int_st(),
            t.usb_bus_reset_int_st(),
            t.out_ep1_zero_payload_int_st(),
            t.out_ep2_zero_payload_int_st(),
            t.rts_chg_int_st(),
            t.dtr_chg_int_st(),
            t.get_line_code_int_st(),
            t.set_line_code_int_st(),
        ]),
        pack([
            e.jtag_in_flush_int_ena(),
            e.sof_int_ena(),
            e.serial_out_recv_pkt_int_ena(),
            e.serial_in_empty_int_ena(),
            e.pid_err_int_ena(),
            e.crc5_err_int_ena(),
            e.crc16_err_int_ena(),
            e.stuff_err_int_ena(),
            e.in_token_rec_in_ep1_int_ena(),
            e.usb_bus_reset_int_ena(),
            e.out_ep1_zero_payload_int_ena(),
            e.out_ep2_zero_payload_int_ena(),
            e.rts_chg_int_ena(),
            e.dtr_chg_int_ena(),
            e.get_line_code_int_ena(),
            e.set_line_code_int_ena(),
        ]),
    );
    matrix_state(s, tag + 1);
    emit(
        tag + 2,
        u32::from(s.generic.usb.read_ep1_conf().serial_in_ep_data_free()),
        u32::from(s.generic.usb.read_ep1_conf().serial_out_ep_data_avail()),
        0,
    );
}
fn usb_empty(s: &mut Session<'_>, tag: u32) -> bool {
    if !s.generic.usb.read_int_raw().serial_in_empty_int_raw()
        && s.generic.usb.read_ep1_conf().serial_in_ep_data_free()
    {
        s.generic.usb.push_ep1(b'U');
        s.generic.usb.trigger_ep1_conf_wr_done();
    }
    let start = oer_riscv::cycle32();
    while !s.generic.usb.read_int_raw().serial_in_empty_int_raw()
        && oer_riscv::cycle32().wrapping_sub(start) < LIMIT
    {}
    let ready = s.generic.usb.read_int_raw().serial_in_empty_int_raw();
    emit(
        tag,
        u32::from(ready),
        oer_riscv::cycle32().wrapping_sub(start),
        0,
    );
    ready
}
pub(super) fn usb(s: &mut Session<'_>) {
    super::contain(s);
    usb_state(s, 0x5000); // Before probe writes, after the ROM RAM loader.
    begin(s, pac::Interrupt::USB_SERIAL_JTAG);
    usb_mask_all(s);
    s.generic.usb.modify_int_ena(|w| {
        w.set_serial_in_empty_int_ena(true);
    });
    if usb_empty(s, 0x5010) {
        usb_state(s, 0x5020);
        admit(s, 0x5023);
        admit(s, 0x5024);
        let ack = s.generic.usb_irq.sample_usb_serial_jtag_interrupt();
        s.generic.usb_irq.acknowledge_usb_serial_jtag_interrupt(ack);
        usb_state(s, 0x5030);
        wait_window();
        usb_state(s, 0x5040);
        if usb_empty(s, 0x5050) {
            usb_mask_all(s);
            usb_state(s, 0x5060);
            admit(s, 0x5063);
            s.generic.usb.modify_int_ena(|w| {
                w.set_serial_in_empty_int_ena(true);
            });
            usb_state(s, 0x5070);
            admit(s, 0x5073);
            // The affine enabled ST sample is retained across masking. This
            // tests CLR at ENA=0 without fabricating a clear token.
            let ack = s.generic.usb_irq.sample_usb_serial_jtag_interrupt();
            usb_mask_all(s);
            s.generic.usb_irq.acknowledge_usb_serial_jtag_interrupt(ack);
            usb_state(s, 0x5080);
            wait_window();
            usb_state(s, 0x5090);
            s.generic.usb.modify_int_ena(|w| {
                w.set_serial_in_empty_int_ena(true);
            });
            usb_state(s, 0x50a0);
            admit(s, 0x50a3);
        }
    }
    usb_mask_all(s);
    let ack = s.generic.usb.sample_int_raw();
    s.generic.usb.acknowledge_int_raw(ack); // Separate RAW W1C, not CLR evidence.
    wait_window();
    usb_state(s, 0x50f0);
    admit(s, 0x50f3);
    route(s, pac::Interrupt::USB_SERIAL_JTAG, false);
}

fn sy_work(s: &mut Session<'_>, n: usize, enable: bool) {
    s.generic.systimer.systimer.modify_conf(|w| {
        if n == 0 {
            w.set_target0_work_en(enable);
        } else {
            w.set_target1_work_en(enable);
        }
    });
}
fn sy_ena(s: &mut Session<'_>, n: usize, enable: bool) {
    s.generic.systimer.systimer.modify_int_ena(|w| {
        if n == 0 {
            w.set_target0_int_ena(enable);
        } else {
            w.set_target1_int_ena(enable);
        }
    });
}
fn sy_clear(s: &mut Session<'_>, n: usize) {
    if n == 0 {
        s.generic
            .systimer
            .systimer_alarm0
            .trigger_int_clr_target0_int_clr();
    } else {
        s.generic
            .systimer
            .systimer_alarm1
            .trigger_int_clr_target1_int_clr();
    }
    oer_riscv::fence();
}
fn sy_raw(s: &Session<'_>, n: usize) -> bool {
    let r = s.generic.systimer.systimer.read_int_raw();
    if n == 0 {
        r.target0_int_raw()
    } else {
        r.target1_int_raw()
    }
}
fn sy_now(s: &Session<'_>) -> Option<u64> {
    let counter = &s.generic.systimer.systimer_counter;
    counter.trigger_unit0_op_timer_unit0_update();
    let start = oer_riscv::cycle32();
    while !counter.read_unit0_op().timer_unit0_value_valid() {
        if oer_riscv::cycle32().wrapping_sub(start) >= LIMIT {
            return None;
        }
    }
    Some(
        (u64::from(counter.read_unit0_value_hi().timer_unit0_value_hi().get()) << 32)
            | u64::from(counter.read_unit0_value_lo().timer_unit0_value_lo()),
    )
}
fn sy_program(s: &mut Session<'_>, n: usize, ticks: u64) {
    let i = if n == 0 {
        Index::constant::<0>()
    } else {
        Index::constant::<1>()
    };
    sy_work(s, n, false);
    let timer = &mut s.generic.systimer.systimer;
    timer.modify_target_hi(i, |w| {
        w.set_hi(Bits::truncate((ticks >> 32) as u32));
    });
    timer.modify_target_lo(i, |w| {
        w.set_lo(ticks as u32);
    });
    timer.trigger_comp_load_load(i);
    sy_work(s, n, true);
}
fn sy_state(s: &Session<'_>, tag: u32) {
    let timer = &s.generic.systimer.systimer;
    let r = timer.read_int_raw();
    let t = timer.read_int_st();
    let e = timer.read_int_ena();
    emit(
        tag,
        pack([
            r.target0_int_raw(),
            r.target1_int_raw(),
            r.target2_int_raw(),
        ]),
        pack([t.target0_int_st(), t.target1_int_st(), t.target2_int_st()]),
        pack([
            e.target0_int_ena(),
            e.target1_int_ena(),
            e.target2_int_ena(),
        ]),
    );
    matrix_state(s, tag + 1);
    let c = timer.read_conf();
    emit(
        tag + 2,
        pack([
            c.target0_work_en(),
            c.target1_work_en(),
            c.target2_work_en(),
        ]),
        u32::from(c.timer_unit0_work_en()),
        0,
    );
}
fn sy_future(s: &mut Session<'_>, n: usize, tag: u32) -> bool {
    let Some(now) = sy_now(s) else {
        emit(tag, 0, 0, 1);
        return false;
    };
    sy_program(s, n, now.wrapping_add(16_000));
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
fn systimer(s: &mut Session<'_>, n: usize) {
    begin(
        s,
        if n == 0 {
            pac::Interrupt::SYSTIMER_TARGET0
        } else {
            pac::Interrupt::SYSTIMER_TARGET1
        },
    );
    // SOURCE: production platform::system_timer::open_clock. Only this block
    // is reset; no timer-group/watchdog reset or RF operation occurs.
    let clock = &mut s.generic.systimer_clock;
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_apb_clk_en(true);
    });
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(true);
    });
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(false);
    });
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_clk_en(true);
    });
    s.generic.systimer.systimer.modify_conf(|w| {
        w.set_timer_unit0_work_en(true)
            .set_timer_unit0_core0_stall_en(false)
            .set_timer_unit0_core1_stall_en(false);
    });
    let i = if n == 0 {
        Index::constant::<0>()
    } else {
        Index::constant::<1>()
    };
    s.generic.systimer.systimer.modify_target_conf(i, |w| {
        w.set_period_mode(false).set_timer_unit_sel(false);
    });
    sy_work(s, n, false);
    sy_ena(s, n, true);
    sy_clear(s, n);
    emit(0x6000, n as u32, 0, 0);
    sy_state(s, 0x6010);
    if sy_future(s, n, 0x6020) {
        sy_state(s, 0x6030);
        admit(s, 0x6033);
        admit(s, 0x6034);
        sy_ena(s, n, false);
        sy_state(s, 0x6110); // Mask an already-positive enabled pending cause.
        admit(s, 0x6113);
        sy_ena(s, n, true);
        sy_state(s, 0x6120);
        admit(s, 0x6123);
        sy_work(s, n, false);
        sy_state(s, 0x6040);
        admit(s, 0x6043);
        sy_clear(s, n);
        wait_window();
        sy_state(s, 0x6050);
        admit(s, 0x6053);
    }
    sy_ena(s, n, false);
    if sy_future(s, n, 0x6060) {
        sy_state(s, 0x6070);
        admit(s, 0x6073);
        sy_clear(s, n); // ENA=0, WORK_EN=1.
        wait_window();
        sy_state(s, 0x6130);
        // Recreate pending for the independent WORK_EN=0 / ENA=0 clear.
        if !sy_future(s, n, 0x6133) {
            emit(0x6134, 0, 0, 1);
        }
        sy_work(s, n, false);
        sy_clear(s, n);
        wait_window();
        sy_state(s, 0x6080);
    }
    if sy_future(s, n, 0x6090) {
        sy_state(s, 0x60a0);
        sy_ena(s, n, true);
        sy_state(s, 0x60b0);
        admit(s, 0x60b3);
        sy_clear(s, n);
        wait_window();
        sy_state(s, 0x60c0);
    }
    // One phased masked-clear/reprogram example, not a race qualification.
    sy_ena(s, n, false);
    sy_work(s, n, false);
    sy_clear(s, n);
    if let Some(now) = sy_now(s) {
        sy_program(s, n, now.saturating_sub(16));
        sy_state(s, 0x60d0);
        wait_window();
        sy_state(s, 0x60e0);
        sy_ena(s, n, true);
        admit(s, 0x60e3);
        sy_state(s, 0x60e4);
    }
    sy_work(s, n, false);
    sy_ena(s, n, false);
    sy_clear(s, n);
    wait_window();
    sy_state(s, 0x60f0);
    admit(s, 0x60f3);
    route(
        s,
        if n == 0 {
            pac::Interrupt::SYSTIMER_TARGET0
        } else {
            pac::Interrupt::SYSTIMER_TARGET1
        },
        false,
    );
}
pub(super) fn systimer0(s: &mut Session<'_>) {
    systimer(s, 0);
}
pub(super) fn systimer1(s: &mut Session<'_>) {
    systimer(s, 1);
}

fn t0_count(s: &mut Session<'_>) -> Option<u64> {
    let timer = &mut s.generic.timer.timg0_timer0;
    timer.capture_timg0_timer0();
    let start = oer_riscv::cycle32();
    while timer.read_t0update().t_update() {
        if oer_riscv::cycle32().wrapping_sub(start) >= LIMIT {
            return None;
        }
    }
    Some((u64::from(timer.read_t0hi().t_hi().get()) << 32) | u64::from(timer.read_t0lo().t_lo()))
}
fn t0_state(s: &Session<'_>, tag: u32) {
    let timer = &s.generic.timer.timg0_timer0;
    let bank = &s.generic.timer.timg0_interrupt_bank;
    let r = bank.read_int_raw_timers();
    let t = bank.read_int_st_timers();
    let e = bank.read_int_ena_timers();
    emit(
        tag,
        pack([r.t0_int_raw(), r.t1_int_raw(), r.wdt_int_raw()]),
        pack([t.t0_int_st(), t.t1_int_st(), t.wdt_int_st()]),
        pack([e.t0_int_ena(), e.t1_int_ena(), e.wdt_int_ena()]),
    );
    matrix_state(s, tag + 1);
    let c = timer.read_t0config();
    emit(
        tag + 2,
        pack([c.t_en(), c.t_alarm_en(), c.t_autoreload(), c.t_increase()]),
        u32::from(c.t_divider()),
        0,
    );
}
fn t0_ena(s: &mut Session<'_>, enable: bool) {
    s.generic
        .timer
        .timg0_interrupt_bank
        .modify_int_ena_timers(|w| {
            w.set_t0_int_ena(enable);
        });
}
fn t0_clear(s: &mut Session<'_>) {
    s.generic
        .timer
        .timg0_interrupt_bank
        .trigger_int_clr_timers_t0_int_clr();
    oer_riscv::fence();
}
fn t0_target(s: &mut Session<'_>, ticks: u64) {
    let timer = &mut s.generic.timer.timg0_timer0;
    timer.modify_t0alarmhi(|w| {
        w.set_t_alarm_hi(Bits::truncate((ticks >> 32) as u32));
    });
    timer.modify_t0alarmlo(|w| {
        w.set_t_alarm_lo(ticks as u32);
    });
    timer.configure_timg0_timer0_armed();
}
fn t0_wait(s: &Session<'_>, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    while !s
        .generic
        .timer
        .timg0_interrupt_bank
        .read_int_raw_timers()
        .t0_int_raw()
        && oer_riscv::cycle32().wrapping_sub(start) < LIMIT
    {}
    let ready = s
        .generic
        .timer
        .timg0_interrupt_bank
        .read_int_raw_timers()
        .t0_int_raw();
    emit(
        tag,
        u32::from(ready),
        oer_riscv::cycle32().wrapping_sub(start),
        0,
    );
    ready
}
fn t0_future(s: &mut Session<'_>, tag: u32) -> bool {
    let Some(now) = t0_count(s) else {
        emit(tag, 0, 0, 1);
        return false;
    };
    t0_target(s, now.wrapping_add(1000));
    t0_wait(s, tag)
}
pub(super) fn timer0(s: &mut Session<'_>) {
    begin(s, pac::Interrupt::TIMERGRP0_T0);
    let w0 = s.generic.watchdog.read_wdtconfig0();
    let w1 = s.generic.watchdog.read_wdtconfig1();
    let w2 = s.generic.watchdog.read_wdtconfig2();
    let clock = s.generic.watchdog_clock.read_timergrp0_ctrl0();
    let ena = s.generic.timer.timg0_interrupt_bank.read_int_ena_timers();
    emit(
        0x7000,
        pack([
            clock.reg_timergrp0_apb_clk_en(),
            clock.reg_timergrp0_rst_en(),
            clock.reg_timergrp0_t0_clk_en(),
            clock.reg_timergrp0_wdt_clk_en(),
        ]),
        clock.reg_timergrp0_wdt_src_sel().get(),
        u32::from(ena.wdt_int_ena()),
    );
    if clock.reg_timergrp0_rst_en() || !clock.reg_timergrp0_apb_clk_en() {
        emit(0x70ee, 0, 0, 1);
        route(s, pac::Interrupt::TIMERGRP0_T0, false);
        return;
    }
    // Startup uses the watchdog-side clock owner, not the T0 partition.
    s.generic.watchdog_clock.modify_timergrp0_ctrl0(|w| {
        w.set_reg_timergrp0_t0_src_sel(Bits::constant::<
            { pac::hp_sys_clkrst::timergrp0_ctrl0::RegTimergrp0T0SrcSel::Xtal.bits() },
        >())
        .set_reg_timergrp0_t0_clk_en(true);
    });
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_stopped();
    t0_ena(s, false);
    t0_clear(s);
    s.generic.timer.timg0_timer0.modify_t0loadlo(|w| {
        w.set_t_load_lo(0);
    });
    s.generic.timer.timg0_timer0.modify_t0loadhi(|w| {
        w.set_t_load_hi(Bits::constant::<0>());
    });
    s.generic.timer.timg0_timer0.trigger_t0load_t_load(1);
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_counting();
    if let Some(a) = t0_count(s) {
        let cycle = oer_riscv::cycle32();
        window(40_000);
        if let Some(b) = t0_count(s) {
            emit(
                0x7010,
                a as u32,
                b as u32,
                oer_riscv::cycle32().wrapping_sub(cycle),
            );
        }
    }
    t0_ena(s, true);
    if t0_future(s, 0x7020) {
        t0_state(s, 0x7030);
        admit(s, 0x7033);
        admit(s, 0x7034);
        s.generic
            .timer
            .timg0_timer0
            .configure_timg0_timer0_stopped();
        t0_state(s, 0x7040);
        t0_ena(s, false);
        t0_state(s, 0x7050);
        admit(s, 0x7053);
        t0_ena(s, true);
        t0_state(s, 0x7060);
        admit(s, 0x7063);
        t0_clear(s);
        wait_window();
        t0_state(s, 0x7070);
    }
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_counting();
    t0_ena(s, false);
    if t0_future(s, 0x7080) {
        t0_state(s, 0x7090);
        t0_clear(s);
        wait_window();
        t0_state(s, 0x70a0);
        t0_ena(s, true);
        admit(s, 0x70a3);
    }
    t0_ena(s, true);
    if t0_future(s, 0x70b0) {
        t0_state(s, 0x70c0);
        admit(s, 0x70c3);
        t0_clear(s);
    }
    t0_clear(s);
    wait_window();
    t0_state(s, 0x70c4); // Positive zero-RAW control before the past target.
    t0_target(s, 0);
    t0_wait(s, 0x70d0);
    t0_state(s, 0x70e0);
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_stopped();
    t0_ena(s, false);
    t0_clear(s);
    let a = t0_count(s);
    wait_window();
    let b = t0_count(s);
    emit(
        0x70f0,
        u32::from(a.is_some() && a == b),
        a.unwrap_or(0) as u32,
        b.unwrap_or(0) as u32,
    );
    t0_state(s, 0x70f1);
    admit(s, 0x70f4);
    s.generic.watchdog_clock.modify_timergrp0_ctrl0(|w| {
        w.set_reg_timergrp0_t0_clk_en(false);
    });
    let after = s.generic.watchdog_clock.read_timergrp0_ctrl0();
    emit(
        0x70ff,
        pack([
            s.generic.watchdog.read_wdtconfig0() == w0,
            s.generic.watchdog.read_wdtconfig1() == w1,
            s.generic.watchdog.read_wdtconfig2() == w2,
        ]),
        pack([
            after.reg_timergrp0_wdt_clk_en() == clock.reg_timergrp0_wdt_clk_en(),
            after.reg_timergrp0_wdt_src_sel() == clock.reg_timergrp0_wdt_src_sel(),
            s.generic
                .timer
                .timg0_interrupt_bank
                .read_int_ena_timers()
                .wdt_int_ena()
                == ena.wdt_int_ena(),
            after.reg_timergrp0_t1_clk_en() == clock.reg_timergrp0_t1_clk_en(),
        ]),
        0,
    );
    route(s, pac::Interrupt::TIMERGRP0_T0, false);
}
