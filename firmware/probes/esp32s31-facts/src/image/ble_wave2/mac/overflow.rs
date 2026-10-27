//! Isolated overflow acknowledgement, rate and APB-reset boundaries.
use super::{
    LIMIT, Session, clear, emit, enable, finish, group, pac, prepare, produce, sample, seed, state,
    window,
};

fn baseline(s: &mut Session<'_>) {
    // SOURCE: BLOB_LIBBTDM_BASELINE_IRQ_CURRENT: bank 0 baseline 0x8000,
    // bank 1 baseline 0x1302. Individual PAC fields preserve unknown lanes.
    group(s, 0, true);
    group(s, 1, true);
    enable(s, 2, true);
}

pub(in crate::image) fn overflow_ack(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    baseline(s);
    state(s, 0xa200);
    if !produce(s, 2, 0xa20f) {
        finish(s);
        return;
    }
    state(s, 0xa210);
    enable(s, 2, false);
    state(s, 0xa220);
    enable(s, 2, true);
    state(s, 0xa230);
    // SOURCE: BLOB_LIBBLE_NRT_SNAPSHOTS_CURRENT: copy +0x340/+0x348
    // to the corresponding shared clears, even when the snapshots are zero.
    let n0 = s.ble.bank.sample_bluetooth_interrupt_bank_0();
    let n1 = s.ble.bank.sample_bluetooth_interrupt_bank_1();
    emit(0xa23f, n0.bits(), n1.bits(), 0);
    s.ble.bank.acknowledge_bluetooth_interrupt_bank_0(n0);
    s.ble.bank.acknowledge_bluetooth_interrupt_bank_1(n1);
    oer_riscv::fence();
    state(s, 0xa240);
    let primary = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
    emit(0xa24f, primary.bits(), 0, 0);
    // The affine status image writes exactly its sampled bits to CLEAR_1.
    s.ble
        .bank
        .acknowledge_bluetooth_primary_interrupt_bank_1(primary);
    oer_riscv::fence();
    state(s, 0xa250);
    window(4096);
    state(s, 0xa260);
    finish(s);
}

fn timed(s: &mut Session<'_>, tag: u32) -> Option<u32> {
    let before = oer_riscv::cycle32();
    let value = sample(s);
    let after = oer_riscv::cycle32();
    emit(tag, before, after, value.unwrap_or(0));
    emit(tag + 1, u32::from(value.is_some()), 0, 0);
    value
}

pub(in crate::image) fn overflow_rate(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    baseline(s);
    let c = s.ble.core.read_btlc_timer_control();
    emit(
        0xa300,
        c.clock_shift().get(),
        u32::from(c.config_24()),
        u32::from(c.timer_arm()),
    );
    let cpu = s.sar.boot.hp_sys_clkrst_boot_clock.read_cpu_freq_ctrl0();
    emit(
        0xa301,
        u32::from(cpu.reg_cpu_clk_div_num()),
        cpu.reg_cpu_clk_div_numerator().get(),
        cpu.reg_cpu_clk_div_denominator().get(),
    );
    let m = s.sar.radio.hp_sys_clkrst_radio.read_modem_conf();
    emit(
        0xa302,
        s.sar
            .boot
            .hp_sys_clkrst_boot_clock
            .read_soc_clk_sel()
            .reg_soc_clk_sel()
            .get(),
        u32::from(m.modem_clk_source_sel()),
        u32::from(m.modem_pll_clk_en()),
    );
    let pll = s.sar.boot.lp_aon_clkrst_cpll.read_cpll_div();
    emit(
        0xa303,
        u32::from(pll.cpll_fb_div()),
        pll.cpll_ref_div().get(),
        0,
    );
    // One rate observation, not a qualification series. Both capture instants
    // lie within their mcycle brackets; the host derives a ratio interval.
    if timed(s, 0xa310).is_none() {
        finish(s);
        return;
    }
    window(4_800_000);
    if timed(s, 0xa320).is_none() {
        finish(s);
        return;
    }
    seed(s, 0xfffffc00);
    let Some(origin) = timed(s, 0xa330) else {
        finish(s);
        return;
    };
    if origin < 0xfffffc00 {
        emit(0xa33f, 0, origin, 0);
        finish(s);
        return;
    }
    state(s, 0xa340);
    let start = oer_riscv::cycle32();
    let mut crossed = false;
    while oer_riscv::cycle32().wrapping_sub(start) < LIMIT {
        let Some(now) = sample(s) else { break };
        if now < origin {
            emit(0xa350, origin, now, oer_riscv::cycle32());
            crossed = true;
            break;
        }
    }
    emit(0xa351, u32::from(crossed), 0, 0);
    state(s, 0xa360);
    clear(s);
    finish(s);
}

// Emit every modeled lane separately: no hand-written register packing.
fn enables(s: &Session<'_>, tag: u32) {
    let a = s.ble.bank.read_irq_enable_0();
    emit(
        tag,
        u32::from(a.source_0()),
        u32::from(a.source_1()),
        u32::from(a.source_8()),
    );
    emit(
        tag + 1,
        u32::from(a.source_13()),
        u32::from(a.source_15()),
        u32::from(a.source_16()),
    );
    emit(
        tag + 2,
        u32::from(a.source_21()),
        a.sources_27_28().get(),
        a.bits_2_7_opaque().get(),
    );
    emit(
        tag + 3,
        a.bits_9_12_opaque().get(),
        u32::from(a.bit_14_opaque()),
        a.bits_17_20_opaque().get(),
    );
    emit(
        tag + 4,
        a.bits_22_26_opaque().get(),
        a.bits_29_31_opaque().get(),
        0,
    );
    let b = s.ble.bank.read_irq_enable_1();
    emit(
        tag + 5,
        u32::from(b.controller_time_overflow()),
        u32::from(b.source_3()),
        u32::from(b.source_4()),
    );
    emit(
        tag + 6,
        u32::from(b.source_5()),
        u32::from(b.source_6()),
        u32::from(b.source_8()),
    );
    emit(
        tag + 7,
        u32::from(b.source_9()),
        u32::from(b.source_12()),
        u32::from(b.source_13()),
    );
    emit(
        tag + 8,
        u32::from(b.bit_0_opaque()),
        u32::from(b.bit_2_opaque()),
        u32::from(b.bit_7_opaque()),
    );
    emit(
        tag + 9,
        b.bits_10_11_opaque().get(),
        b.bits_14_31_opaque().get(),
        0,
    );
}

fn boundary(s: &Session<'_>, tag: u32) {
    state(s, tag);
    enables(s, tag + 5);
}

pub(in crate::image) fn overflow_reset(s: &mut Session<'_>) {
    if !prepare(s) {
        finish(s);
        return;
    }
    baseline(s);
    if !produce(s, 2, 0xa40f) {
        finish(s);
        return;
    }
    boundary(s, 0xa410);
    // SOURCE: production Bluetooth shutdown reset order, MAC then APB.
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Released);
        });
    oer_riscv::fence();
    boundary(s, 0xa430);
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_btmac_apb(pac::modem_syscon::modem_rst_conf::RstBtmacApb::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_btmac_apb(pac::modem_syscon::modem_rst_conf::RstBtmacApb::Released);
        });
    oer_riscv::fence();
    boundary(s, 0xa450);
    // Re-enable before any clear/reseed: a retained pending cause would become
    // visible again if reset merely hid it behind zero enable registers.
    baseline(s);
    boundary(s, 0xa470);
    window(4096);
    boundary(s, 0xa490);
    super::command(s, 0);
    boundary(s, 0xa4b0);
    if produce(s, 2, 0xa4cf) {
        state(s, 0xa4d0);
        clear(s);
    }
    finish(s);
}
