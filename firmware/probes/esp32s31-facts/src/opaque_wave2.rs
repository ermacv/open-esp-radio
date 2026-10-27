//! Short OPAQUE wave-2 transport experiment. No analog byte is written.
//! SOURCE: oer-notes/research/opaque-blobray-wifi.md P-I2C.
use super::{Bits, Session, contain, emit, pac};
use pac::analog_i2c::{self, Address, Host, Profile};
use pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode;

// Probe-only command deadline: 1 ms at the verified 40 MHz CPU XTAL.
#[path = "opaque_wave2/observer.rs"]
pub(super) mod observer;

const COMMAND_CYCLES: u32 = 40_000;

fn settle(cycles: u32) {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < cycles {}
}

/// Probe-specific transport prefix. Its complete differences from production
/// power/phy_rf_init are listed in the README; no calibrated epoch is claimed.
pub(super) fn prepare(s: &mut Session<'_>) {
    contain(s);
    // SOURCE: hal/src/power/chip_clocks.rs ClockCounts::adopt/ClockWords.
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_wifi_power_clock(true);
    // SOURCE: hal/src/power.rs execute_owned; modem/ops.rs. Preserve its
    // PMU update -> APB -> maps -> source -> BB reset -> clocks order only.
    // MPLL reference counting and the separate RF-init graph are not run.
    s.sar.radio.pmu_radio.modify_hp_active_icg_modem(|w| {
        w.set_hp_active_dig_icg_modem_code(HpActiveDigIcgModemCode::Active);
    });
    s.sar
        .radio
        .pmu_radio
        .trigger_imm_modem_icg_update_dig_icg_modem_en();
    s.sar
        .radio
        .pmu_radio
        .trigger_imm_sleep_sysclk_update_dig_icg_switch();
    s.sar.radio.hp_sys_clkrst_radio.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled);
    });
    s.sar
        .radio
        .modem_syscon_radio
        .modify_clk_conf_power_st(|w| {
            w.set_clk_zb_st_map(Bits::truncate(w.clk_zb_st_map().get() | 4));
            w.set_clk_fe_st_map(Bits::truncate(w.clk_fe_st_map().get() | 6));
            w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
            w.set_clk_wifi_st_map(Bits::truncate(w.clk_wifi_st_map().get() | 6));
            w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
            w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
        });
    s.sar
        .radio
        .modem_lpcon_shared_clock
        .modify_clk_conf_power_st(|w| {
            w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
            w.set_clk_coex_st_map(Bits::truncate(w.clk_coex_st_map().get() | 6));
            w.set_clk_i2c_mst_st_map(Bits::truncate(w.clk_i2c_mst_st_map().get() | 6));
            w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
        });
    s.sar
        .clocks
        .hp_sys_clkrst_pll160
        .modify_ref_160m_ctrl0(|w| {
            w.set_reg_ref_160m_clk_en(true);
        });
    s.sar.radio.hp_sys_clkrst_radio.modify_modem_conf(|w| {
        w.set_modem_apb_clk_en(true)
            .set_modem_rst_en(false)
            .set_modem_clk_en(true)
            .set_modem_clk_source_sel(true)
            .set_modem_pll_clk_en(true)
            .set_modem_xtal_clk_en(true);
    });
    for reset in [
        pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted,
        pac::modem_syscon::modem_rst_conf::RstWifibb::Released,
    ] {
        s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
            w.set_rst_wifibb(reset);
        });
    }
    s.sar.radio.modem_syscon_radio.modify_clk_conf1(|w| {
        use pac::modem_syscon::clk_conf1::*;
        w.set_clk_wifibb_22m_en(ClkWifibb22mEn::Enabled)
            .set_clk_wifibb_40m_en(ClkWifibb40mEn::Enabled)
            .set_clk_wifibb_44m_en(ClkWifibb44mEn::Enabled)
            .set_clk_wifibb_80m_en(ClkWifibb80mEn::Enabled)
            .set_clk_wifibb_40x_en(ClkWifibb40xEn::Enabled)
            .set_clk_wifibb_80x_en(ClkWifibb80xEn::Enabled)
            .set_clk_wifibb_40x1_en(ClkWifibb40x1En::Enabled)
            .set_clk_wifibb_80x1_en(ClkWifibb80x1En::Enabled)
            .set_clk_wifibb_160x1_en(ClkWifibb160x1En::Enabled)
            .set_clk_wifi_apb_en(ClkWifiApbEn::Enabled)
            .set_clk_bt_apb_en(ClkBtApbEn::Enabled)
            .set_clk_btbb_en(ClkBtbbEn::Enabled)
            .set_clk_fe_80m_en(true)
            .set_clk_fe_160m_en(true)
            .set_clk_fe_apb_en(true)
            .set_clk_fe_pwdet_adc_en(true)
            .set_clk_fe_adc_en(true)
            .set_clk_fe_dac_en(true);
    });
    s.sar.radio.modem_syscon_radio.modify_clk_conf(|w| {
        w.set_clk_i2c_mst_sel_160m(pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled);
    });
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_i2c_ana_mst_master_clock(true);

    // SOURCE: hal/src/phy/analog_i2c.rs prepare_open_i2c_pre_delay and
    // complete_open_i2c_power_and_reset; modem/ops.rs. The false BB tie
    // deliberately has no write in production (write-trigger field).
    s.sar.radio.pmu_radio.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(pac::pmu::rf_pwc::XpdRfCircuit::PoweredOff);
    });
    settle(4_000); // sourced 100 us pre-delay, once, in the standalone probe
    s.sar.radio.pmu_radio.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(pac::pmu::rf_pwc::XpdRfCircuit::PoweredOn);
    });
    s.sar
        .radio
        .pmu_radio
        .trigger_imm_hp_ck_power_0_tie_high_xpd_bb_i2c();
    if !s
        .sar
        .radio
        .pmu_radio
        .read_ana_peri_pwr_ctrl()
        .xpd_perif_i2c()
    {
        s.sar.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_xpd_perif_i2c(true);
        });
        s.sar.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(false);
        });
        s.sar.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(true);
        });
    }
    if !s
        .sar
        .radio
        .pmu_radio
        .read_ana_peri_pwr_ctrl()
        .rstb_perif_i2c()
    {
        s.sar.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(true);
        });
    }
    // SOURCE: hal/src/phy/i2c/master.rs configure_clock_selection and
    // configure_master_registers (six timing RMWs, mode then enable).
    let m = &mut s.sar.analog.i2c_ana_mst;
    m.modify_i2c0_ctrl1(|w| {
        w.set_i2c0_sda_side_guard(Bits::constant::<2>());
    });
    m.modify_i2c0_ctrl1(|w| {
        w.set_i2c0_scl_pulse_dur(Bits::constant::<4>());
    });
    m.modify_i2c1_ctrl1(|w| {
        w.set_i2c1_sda_side_guard(Bits::constant::<2>());
    });
    m.modify_i2c1_ctrl1(|w| {
        w.set_i2c1_scl_pulse_dur(Bits::constant::<4>());
    });
    m.modify_hw_i2c_ctrl(|w| {
        w.set_hw_i2c_sda_side_guard(Bits::constant::<2>());
    });
    m.modify_hw_i2c_ctrl(|w| {
        w.set_hw_i2c_scl_pulse_dur(Bits::constant::<4>());
    });
    m.modify_ana_conf0(|w| {
        w.set_phy_register_mode(Bits::constant::<2>());
    });
    m.modify_ana_conf0(|w| {
        w.set_phy_register_enable(true);
    });
    oer_riscv::fence();
}

// Diagnostic packing above the TXRF lane retains the recorded result format.
// Every input is a typed getter; positions derive from the generated model.
fn upper_host_map_image(map: pac::i2c_ana_mst::ana_conf2::R) -> u32 {
    use pac::i2c_ana_mst::ana_conf2 as fields;
    [
        (
            u32::from(map.sdm_mst0_select()),
            fields::SDM_MST0_SELECT_MASK,
        ),
        (
            u32::from(map.rfpll_mst0_select()),
            fields::RFPLL_MST0_SELECT_MASK,
        ),
        (
            u32::from(map.bias_mst0_select()),
            fields::BIAS_MST0_SELECT_MASK,
        ),
        (
            u32::from(map.bbpll_mst0_select()),
            fields::BBPLL_MST0_SELECT_MASK,
        ),
        (
            u32::from(map.ulp_bod_mst0_select()),
            fields::ULP_BOD_MST0_SELECT_MASK,
        ),
        (
            map.bits_11_12_opaque().get(),
            fields::BITS_11_12_OPAQUE_MASK,
        ),
        (
            u32::from(map.saradc_mst0_select()),
            fields::SARADC_MST0_SELECT_MASK,
        ),
        (
            map.bits_14_16_opaque().get(),
            fields::BITS_14_16_OPAQUE_MASK,
        ),
        (
            u32::from(map.dig_reg_mst0_select()),
            fields::DIG_REG_MST0_SELECT_MASK,
        ),
    ]
    .into_iter()
    .fold(0, |image, (value, mask)| {
        image | value.wrapping_shl(mask.trailing_zeros())
    })
    .wrapping_shr(fields::SDM_MST0_SELECT_MASK.trailing_zeros())
}

// A case has route at +0, completion at +1, identity at +2,
// mode at +3 and request at +4. Refusals have separate global tags.
fn routing(s: &Session<'_>, tag: u32, host: Host, seed: u8) {
    let m = &s.sar.analog.i2c_ana_mst;
    let mask = m.read_ana_conf1();
    let map = m.read_ana_conf2();
    emit(
        tag,
        mask.ana_conf1().get()
            | (u32::from(mask.ana_status1())
                << pac::i2c_ana_mst::ana_conf1::ANA_STATUS1_MASK.trailing_zeros()),
        u32::from(map.bbtop_mst0_select()),
        u32::from(map.txrf_mst0_select()),
    );
    emit(
        tag + 3,
        upper_host_map_image(map),
        m.read_ana_conf0().phy_register_mode().get(),
        u32::from(m.read_ana_conf0().phy_register_enable()),
    );
    emit(tag + 4, host_number(host), u32::from(seed), 0);
}

fn host_number(host: Host) -> u32 {
    match host {
        Host::Host0 => 0,
        Host::Host1 => 1,
    }
}

fn busy(s: &Session<'_>, host: Host) -> bool {
    let m = &s.sar.analog.i2c_ana_mst;
    match host {
        Host::Host0 => m.read_i2c0_ctrl().i2c0_busy(),
        Host::Host1 => m.read_i2c1_ctrl().i2c1_busy(),
    }
}

fn start(s: &mut Session<'_>, host: Host, address: Address, seed: u8) {
    let m = &mut s.sar.analog.i2c_ana_mst;
    // SOURCE: hal/src/phy/i2c/master.rs publish_command; ROM_REV0_PHY_I2C.
    // Deliberate probe difference: nonzero DATA seeds distinguish a read
    // returning zero from an unchanged command. READ_WRITE is always false.
    match host {
        Host::Host0 => m.write_i2c0_ctrl(|w| {
            w.set_slave_addr(address.block())
                .set_slave_reg_addr(address.index())
                .set_data(seed)
                .set_read_write(false)
                .set_start_or_reset(true);
        }),
        Host::Host1 => m.write_i2c1_ctrl(|w| {
            w.set_slave_addr(address.block())
                .set_slave_reg_addr(address.index())
                .set_data(seed)
                .set_read_write(false)
                .set_start_or_reset(true);
        }),
    }
}

fn finish(s: &Session<'_>, host: Host, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    let mut polls = 0u32;
    while busy(s, host) {
        polls = polls.saturating_add(1);
        if oer_riscv::cycle32().wrapping_sub(start) >= COMMAND_CYCLES {
            emit(
                tag + 1,
                u32::MAX,
                polls,
                oer_riscv::cycle32().wrapping_sub(start),
            );
            return false;
        }
    }
    let m = &s.sar.analog.i2c_ana_mst;
    let (data, block, index) = match host {
        Host::Host0 => {
            let r = m.read_i2c0_ctrl();
            (r.data(), r.slave_addr(), r.slave_reg_addr())
        }
        Host::Host1 => {
            let r = m.read_i2c1_ctrl();
            (r.data(), r.slave_addr(), r.slave_reg_addr())
        }
    };
    emit(
        tag + 1,
        u32::from(data),
        polls,
        oer_riscv::cycle32().wrapping_sub(start),
    );
    emit(
        tag + 2,
        u32::from(block),
        u32::from(index),
        u32::from(busy(s, host)),
    );
    true
}

fn normal_map(s: &mut Session<'_>) {
    // SOURCE: hal/src/phy/i2c/master.rs configure_host_map, image 0x3fa0.
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_bbtop_mst0_select(false)
            .set_txrf_mst0_select(false)
            .set_sdm_mst0_select(false)
            .set_rfpll_mst0_select(false)
            .set_bias_mst0_select(false)
            .set_bbpll_mst0_select(true)
            .set_ulp_bod_mst0_select(false)
            .set_bits_11_12_opaque(Bits::constant::<3>())
            .set_saradc_mst0_select(true)
            .set_bits_14_16_opaque(Bits::constant::<7>())
            .set_dig_reg_mst0_select(true);
    });
}

fn read_case(s: &mut Session<'_>, address: Address, host: Host, seed: u8, tag: u32) -> bool {
    if busy(s, host) {
        emit(0x14d, tag, host_number(host), 0);
        return false;
    }
    routing(s, tag, host, seed);
    start(s, host, address, seed);
    finish(s, host, tag)
}

fn serial(s: &mut Session<'_>, address: Address, seed: u8, tag: u32) -> bool {
    let Some(route) = analog_i2c::route(Profile::LibphyNormal, address.block()) else {
        emit(
            0x14b,
            tag,
            u32::from(address.block()),
            u32::from(address.index()),
        );
        return false;
    };
    normal_map(s);
    s.sar
        .analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(route.complemented_read_select()));
    read_case(s, address, route.host, seed, tag)
}

fn lane_snapshot(s: &Session<'_>, tag: u32) {
    let m = s.sar.analog.i2c_ana_mst.read_ana_conf2();
    emit(
        tag,
        u32::from(m.bbtop_mst0_select()),
        u32::from(m.txrf_mst0_select()),
        upper_host_map_image(m),
    );
}

pub(super) fn i2c(s: &mut Session<'_>) {
    if !super::opaque::xtal_cpu(s) {
        emit(0x14c, 14, 0, 0);
        return;
    }
    emit(0x140, 0, 0, 0);
    prepare(s);
    let power = s.sar.radio.pmu_radio.read_ana_peri_pwr_ctrl();
    let gates = s.sar.clocks.modem_lpcon_chip_clocks.read_clk_conf();
    emit(
        0x141,
        u32::from(power.xpd_perif_i2c()),
        u32::from(power.rstb_perif_i2c()),
        u32::from(gates.clk_i2c_mst_en()),
    );
    emit(
        0x142,
        u32::from(
            s.sar
                .radio
                .modem_syscon_radio
                .read_clk_conf()
                .clk_i2c_mst_sel_160m(),
        ),
        u32::from(
            s.sar
                .clocks
                .hp_sys_clkrst_pll160
                .read_ref_160m_ctrl0()
                .reg_ref_160m_clk_en(),
        ),
        u32::from(
            s.sar
                .analog
                .i2c_ana_mst
                .read_ana_conf0()
                .phy_register_enable(),
        ),
    );
    let b = analog_i2c::bbtop::byte_04_opaque::ADDRESS;
    let t = analog_i2c::txrf::byte_08_opaque::ADDRESS;
    if !serial(s, b, 0xa5, 0x150) || !serial(s, t, 0xa5, 0x160) {
        return;
    }
    let busy0 = busy(s, Host::Host0);
    let busy1 = busy(s, Host::Host1);
    if busy0 || busy1 {
        emit(0x14e, 0x170, u32::from(busy0), u32::from(busy1));
        return;
    }
    // Probe preparation makes the ROM's second operation a real 1->0 edge.
    // No command executes while both banks are assigned to host zero.
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_txrf_mst0_select(true);
    });
    lane_snapshot(s, 0x143);
    // SOURCE: ROM_REV0_PHY_I2C_HOST_LANES: set BBTOP, then clear TXRF.
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_bbtop_mst0_select(true);
    });
    lane_snapshot(s, 0x144);
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_txrf_mst0_select(false);
    });
    lane_snapshot(s, 0x145);
    let Some(br) = analog_i2c::route(Profile::LibphyNormal, b.block()) else {
        emit(0x14b, 0x170, u32::from(b.block()), u32::from(b.index()));
        return;
    };
    let Some(tr) = analog_i2c::route(Profile::LibphyNormal, t.block()) else {
        emit(0x14b, 0x180, u32::from(t.block()), u32::from(t.index()));
        return;
    };
    // SOURCE: ROM phy_i2c_paral_set_read/read; host0 publication first.
    s.sar
        .analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(
            !(u32::from(br.read_select) | u32::from(tr.read_select)),
        ));
    routing(s, 0x170, Host::Host0, 0x5a);
    routing(s, 0x180, Host::Host1, 0xc3);
    start(s, Host::Host0, b, 0x5a);
    start(s, Host::Host1, t, 0xc3);
    if !finish(s, Host::Host0, 0x170) || !finish(s, Host::Host1, 0x180) {
        return;
    }
    if !serial(s, b, 0x3c, 0x190) || !serial(s, t, 0x5a, 0x1a0) {
        return;
    }
    emit(0x16f, 6, 0, 0); // Six positive reads finished, not a pass verdict.

    // Negative control: no read selector enabled. Expected to differ from
    // the positive BBTOP value or time out; no exact electrical code assumed.
    normal_map(s);
    s.sar
        .analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(u32::MAX));
    if !read_case(s, b, Host::Host1, 0xa5, 0x1b0) {
        return;
    }
    if !serial(s, b, 0x5a, 0x1c0) {
        return;
    }
    // Negative control: BBTOP routed to host0, command deliberately on host1.
    // The correct BBTOP read mask from the preceding serial read remains set.
    s.sar.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_bbtop_mst0_select(true);
    });
    if !read_case(s, b, Host::Host1, 0xa5, 0x1d0) {
        return;
    }
    if !serial(s, b, 0x3c, 0x1e0) {
        return;
    }
    emit(0x1ff, 1, 0, 0);
    // Return this disposable session to ROM download under the held lease.
}
