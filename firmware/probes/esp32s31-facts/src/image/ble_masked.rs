//! Finite BT_MAC hidden-pending and diagnostic-read observations; no RF/DMA.
#![forbid(unsafe_code)]
use super::{Index, Session, ble_wave2::mac, emit, irq_observer, pac};

fn settle() {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}

fn discard(s: &mut Session<'_>, cause: u32) {
    // This disposable session consumes this indication itself. Compare
    // producers have been disabled by produce; no IRQ handler/time owner runs.
    match cause {
        0 => s.ble.bank.discard_bluetooth_compare_0_indication(),
        1 => s.ble.bank.discard_bluetooth_compare_1_indication(),
        _ => s
            .ble
            .bank
            .discard_bluetooth_controller_time_overflow_indication(),
    }
    oer_riscv::fence();
}

fn state(s: &Session<'_>, tag: u32) {
    emit(
        tag,
        s.ble
            .bank
            .sample_bluetooth_primary_interrupt_bank_0()
            .bits(),
        s.ble
            .bank
            .sample_bluetooth_primary_interrupt_bank_1()
            .bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        s.ble.bank.read_irq_status_snapshot_0().pending_bits(),
        s.ble.bank.read_irq_status_snapshot_1().pending_bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    let ie = irq_observer::INPUTS
        .into_iter()
        .enumerate()
        .fold(0, |a, (n, i)| {
            a | (u32::from(s.interrupt_matrix.clic.read_ctrl(i).ie()) << n)
        });
    emit(
        tag + 2,
        irq_observer::pending(s),
        ie,
        u32::from(oer_riscv::take_interrupts()),
    );
    let w = &s.wake_status;
    let r = w.read_int_raw();
    use pac::pmu::int_raw as f;
    let raw = u32::from(r.lp_cpu_exc_int_raw()) * f::LP_CPU_EXC_INT_RAW_MASK
        | u32::from(r.sdio_idle_int_raw()) * f::SDIO_IDLE_INT_RAW_MASK
        | u32::from(r.sw_int_raw()) * f::SW_INT_RAW_MASK
        | u32::from(r.soc_sleep_reject_int_raw()) * f::SOC_SLEEP_REJECT_INT_RAW_MASK
        | u32::from(r.soc_wakeup_int_raw()) * f::SOC_WAKEUP_INT_RAW_MASK;
    emit(
        tag + 3,
        s.modem_wake_status
            .read_modem_intr_status()
            .modem_intr_vector()
            .get(),
        raw,
        w.read_slp_wakeup_status0().wakeup_cause(),
    );
    emit(
        tag + 4,
        lp_raw(w),
        u32::from(w.read_hp_int_st().soc_wakeup_int_st())
            | (u32::from(w.read_hp_int_st().soc_sleep_reject_int_st()) << 1)
            | (u32::from(w.read_lp_int_st().lp_cpu_wakeup_int_st()) << 2),
        u32::from(s.ble.etm.read_word_010c_opaque().bit_26_opaque()),
    );
    let ena = s.ble.bank.read_irq_enable_1();
    emit(
        tag + 5,
        u32::from(ena.source_4()),
        u32::from(ena.source_5()),
        u32::from(ena.controller_time_overflow()),
    );
}

fn lp_raw(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::lp_int_raw as f;
    let r = p.read_lp_int_raw();
    u32::from(r.lp_cpu_wakeup_int_raw()) * f::LP_CPU_WAKEUP_INT_RAW_MASK
        | u32::from(r.modem_switch_active_end_int_raw()) * f::MODEM_SWITCH_ACTIVE_END_INT_RAW_MASK
        | u32::from(r.sleep_switch_active_end_int_raw()) * f::SLEEP_SWITCH_ACTIVE_END_INT_RAW_MASK
        | u32::from(r.sleep_switch_modem_end_int_raw()) * f::SLEEP_SWITCH_MODEM_END_INT_RAW_MASK
        | u32::from(r.modem_switch_sleep_end_int_raw()) * f::MODEM_SWITCH_SLEEP_END_INT_RAW_MASK
        | u32::from(r.active_switch_sleep_end_int_raw()) * f::ACTIVE_SWITCH_SLEEP_END_INT_RAW_MASK
        | u32::from(r.modem_switch_active_start_int_raw())
            * f::MODEM_SWITCH_ACTIVE_START_INT_RAW_MASK
        | u32::from(r.sleep_switch_active_start_int_raw())
            * f::SLEEP_SWITCH_ACTIVE_START_INT_RAW_MASK
        | u32::from(r.sleep_switch_modem_start_int_raw()) * f::SLEEP_SWITCH_MODEM_START_INT_RAW_MASK
        | u32::from(r.modem_switch_sleep_start_int_raw()) * f::MODEM_SWITCH_SLEEP_START_INT_RAW_MASK
        | u32::from(r.active_switch_sleep_start_int_raw())
            * f::ACTIVE_SWITCH_SLEEP_START_INT_RAW_MASK
        | u32::from(r.hp_sw_trigger_int_raw()) * f::HP_SW_TRIGGER_INT_RAW_MASK
}

fn routes(s: &Session<'_>, tag: u32, cause: u32) {
    emit(
        tag,
        cause,
        s.interrupt_matrix
            .interrupt_core0
            .read_route(Index::constant::<{ pac::Interrupt::MODEM_BT_MAC as usize }>())
            .map()
            .get(),
        s.interrupt_matrix
            .interrupt_core0
            .read_route(Index::constant::<
                { pac::Interrupt::MODEM_BT_MAC_INT1 as usize },
            >())
            .map()
            .get(),
    );
}

fn word_04bc(s: &Session<'_>) -> u32 {
    s.ble.task_irq.read_bluetooth_task_diagnostic_image()
}

fn reads(s: &mut Session<'_>, tag: u32) {
    // Positive advancing-count brackets distinguish a dead timer from a read
    // that stops it. Neither interval is a calibrated timing measurement.
    let a = mac::sample(s);
    settle();
    let b = mac::sample(s);
    let r0 = word_04bc(s);
    let r1 = word_04bc(s);
    let r2 = word_04bc(s);
    let c = mac::sample(s);
    settle();
    let d = mac::sample(s);
    emit(tag, r0, r1, r2);
    match (a, b, c, d) {
        (Some(a), Some(b), Some(c), Some(d)) => {
            emit(
                tag + 1,
                b.wrapping_sub(a),
                c.wrapping_sub(b),
                d.wrapping_sub(c),
            );
        }
        _ => emit(0x32ee, 2, tag, 0),
    }
}

fn produce(s: &mut Session<'_>, cause: u32, phase: u32) -> bool {
    if mac::produce(s, cause, 0x32f0 + phase) {
        true
    } else {
        emit(0x32ee, 1, cause, phase);
        false
    }
}

fn run(s: &mut Session<'_>, cause: u32) {
    if !irq_observer::control(s) || !mac::prepare(s) {
        emit(0x32ee, 0, cause, 0);
        mac::finish(s);
        return;
    }
    irq_observer::route(s, pac::Interrupt::MODEM_BT_MAC, 0);
    irq_observer::route(s, pac::Interrupt::MODEM_BT_MAC_INT1, 0);
    routes(s, 0x32fe, cause); // Main experiment: both matrix routes parked.
    state(s, 0x3200);
    if !produce(s, cause, 0) {
        mac::finish(s);
        return;
    }
    state(s, 0x3210); // New hidden pending; no earlier positive sample.
    mac::enable(s, cause, true);
    settle();
    state(s, 0x3220); // Positive control for hidden arrival.
    mac::enable(s, cause, false);
    state(s, 0x3230);
    mac::enable(s, cause, true);
    state(s, 0x3240); // Status reads and masking have retained pending.
    discard(s, cause); // Fixed image, enable unchanged.
    settle();
    state(s, 0x3250);
    mac::enable(s, cause, false);
    mac::enable(s, cause, true);
    state(s, 0x3260); // Quiet reopen after enabled fixed clear.
    mac::enable(s, cause, false);
    if !produce(s, cause, 1) {
        mac::finish(s);
        return;
    }
    state(s, 0x3270);
    discard(s, cause); // Fresh hidden arrival; never opened before W1C.
    state(s, 0x3280);
    mac::enable(s, cause, true);
    state(s, 0x3290); // Quiet reopen after masked fixed clear.

    mac::enable(s, cause, false);
    // Separate output observation: route to validated positive-edge inputs,
    // with IE=0 throughout. No CPU delivery is admitted on either route.
    irq_observer::route(s, pac::Interrupt::MODEM_BT_MAC, super::LINE as u32);
    irq_observer::route(
        s,
        pac::Interrupt::MODEM_BT_MAC_INT1,
        (super::LINE + 1) as u32,
    );
    irq_observer::clear(s);
    routes(s, 0x32ff, cause);
    if !produce(s, cause, 2) {
        mac::finish(s);
        return;
    }
    mac::enable(s, cause, true);
    settle();
    state(s, 0x32a0); // Fresh positive controls stimulus, matrix and CLIC IP.
    reads(s, 0x32a8);
    state(s, 0x32b0); // Pending and progressing timer after repeated +4bc.
    mac::enable(s, cause, false);
    state(s, 0x32c0);
    reads(s, 0x32c8);
    state(s, 0x32d0);
    mac::enable(s, cause, true);
    state(s, 0x32e0); // Hidden pending survives the diagnostic reads.
    discard(s, cause);
    settle();
    irq_observer::clear(s);
    state(s, 0x3300); // Positive clear control also after diagnostic reads.
    irq_observer::route(s, pac::Interrupt::MODEM_BT_MAC, 0);
    irq_observer::route(s, pac::Interrupt::MODEM_BT_MAC_INT1, 0);
    mac::finish(s);
}

pub(super) fn compare0(s: &mut Session<'_>) {
    run(s, 0);
}
pub(super) fn compare1(s: &mut Session<'_>) {
    run(s, 1);
}
pub(super) fn overflow(s: &mut Session<'_>) {
    run(s, 2);
}
