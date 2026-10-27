//! PB-1d: command correlations, not a peripheral interrupt/acknowledge contract.
#![forbid(unsafe_code)]
use super::*;
use crate::image::irq_observer;

#[derive(Clone, Copy, Default)]
struct Sample {
    cycle: u32,
    host0: u32,
    host1: u32,
    busy: u32,
    matrix3: u32,
    matrix4: u32,
    status0: u32,
    status1: u32,
    burst: u32,
    ip: u32,
}
fn sample(s: &Session<'_>, start: u32) -> Sample {
    let m = &s.sar.analog.i2c_ana_mst;
    let h0 = m.read_i2c0_ctrl();
    let h1 = m.read_i2c1_ctrl();
    let b = m.read_i2c_burst_status();
    Sample {
        cycle: oer_riscv::cycle32().wrapping_sub(start),
        host0: u32::from(h0.data()),
        host1: u32::from(h1.data()),
        busy: u32::from(h0.i2c0_busy()) | (2 * u32::from(h1.i2c1_busy())),
        matrix3: s
            .interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
        matrix4: s
            .interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
        status0: u32::from(m.read_i2c0_conf().i2c0_status()),
        status1: u32::from(m.read_i2c1_conf().i2c1_status()),
        burst: u32::from(b.i2c_mst_burst_done())
            | (2 * u32::from(b.i2c_mst0_burst_err_flag()))
            | (4 * u32::from(b.i2c_mst1_burst_err_flag())),
        ip: irq_observer::pending(s),
    }
}
fn record(v: Sample, tag: u32) {
    emit(tag, v.cycle, v.busy, v.host0 | v.host1 << 8);
    emit(tag + 1, v.matrix3, v.matrix4, v.ip);
    emit(tag + 2, v.status0, v.status1, v.burst);
}
fn begin(s: &mut Session<'_>) -> bool {
    irq_observer::inherited(s);
    if !irq_observer::control(s) || !super::super::opaque::xtal_cpu(s) {
        emit(0x34ee, 1, 0, 0);
        return false;
    }
    prepare(s);
    irq_observer::route(s, pac::Interrupt::MODEM_WIFI_BB, crate::image::LINE as u32);
    irq_observer::route(
        s,
        pac::Interrupt::MODEM_I2C_MST,
        (crate::image::LINE + 1) as u32,
    );
    irq_observer::clear(s);
    emit(
        0x3400,
        s.sar
            .analog
            .i2c_ana_mst
            .read_i2c_burst_conf()
            .i2c_mst_burst_ctrl(),
        s.sar
            .analog
            .i2c_ana_mst
            .read_i2c_burst_status()
            .i2c_mst_burst_timeout_cnt()
            .get(),
        0,
    );
    true
}
fn reset_busy(s: &mut Session<'_>) -> u32 {
    let mut published = 0;
    // SOURCE: ROM phy_i2c_master_reset, BX12_S31_I2C_HOST_PUBLICATION_20261027.
    // These are only the conditionally busy hosts; no analog write or stuck-host injection.
    if busy(s, Host::Host0) {
        published |= 1;
        s.sar.analog.i2c_ana_mst.write_i2c0_ctrl(|w| {
            w.set_start_or_reset(true);
        });
    }
    if busy(s, Host::Host1) {
        published |= 2;
        s.sar.analog.i2c_ana_mst.write_i2c1_ctrl(|w| {
            w.set_start_or_reset(true);
        });
    }
    published
}
fn command(s: &mut Session<'_>, commands: &[(Host, Address, u8)], reset: bool, tag: u32) -> bool {
    if busy(s, Host::Host0) || busy(s, Host::Host1) {
        emit(0x34ee, 2, tag, 0);
        return false;
    }
    irq_observer::clear(s);
    let start_cycle = oer_riscv::cycle32();
    let before = sample(s, start_cycle);
    for &(host, address, seed) in commands {
        start(s, host, address, seed);
    }
    // Keep the conditional busy read adjacent to the bit26 store. The broad
    // status snapshot otherwise outlives an ordinary command's BUSY epoch.
    let input_before = s
        .interrupt_matrix
        .interrupt_core0_status
        .read_int_status_reg_4()
        .int_status_4();
    let reset_mask = if reset { reset_busy(s) } else { 0 };
    let input_after = s
        .interrupt_matrix
        .interrupt_core0_status
        .read_int_status_reg_4()
        .int_status_4();
    let first = sample(s, start_cycle);
    let after_reset = sample(s, start_cycle);
    let mut last = after_reset;
    let mut matrix3 = first.matrix3 | after_reset.matrix3;
    let mut matrix4 = first.matrix4 | after_reset.matrix4;
    let mut seen_busy = first.busy | after_reset.busy | reset_mask;
    let mut polls = 2u32;
    while last.busy != 0 && oer_riscv::cycle32().wrapping_sub(start_cycle) < COMMAND_CYCLES {
        last = sample(s, start_cycle);
        matrix3 |= last.matrix3;
        matrix4 |= last.matrix4;
        seen_busy |= last.busy;
        polls = polls.saturating_add(1);
    }
    record(before, tag);
    record(first, tag + 3);
    record(after_reset, tag + 6);
    record(last, tag + 9);
    emit(tag + 12, seen_busy, reset_mask, polls);
    if reset {
        emit(0x34f0, input_before, input_after, reset_mask);
    }
    emit(tag + 13, matrix3, matrix4, irq_observer::pending(s));
    for &(host, address, seed) in commands {
        emit(
            tag + 14,
            host_number(host),
            u32::from(address.block()) | u32::from(address.index()) << 8,
            u32::from(seed),
        );
    }
    if last.busy != 0 {
        emit(0x34ee, 3, tag, last.busy);
        return false;
    }
    true
}
fn serial_observe(s: &mut Session<'_>, address: Address, seed: u8, reset: bool, tag: u32) -> bool {
    let Some(r) = analog_i2c::route(Profile::LibphyNormal, address.block()) else {
        emit(0x34ee, 4, tag, 0);
        return false;
    };
    normal_map(s);
    s.sar
        .analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(r.complemented_read_select()));
    routing(s, tag + 15, r.host, seed);
    command(s, &[(r.host, address, seed)], reset, tag)
}
pub(in crate::image) fn reads(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    let b = analog_i2c::bbtop::byte_04_opaque::ADDRESS;
    let t = analog_i2c::txrf::byte_08_opaque::ADDRESS;
    if !serial_observe(s, b, 0xa5, false, 0x3500) || !serial_observe(s, t, 0x5a, false, 0x3520) {
        return;
    }
    // SOURCE: the admitted ROM parallel-map transaction, as in wave2.
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_txrf_mst0_select(true);
    });
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_bbtop_mst0_select(true);
    });
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_txrf_mst0_select(false);
    });
    let Some(br) = analog_i2c::route(Profile::LibphyNormal, b.block()) else {
        return;
    };
    let Some(tr) = analog_i2c::route(Profile::LibphyNormal, t.block()) else {
        return;
    };
    s.sar
        .analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(
            !(u32::from(br.read_select) | u32::from(tr.read_select)),
        ));
    if !command(
        s,
        &[(Host::Host0, b, 0x3c), (Host::Host1, t, 0xc3)],
        false,
        0x3540,
    ) {
        return;
    }
    if !serial_observe(s, b, 0x5a, false, 0x3560) || !serial_observe(s, t, 0xa5, false, 0x3580) {
        return;
    }
    irq_observer::inherited(s);
    emit(0x34ff, 5, 0, 0);
}
pub(in crate::image) fn reset(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    let b = analog_i2c::bbtop::byte_04_opaque::ADDRESS;
    if !serial_observe(s, b, 0xa5, false, 0x35a0) || !serial_observe(s, b, 0x5a, true, 0x35c0) {
        return;
    }
    let _ = serial_observe(s, b, 0x3c, false, 0x35e0);
}
