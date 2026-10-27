//! Bounded inherited-enable and saved-snapshot experiments; no RF or DMA.
use super::{
    Bits, FACTS_OUTPUT, Index, Ordering, RETAINED, SECONDARY, Session, ble, ble_wave2::mac,
    contain, emit, halt, pac,
};
mod admission;
const MAGIC: u32 = 0xfac70007;
const REASON: u32 = 0x687725;

pub(super) fn is_record(r: &[u32; 12]) -> bool {
    r[0] == MAGIC && r[1] == REASON && r[11] == !REASON
}
fn irq_enable_0(s: &Session<'_>) -> u32 {
    use pac::bt_mac::irq_enable_0 as r;
    let v = s.ble.bank.read_irq_enable_0();
    (u32::from(v.source_0()) << r::SOURCE_0_MASK.trailing_zeros())
        | (u32::from(v.source_1()) << r::SOURCE_1_MASK.trailing_zeros())
        | (u32::from(v.source_8()) << r::SOURCE_8_MASK.trailing_zeros())
        | (u32::from(v.source_13()) << r::SOURCE_13_MASK.trailing_zeros())
        | (u32::from(v.source_15()) << r::SOURCE_15_MASK.trailing_zeros())
        | (u32::from(v.source_16()) << r::SOURCE_16_MASK.trailing_zeros())
        | (u32::from(v.source_21()) << r::SOURCE_21_MASK.trailing_zeros())
        | (v.sources_27_28().get() << r::SOURCES_27_28_MASK.trailing_zeros())
        | (v.bits_2_7_opaque().get() << r::BITS_2_7_OPAQUE_MASK.trailing_zeros())
        | (v.bits_9_12_opaque().get() << r::BITS_9_12_OPAQUE_MASK.trailing_zeros())
        | (u32::from(v.bit_14_opaque()) << r::BIT_14_OPAQUE_MASK.trailing_zeros())
        | (v.bits_17_20_opaque().get() << r::BITS_17_20_OPAQUE_MASK.trailing_zeros())
        | (v.bits_22_26_opaque().get() << r::BITS_22_26_OPAQUE_MASK.trailing_zeros())
        | (v.bits_29_31_opaque().get() << r::BITS_29_31_OPAQUE_MASK.trailing_zeros())
}
fn irq_enable_1(s: &Session<'_>) -> u32 {
    use pac::bt_mac::irq_enable_1 as r;
    let v = s.ble.bank.read_irq_enable_1();
    (u32::from(v.controller_time_overflow()) << r::CONTROLLER_TIME_OVERFLOW_MASK.trailing_zeros())
        | (u32::from(v.source_3()) << r::SOURCE_3_MASK.trailing_zeros())
        | (u32::from(v.source_4()) << r::SOURCE_4_MASK.trailing_zeros())
        | (u32::from(v.source_5()) << r::SOURCE_5_MASK.trailing_zeros())
        | (u32::from(v.source_6()) << r::SOURCE_6_MASK.trailing_zeros())
        | (u32::from(v.source_8()) << r::SOURCE_8_MASK.trailing_zeros())
        | (u32::from(v.source_9()) << r::SOURCE_9_MASK.trailing_zeros())
        | (u32::from(v.source_12()) << r::SOURCE_12_MASK.trailing_zeros())
        | (u32::from(v.source_13()) << r::SOURCE_13_MASK.trailing_zeros())
        | (u32::from(v.bit_0_opaque()) << r::BIT_0_OPAQUE_MASK.trailing_zeros())
        | (u32::from(v.bit_2_opaque()) << r::BIT_2_OPAQUE_MASK.trailing_zeros())
        | (u32::from(v.bit_7_opaque()) << r::BIT_7_OPAQUE_MASK.trailing_zeros())
        | (v.bits_10_11_opaque().get() << r::BITS_10_11_OPAQUE_MASK.trailing_zeros())
        | (v.bits_14_31_opaque().get() << r::BITS_14_31_OPAQUE_MASK.trailing_zeros())
}
fn interrupt_enable(s: &Session<'_>) -> u32 {
    use pac::bt_mac::interrupt_enable as r;
    let v = s.ble.task_irq.read_interrupt_enable();
    (u32::from(v.scheduler_run()) << r::SCHEDULER_RUN_MASK.trailing_zeros())
        | (u32::from(v.source_17()) << r::SOURCE_17_MASK.trailing_zeros())
        | (v.bits_0_13_opaque().get() << r::BITS_0_13_OPAQUE_MASK.trailing_zeros())
        | (v.bits_15_16_opaque().get() << r::BITS_15_16_OPAQUE_MASK.trailing_zeros())
        | (v.bits_18_31_opaque().get() << r::BITS_18_31_OPAQUE_MASK.trailing_zeros())
}
pub(super) fn state(s: &Session<'_>, tag: u32) {
    emit(tag, irq_enable_0(s), irq_enable_1(s), interrupt_enable(s));
    // Dynamic groups are fields in those same primary words, not another bank.
    let a = s.ble.bank.read_irq_enable_0();
    let b = s.ble.bank.read_irq_enable_1();
    emit(
        tag + 1,
        u32::from(a.source_21()),
        a.sources_27_28().get(),
        u32::from(b.source_3()),
    );
    emit(
        tag + 2,
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
        tag + 3,
        s.ble.bank.read_irq_status_snapshot_0().pending_bits(),
        s.ble.bank.read_irq_status_snapshot_1().pending_bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    let c = s.sar.radio.hp_sys_clkrst_radio.read_modem_conf();
    let r = s.sar.radio.modem_syscon_radio.read_modem_rst_conf();
    emit(
        tag + 4,
        u32::from(c.modem_apb_clk_en()),
        u32::from(r.rst_btmac()),
        u32::from(r.rst_btmac_apb()),
    );
}
pub(super) fn inherited(s: &mut Session<'_>) {
    // No peripheral write precedes this gated diagnostic. ROM loading already
    // occurred; even after a power cycle this is not a pristine POR sample.
    state(s, 0x2100);
    contain(s);
    ble::clocks(s);
    state(s, 0x2110);
    emit(
        0x211f,
        s.reset_cause
            .lp_aon_reset_cause
            .read_hpcore_reset_cause(pac::BOOT_HART.index())
            .hpcore_reset_cause()
            .get(),
        0,
        0,
    );
}
pub(super) fn admission(s: &mut Session<'_>) {
    contain(s);
    ble::clocks(s);
    state(s, 0x2120);
    admission::run(s);
    state(s, 0x2150);
}
fn seed_enables(s: &mut Session<'_>) {
    // SOURCE: BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027 and
    // BLOB_S31_BLE_TASK_IRQ_INHERITANCE_20261027, P2 modeled-lane recipe.
    // No claim that production or ROM supplies these synthetic values.
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_0(true)
            .set_source_1(true)
            .set_source_8(true)
            .set_source_13(true)
            .set_source_16(true)
            .set_bits_2_7_opaque(Bits::constant::<1>())
            .set_source_21(true)
            .set_sources_27_28(Bits::constant::<3>());
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_bit_0_opaque(true)
            .set_bit_2_opaque(true)
            .set_source_4(true)
            .set_source_5(true)
            .set_source_6(true)
            .set_source_13(true)
            .set_source_3(true);
    });
    s.ble.task_irq.modify_interrupt_enable(|w| {
        w.set_source_17(true).set_scheduler_run(true);
    });
    oer_riscv::fence();
}
pub(super) fn software_reset(s: &mut Session<'_>) {
    contain(s);
    ble::clocks(s);
    seed_enables(s);
    state(s, 0x2160);
    // SOURCE: production fatal::reset_recording -> cores::prepare_reset,
    // software_reset; same held-secondary successful register-effect order.
    s.uart0_clock.hp_sys_clkrst_uart0.modify_uart0_ctrl0(|w| {
        w.set_reg_uart0_clk_src_sel(Bits::constant::<0>())
            .set_reg_uart0_sclk_div_num(0)
            .set_reg_uart0_sclk_div_numerator(0)
            .set_reg_uart0_sclk_div_denominator(0)
            .set_reg_uart0_clk_en(true);
    });
    s.core_control
        .lp_sys_core_control
        .modify_boot_addr_hp_core(SECONDARY.index(), |w| {
            w.set_boot_addr_hp_core(0);
        });
    s.core_control
        .lp_aon_clkrst_core_reset
        .trigger_hpcore_reset_ctrl_hpcore_sw_reset(SECONDARY.hart().index());
    s.core_control.pmu_cpu_stall.modify_cpu_stall_sw(|w| {
        w.set_hpcore_sw_stall_code(SECONDARY.hart().index(), 0x86);
    });
    RETAINED[0].store(0, Ordering::Relaxed);
    for (word, value) in RETAINED.iter().skip(1).zip([
        REASON,
        irq_enable_0(s),
        irq_enable_1(s),
        interrupt_enable(s),
        oer_riscv::cycle32(),
        0,
        0,
        0,
        0,
        0,
        !REASON,
    ]) {
        word.store(value, Ordering::Relaxed);
    }
    RETAINED[0].store(MAGIC, Ordering::Release);
    FACTS_OUTPUT[0].store(MAGIC, Ordering::Release);
    oer_riscv::fence();
    s.core_control
        .lp_sys_core_control
        .trigger_sys_ctrl_sys_sw_rst();
    halt()
}
pub(super) fn after_reset(s: &mut Session<'_>) {
    // Automatically selected before contain or any peripheral write after
    // ROM MEM_END. No host reset may occur between the two loader entries.
    state(s, 0x2170);
    let r = super::retained_record();
    for (i, row) in r.chunks_exact(3).enumerate() {
        if let [a, b, c] = row {
            emit(0x2180 + i as u32, *a, *b, *c);
        }
    }
    emit(
        0x218f,
        u32::from(is_record(&r)),
        s.reset_cause
            .lp_aon_reset_cause
            .read_hpcore_reset_cause(pac::BOOT_HART.index())
            .hpcore_reset_cause()
            .get(),
        0,
    );
    RETAINED[0].store(0, Ordering::Release);
    if !is_record(&r) {
        return;
    }
    contain(s);
    ble::clocks(s);
    state(s, 0x2190);
}
fn collision(s: &mut Session<'_>, kind: u32) {
    // SOURCE: HW9_S31_MAC_CAUSES_20261027 / HW12_S31_BLE_OVERFLOW_ACK_20261026.
    if !mac::prepare(s) {
        emit(0x21ee, 1, kind, 0);
        mac::finish(s);
        return;
    }
    mac::enable(s, 0, true);
    mac::enable(s, 1, true);
    mac::enable(s, 2, true);
    state(s, 0x21a0);
    if kind != 0 && !mac::produce(s, 0, 0x21a8) {
        emit(0x21ee, 2, kind, 0);
        mac::finish(s);
        return;
    }
    let saved0 = s.ble.bank.sample_bluetooth_primary_interrupt_bank_0();
    let saved1 = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
    emit(0x21b0, saved0.bits(), saved1.bits(), oer_riscv::cycle32());
    let cause = match kind {
        0 | 2 => 0,
        1 => 1,
        _ => 2,
    };
    if !mac::produce(s, cause, 0x21b8) {
        emit(0x21ee, 3, kind, cause);
        mac::finish(s);
        return;
    }
    state(s, 0x21c0);
    let before = oer_riscv::cycle32();
    s.ble
        .bank
        .acknowledge_bluetooth_primary_interrupt_bank_0(saved0);
    s.ble
        .bank
        .acknowledge_bluetooth_primary_interrupt_bank_1(saved1);
    oer_riscv::fence();
    emit(0x21c8, before, oer_riscv::cycle32(), kind);
    state(s, 0x21d0);
    // Exact set/clear coincidence is not measured: the new positive must be
    // visible before the saved W1C. Fresh same-bit recurrence is a control.
    mac::clear(s);
    if mac::produce(s, cause, 0x21d8) {
        state(s, 0x21e0);
    } else {
        emit(0x21ee, 4, kind, cause);
    }
    mac::finish(s);
}
pub(super) fn empty_arrival(s: &mut Session<'_>) {
    collision(s, 0);
}
pub(super) fn different_arrival(s: &mut Session<'_>) {
    collision(s, 1);
}
pub(super) fn same_arrival(s: &mut Session<'_>) {
    collision(s, 2);
}
pub(super) fn overflow_arrival(s: &mut Session<'_>) {
    collision(s, 3);
}
pub(super) fn retention(s: &mut Session<'_>) {
    if !mac::prepare(s) {
        emit(0x21ee, 1, 218, 0);
        mac::finish(s);
        return;
    }
    seed_enables(s);
    mac::enable(s, 0, true);
    if !mac::produce(s, 0, 0x220f) {
        emit(0x21ee, 2, 218, 0);
        mac::finish(s);
        return;
    }
    state(s, 0x2200);
    // SOURCE: HAL registers/phy.rs::initialize_ble_phy_registers, exact
    // initial task-bank mask/clear only; no claim of full PHY initialization.
    s.ble.task_irq.modify_interrupt_enable(|w| {
        w.set_scheduler_run(false)
            .set_bits_0_13_opaque(Bits::constant::<0>())
            .set_bits_15_16_opaque(Bits::constant::<0>())
            .set_bits_18_31_opaque(Bits::constant::<0>());
    });
    s.ble.task_irq.trigger_interrupt_clear(|t| {
        t.bits_0_13_opaque(Bits::constant::<0x3fff>())
            .scheduler_run()
            .bits_15_31_opaque(Bits::constant::<0x1ffff>());
    });
    state(s, 0x2210);
    // SOURCE: HAL interrupt/retirement.rs mask_dynamic -> disable_run ->
    // clear_dynamic -> execute_primary_release, on an idle/no-list probe.
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_21(false)
            .set_sources_27_28(Bits::constant::<0>());
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_source_3(false);
    });
    oer_riscv::fence();
    s.ble.task_irq.modify_interrupt_enable(|w| {
        w.set_scheduler_run(false);
    });
    oer_riscv::fence();
    s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_0();
    s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_1();
    s.ble.bank.clear_bluetooth_controller_time_overflow();
    mac::command(s, 1);
    mac::command(s, 2);
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_15(false);
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_controller_time_overflow(false)
            .set_source_8(false)
            .set_source_9(false)
            .set_source_12(false);
    });
    oer_riscv::fence();
    state(s, 0x2220);
    mac::clear(s);
    if mac::produce(s, 0, 0x223f) {
        state(s, 0x2230);
    } else {
        emit(0x21ee, 3, 218, 0);
    }
    mac::finish(s);
}
pub(super) fn finish(s: &mut Session<'_>) {
    mac::finish(s);
    s.ble.task_irq.modify_interrupt_enable(|w| {
        w.set_source_17(false).set_scheduler_run(false);
    });
    state(s, 0x2240);
}
