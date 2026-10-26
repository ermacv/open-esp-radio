//! Positive digital controller-time causes, with bounded source observations.
use super::{Bits, LIMIT, Session, emit, pac, setup, window};

mod overflow;
pub(in crate::image) use overflow::{overflow_ack, overflow_rate, overflow_reset};

fn sample(s: &mut Session<'_>) -> Option<u32> {
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_latch_request(true);
    });
    oer_riscv::fence();
    let start = oer_riscv::cycle32();
    while s.ble.core.read_btlc_timer_control().latch_request() {
        if oer_riscv::cycle32().wrapping_sub(start) >= LIMIT {
            return None;
        }
    }
    Some(s.ble.core.read_btlc_timer_capture_0().value())
}
fn all_mask(s: &mut Session<'_>) {
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_0(false)
            .set_source_1(false)
            .set_source_8(false)
            .set_source_13(false)
            .set_source_15(false)
            .set_source_16(false)
            .set_source_21(false)
            .set_sources_27_28(Bits::constant::<0>())
            .set_bits_2_7_opaque(Bits::constant::<0>())
            .set_bits_9_12_opaque(Bits::constant::<0>())
            .set_bit_14_opaque(false)
            .set_bits_17_20_opaque(Bits::constant::<0>())
            .set_bits_22_26_opaque(Bits::constant::<0>())
            .set_bits_29_31_opaque(Bits::constant::<0>());
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_source_1(false)
            .set_source_3(false)
            .set_source_4(false)
            .set_source_5(false)
            .set_source_6(false)
            .set_source_8(false)
            .set_source_9(false)
            .set_source_12(false)
            .set_source_13(false)
            .set_bit_0_opaque(false)
            .set_bit_2_opaque(false)
            .set_bit_7_opaque(false)
            .set_bits_10_11_opaque(Bits::constant::<0>())
            .set_bits_14_31_opaque(Bits::constant::<0>());
    });
    oer_riscv::fence();
}
pub(super) fn clear(s: &mut Session<'_>) {
    let a = s.ble.bank.sample_bluetooth_primary_interrupt_bank_0();
    let b = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
    s.ble.bank.acknowledge_bluetooth_primary_interrupt_bank_0(a);
    s.ble.bank.acknowledge_bluetooth_primary_interrupt_bank_1(b);
    let a = s.ble.bank.sample_bluetooth_interrupt_bank_0();
    let b = s.ble.bank.sample_bluetooth_interrupt_bank_1();
    s.ble.bank.acknowledge_bluetooth_interrupt_bank_0(a);
    s.ble.bank.acknowledge_bluetooth_interrupt_bank_1(b);
    oer_riscv::fence();
}
pub(super) fn command(s: &mut Session<'_>, n: u32) {
    match n {
        0 => s.ble.bank.write_irq_control_0(|w| {
            w.set_command(pac::bt_mac::irq_control_0::Command::Command1.bits());
        }),
        1 => s.ble.bank.write_irq_control_1(|w| {
            w.set_command(pac::bt_mac::irq_control_1::Command::Command1.bits());
        }),
        _ => s.ble.bank.write_irq_control_2(|w| {
            w.set_command(pac::bt_mac::irq_control_2::Command::Command1.bits());
        }),
    }
    oer_riscv::fence();
}
pub(super) fn enable(s: &mut Session<'_>, cause: u32, on: bool) {
    s.ble.bank.modify_irq_enable_1(|w| match cause {
        0 => {
            w.set_source_4(on);
        }
        1 => {
            w.set_source_5(on);
        }
        _ => {
            w.set_source_1(on);
        }
    });
    oer_riscv::fence();
}
fn compare(s: &mut Session<'_>, cause: u32, on: bool) {
    s.ble.core.modify_btlc_timer_control(|w| {
        if cause == 0 {
            w.set_compare_0_enable(on);
        } else {
            w.set_compare_1_enable(on);
        }
    });
    oer_riscv::fence();
}
pub(super) fn state(s: &Session<'_>, tag: u32) {
    let b = &s.ble.bank;
    emit(
        tag,
        b.sample_bluetooth_primary_interrupt_bank_0().bits(),
        b.sample_bluetooth_primary_interrupt_bank_1().bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        b.read_irq_status_snapshot_0().pending_bits(),
        b.read_irq_status_snapshot_1().pending_bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    let e = b.read_irq_enable_1();
    let f = b.read_irq_enable_0();
    emit(
        tag + 2,
        u32::from(e.source_1()),
        u32::from(e.source_4()),
        u32::from(e.source_5()),
    );
    emit(
        tag + 3,
        u32::from(f.source_15()),
        u32::from(f.source_21()),
        f.sources_27_28().get(),
    );
    emit(
        tag + 4,
        u32::from(e.source_8()) + 2 * u32::from(e.source_9()) + 4 * u32::from(e.source_12()),
        u32::from(e.source_3()),
        e.bits_14_31_opaque().get(),
    );
}
pub(super) fn prepare(s: &mut Session<'_>) -> bool {
    setup(s);
    all_mask(s);
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_config_24(false)
            .set_clock_shift(Bits::constant::<3>())
            .set_compare_0_enable(false)
            .set_compare_1_enable(false);
    });
    command(s, 0);
    clear(s);
    let a = sample(s);
    window(4096);
    let b = sample(s);
    let delta = match (a, b) {
        (Some(a), Some(b)) => b.wrapping_sub(a),
        _ => 0,
    };
    emit(
        0x91f0,
        u32::from(a.is_some()),
        u32::from(b.is_some()),
        delta,
    );
    emit(
        0x91f1,
        u32::from(s.ble.task_irq.read_interrupt_enable().source_17()),
        u32::from(s.ble.task_irq.read_interrupt_enable().scheduler_run()),
        0,
    );
    delta != 0
}
fn seed(s: &mut Session<'_>, value: u32) {
    s.ble.core.modify_btlc_timer_value_0(|w| {
        w.set_value(value);
    });
    s.ble.core.modify_btlc_timer_value_1(|w| {
        w.set_value(0);
    });
    s.ble.core.modify_btlc_timer_value_2(|w| {
        w.set_value(0);
    });
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_timer_arm(true);
    });
    oer_riscv::fence();
}
fn wait(s: &mut Session<'_>, origin: u32, delta: u32) -> Option<u32> {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < LIMIT {
        let now = sample(s)?;
        if now.wrapping_sub(origin) >= delta {
            return Some(now);
        }
    }
    None
}
pub(super) fn produce(s: &mut Session<'_>, cause: u32, tag: u32) -> bool {
    let (origin, delta) = if cause == 2 {
        seed(s, 0xfffffc00);
        let Some(origin) = sample(s) else {
            emit(tag, 0, 0, 0);
            return false;
        };
        if origin < 0xfffffc00 {
            emit(tag, 0, origin, 0);
            return false;
        }
        (origin, 0u32.wrapping_sub(origin))
    } else {
        let Some(origin) = sample(s) else {
            return false;
        };
        let target = origin.wrapping_add(512);
        if cause == 0 {
            s.ble.core.write_btlc_timer_cc_0(|w| {
                w.set_value(target);
            });
        } else {
            s.ble.core.write_btlc_timer_cc_1(|w| {
                w.set_value(target);
            });
        }
        compare(s, cause, true);
        (origin, 512)
    };
    let end = wait(s, origin, delta);
    if cause < 2 {
        compare(s, cause, false);
    }
    emit(tag, origin, end.unwrap_or(origin), u32::from(end.is_some()));
    end.is_some()
}
pub(super) fn finish(s: &mut Session<'_>) {
    all_mask(s);
    clear(s);
    command(s, 1);
    command(s, 2);
    state(s, 0x91e0);
    emit(
        0x91ef,
        u32::from(s.ble.task_irq.read_interrupt_enable().source_17()),
        u32::from(s.ble.task_irq.read_interrupt_enable().scheduler_run()),
        0,
    );
}
pub(in crate::image) fn mapping(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    for cause in 0..3 {
        all_mask(s);
        clear(s);
        enable(s, cause, true);
        let tag = 0x9100 + cause * 0x30;
        state(s, tag);
        if !produce(s, cause, tag + 0x10) {
            break;
        }
        state(s, tag + 0x20);
        clear(s);
    }
    finish(s);
}
pub(in crate::image) fn masks(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    for cause in 0..3 {
        all_mask(s);
        clear(s);
        enable(s, cause, true);
        let tag = 0x9200 + cause * 0x40;
        if !produce(s, cause, tag + 0x3f) {
            break;
        }
        state(s, tag);
        let saved = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
        emit(tag + 5, saved.bits(), 0, 0);
        enable(s, cause, false);
        state(s, tag + 8);
        enable(s, cause, true);
        state(s, tag + 16);
        enable(s, cause, false);
        s.ble
            .bank
            .acknowledge_bluetooth_primary_interrupt_bank_1(saved);
        oer_riscv::fence();
        state(s, tag + 24);
        enable(s, cause, true);
        state(s, tag + 32);
        clear(s);
        enable(s, cause, false);
        if !produce(s, cause, tag + 0x3e) {
            break;
        }
        state(s, tag + 40);
        enable(s, cause, true);
        state(s, tag + 48);
        clear(s);
    }
    finish(s);
}
pub(in crate::image) fn controls(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    for mode in 1..=3 {
        command(s, 0);
        clear(s);
        enable(s, 2, true);
        let tag = 0x9300 + mode * 0x40;
        if !produce(s, 2, tag + 0x3f) {
            break;
        }
        state(s, tag);
        if mode != 2 {
            command(s, 1);
            state(s, tag + 8);
        }
        if mode != 1 {
            command(s, 2);
            state(s, tag + 16);
        }
        command(s, 0);
        state(s, tag + 24);
        window(4096);
        state(s, tag + 32);
        clear(s);
    }
    finish(s);
}
pub(super) fn group(s: &mut Session<'_>, n: u32, on: bool) {
    match n {
        0 => s.ble.bank.modify_irq_enable_0(|w| {
            w.set_source_15(on);
        }),
        1 => s.ble.bank.modify_irq_enable_1(|w| {
            w.set_source_8(on).set_source_9(on).set_source_12(on);
        }),
        2 => s.ble.bank.modify_irq_enable_0(|w| {
            w.set_source_21(on).set_sources_27_28(if on {
                Bits::constant::<3>()
            } else {
                Bits::constant::<0>()
            });
        }),
        _ => s.ble.bank.modify_irq_enable_1(|w| {
            w.set_source_3(on);
        }),
    }
    oer_riscv::fence();
}
pub(in crate::image) fn groups(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    enable(s, 2, true);
    if !produce(s, 2, 0x94ff) {
        finish(s);
        return;
    }
    state(s, 0x9400);
    for n in 0..4 {
        group(s, n, true);
        state(s, 0x9410 + n * 0x10);
        group(s, n, false);
        state(s, 0x9418 + n * 0x10);
    }
    // Exact dynamic clear policy, then release controls and baseline mask.
    s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_0();
    s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_1();
    state(s, 0x9460);
    command(s, 1);
    state(s, 0x9470);
    command(s, 2);
    state(s, 0x9480);
    group(s, 0, false);
    group(s, 1, false);
    state(s, 0x9490);
    finish(s);
}
pub(in crate::image) fn arrivals(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    enable(s, 0, true);
    enable(s, 1, true);
    if !produce(s, 0, 0x950f) {
        finish(s);
        return;
    }
    state(s, 0x9500);
    let old = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
    if !produce(s, 1, 0x951f) {
        finish(s);
        return;
    }
    state(s, 0x9510);
    s.ble
        .bank
        .acknowledge_bluetooth_primary_interrupt_bank_1(old);
    oer_riscv::fence();
    state(s, 0x9520);
    clear(s);
    // Separated same-bit phases; no claim of exact set/clear coincidence.
    if !produce(s, 0, 0x953f) {
        finish(s);
        return;
    }
    let old = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
    if !produce(s, 0, 0x954f) {
        finish(s);
        return;
    }
    state(s, 0x9540);
    s.ble
        .bank
        .acknowledge_bluetooth_primary_interrupt_bank_1(old);
    oer_riscv::fence();
    state(s, 0x9550);
    if produce(s, 0, 0x956f) {
        state(s, 0x9560);
    }
    finish(s);
}
