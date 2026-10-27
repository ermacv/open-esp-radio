//! PB-1e and the unpublished PB-1f RX descriptor fixture.
//! SOURCE: oer-notes/research/bx-{tsens,fe-est-freq,pmu-ck-power,wifi-control0-rate}.md.
//! Every TSENS selection requires its own power-on/download loader session.
use super::{Bits, Index, Session, contain, emit, opaque, opaque_wave2, pac};

fn begin(s: &mut Session<'_>) {
    oer_riscv::disable_interrupts();
    contain(s);
    // SOURCE: the existing facts-image BLE leaves retain/feed the ROM WDT.
    let wdt = &mut s.ble.watchdog;
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    wdt.trigger_feed_feed();
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
}

fn wait(us: u32) {
    let start = oer_riscv::cycle32();
    until(start, us);
}

fn until(start: u32, us: u32) {
    while oer_riscv::cycle32().wrapping_sub(start) < us.saturating_mul(40) {}
}

fn state(s: &Session<'_>, tag: u32) {
    let lp = s.sar.radio.lp_periclkrst.read_tsens_ctrl();
    let power = s.sar.radio.pmu_radio.read_ana_peri_pwr_ctrl();
    emit(
        tag,
        s.sar.radio.lp_tsens.read_tsens_control_image(),
        s.sar.radio.lp_tsens.read_tsens_clock_image(),
        u32::from(lp.lp_tsens_clk_en())
            | (u32::from(lp.lp_tsens_rst_en()) << 1)
            | (u32::from(power.xpd_perif_i2c()) << 2)
            | (u32::from(power.rstb_perif_i2c()) << 3),
    );
}

fn dac(s: &mut Session<'_>, value: u8) -> Option<u32> {
    let field = pac::analog_i2c::saradc::i2c_saradc_tsens_dac::I2C_SARADC_TSENS_DAC;
    let old = s.sar.analog_byte(field.address(), None)?;
    let new = field.insert(old, value)?;
    s.sar.analog_byte(field.address(), Some(new))?;
    let end = oer_riscv::cycle32();
    emit(0x3102, value.into(), old.into(), new.into());
    Some(end)
}

fn verify_dac(s: &mut Session<'_>, value: u8) -> bool {
    let field = pac::analog_i2c::saradc::i2c_saradc_tsens_dac::I2C_SARADC_TSENS_DAC;
    let observed = s.sar.analog_byte(field.address(), None);
    emit(
        0x3104,
        value.into(),
        observed.map_or(u32::MAX, u32::from),
        0,
    );
    observed.is_some_and(|byte| field.extract(byte) == value)
}

fn lane(s: &mut Session<'_>, lanes: u32, value: bool) {
    // Distinct fresh-read RMWs retain the vendor bit23 then bit9 order.
    if lanes == 1 || lanes == 3 {
        s.sar.radio.lp_tsens.modify_clk_conf(|w| {
            w.set_bit_23_opaque(value);
        });
    }
    if lanes == 2 || lanes == 3 {
        s.sar.radio.lp_tsens.modify_clk_conf(|w| {
            w.set_bit_9_opaque(value);
        });
    }
}

fn prepare(s: &mut Session<'_>, lanes: u32) -> bool {
    begin(s);
    if !opaque::xtal_cpu(s) {
        return false;
    }
    if !s.sar.clock() {
        return false;
    }
    state(s, 0x3100);
    // Existing short transport prefix, without PHY calibration or TSENS writes.
    opaque_wave2::prepare(s);
    // SOURCE: existing P-TSENS SAR-I2C power/reset and SDK named TSENS reset.
    s.sar.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
        w.set_rstb_perif_i2c(false);
    });
    wait(1);
    s.sar.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(true).set_rstb_perif_i2c(true);
    });
    s.sar.radio.lp_periclkrst.modify_tsens_ctrl(|w| {
        w.set_lp_tsens_clk_en(pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Enabled)
            .set_lp_tsens_rst_en(true);
    });
    s.sar.radio.lp_periclkrst.modify_tsens_ctrl(|w| {
        w.set_lp_tsens_rst_en(false);
    });
    s.sar.radio.lp_tsens.modify_clk_conf(|w| {
        w.set_clk_en(true);
    });
    lane(s, lanes, true);
    s.sar.radio.lp_tsens.modify_ctrl(|w| {
        w.set_power_up(pac::tsens::ctrl::PowerUp::Enabled)
            .set_power_up_force(true)
            .set_clk_div(15)
            .set_sample_en(true);
    });
    state(s, 0x3101);
    if dac(s, 15).is_none() {
        emit(0x31ee, 3, 15, 0);
        return false;
    }
    wait(500);
    state(s, 0x3103);
    verify_dac(s, 15)
}

fn sample(s: &Session<'_>, tag: u32, start: u32) {
    let before = oer_riscv::cycle32();
    let control = s.sar.radio.lp_tsens.read_tsens_control_image();
    let clock = s.sar.radio.lp_tsens.read_tsens_clock_image();
    emit(tag, before.wrapping_sub(start), control, clock);
}

fn injection_at(s: &mut Session<'_>, lanes: u32, pattern: u32, us: u32, start: u32) {
    if (us == 20 && pattern != 0) || (us == 70 && pattern == 3) || (us == 520 && pattern == 4) {
        let before = oer_riscv::cycle32();
        lane(s, lanes, true);
        if pattern == 2 {
            lane(s, lanes, false);
        }
        emit(
            0x31c0,
            before.wrapping_sub(start),
            oer_riscv::cycle32().wrapping_sub(start),
            s.sar.radio.lp_tsens.read_tsens_clock_image(),
        );
    }
}

fn step(s: &mut Session<'_>, value: u8, lanes: u32, pattern: u32, inject: bool) -> bool {
    let Some(start) = dac(s, value) else {
        emit(0x31ee, 4, value.into(), 0);
        return false;
    };
    // Reference is the completed DAC write, before publication/readback.
    let tag = if value == 11 { 0x3111 } else { 0x3115 };
    for us in [0, 1, 2, 5, 10, 20] {
        until(start, us);
        sample(s, tag, start);
        if inject {
            injection_at(s, lanes, pattern, us, start);
        }
    }
    let end = if inject { 530 } else { 500 };
    for us in (30..=end).step_by(10) {
        until(start, us);
        sample(s, tag, start);
        if inject {
            injection_at(s, lanes, pattern, us, start);
        }
    }
    verify_dac(s, value)
}

fn cold(s: &mut Session<'_>, lanes: u32) {
    emit(0x31a0, lanes, 0, 0);
    if prepare(s, lanes) && step(s, 11, 0, 0, false) {
        step(s, 15, 0, 0, false);
    }
    state(s, 0x31ff);
}

pub(super) fn cold_none(s: &mut Session<'_>) {
    cold(s, 0);
}
pub(super) fn cold_23(s: &mut Session<'_>) {
    cold(s, 1);
}
pub(super) fn cold_9(s: &mut Session<'_>) {
    cold(s, 2);
}
pub(super) fn cold_pair(s: &mut Session<'_>) {
    cold(s, 3);
}

/// Patterns: no write, one, one/zero, repeat at +50 us, repeat at +500 us.
pub(super) fn injection<const LANES: u32, const PATTERN: u32>(s: &mut Session<'_>) {
    emit(0x31b0, LANES, PATTERN, 0);
    if prepare(s, 0) && step(s, 11, LANES, PATTERN, true) {
        step(s, 15, 0, 0, false);
    }
    state(s, 0x31ff);
}

pub(super) fn registers(s: &mut Session<'_>) {
    begin(s);
    if !opaque::xtal_cpu(s) {
        return;
    }
    let before = s.sar.radio.pmu_radio.read_hp_active_clock_power_image();
    s.sar.radio.pmu_radio.modify_hp_active_hp_ck_power(|w| {
        w.set_bits_0_3_opaque(Bits::constant::<15>())
            .set_hp_active_xpd_bb_i2c(true);
    });
    let after = s.sar.radio.pmu_radio.read_hp_active_clock_power_image();
    emit(0x3120, before, after, before ^ after);
    opaque::prepare(s);
    let index = Index::constant::<0>();
    let old = s.tx_control.read_control_n(index);
    let old_image = s.tx_control.read_tx_control0_image();
    emit(
        0x3121,
        old_image,
        u32::from(old.enable()),
        u32::from(old.valid()),
    );
    if old.enable() || old.valid() {
        emit(0x31ee, 5, old_image, 0);
        return;
    }
    for (case, pair, bit) in [(0, 0, false), (1, 2, false), (2, 2, true), (3, 0, true)] {
        s.tx_control.modify_control_n(index, |w| {
            w.set_bits_20_21_opaque(Bits::truncate(pair))
                .set_bit_22_opaque(bit);
        });
        emit(
            0x3122,
            case,
            pair | (u32::from(bit) << 2),
            s.tx_control.read_tx_control0_image(),
        );
    }
    // Restore the only changed lanes from the owned saved image, before
    // publishing completion. No descriptor or ENABLE/VALID is ever published.
    s.tx_control.modify_control_n(index, |w| {
        w.set_bits_20_21_opaque(old.bits_20_21_opaque())
            .set_bit_22_opaque(old.bit_22_opaque());
    });
    emit(0x3123, old_image, s.tx_control.read_tx_control0_image(), 0);
    let fe = &mut s.sar.radio.phy_frequency_channel_oracle_fe;
    let old = fe.read_frequency_control();
    // Hold autonomous updates and every strobe inactive for composition only.
    fe.modify_frequency_control(|w| {
        w.set_hardware_frequency_disable(
            pac::fe::frequency_control::HardwareFrequencyDisable::Software,
        )
        .set_channel_switch_pulse(false)
        .set_memory_write_pulse(false);
    });
    for (address, high) in [(0, false), (0x400, true)] {
        fe.modify_frequency_control(|w| {
            w.set_memory_address_low(Bits::constant::<0>())
                .set_memory_address_high(high);
        });
        emit(0x3124, address, fe.read_frequency_control_image(), 0);
        for channel in [1, 0xff] {
            fe.modify_frequency_control(|w| {
                w.set_channel_index(channel);
            });
            emit(
                0x3125,
                u32::from(channel),
                fe.read_frequency_control_image(),
                0,
            );
        }
    }
    fe.modify_frequency_control(|w| {
        w.set_channel_index(old.channel_index())
            .set_memory_address_low(old.memory_address_low())
            .set_memory_address_high(old.memory_address_high())
            .set_hardware_frequency_disable(
                pac::fe::frequency_control::HardwareFrequencyDisable::from_field(u32::from(
                    old.hardware_frequency_disable(),
                )),
            )
            .set_channel_switch_pulse(old.channel_switch_pulse())
            .set_memory_write_pulse(old.memory_write_pulse());
    });
    emit(0x3126, fe.read_frequency_control_image(), 0, 0);
}

pub(super) fn rxdesc(s: &mut Session<'_>) {
    begin(s);
    use pac::layouts::{WifiRxDmaDescriptor, WifiRxDmaDescriptorControl};
    // CPU-owned stack fixture: its address is never published to a peripheral.
    let descriptor = WifiRxDmaDescriptor::zeroed();
    let seed = WifiRxDmaDescriptorControl::ZERO
        .with_size_constant::<0x123>()
        .with_owner(false);
    for (case, offset, sosf) in [
        (0, false, false),
        (1, true, false),
        (2, false, true),
        (3, true, true),
    ] {
        descriptor
            .control()
            .write(seed.with_offset(offset).with_sosf(sosf));
        let read = descriptor.control().read();
        emit(
            0x3127,
            case,
            read.bits(),
            u32::from(read.offset()) | (u32::from(read.sosf()) << 1),
        );
    }
}
