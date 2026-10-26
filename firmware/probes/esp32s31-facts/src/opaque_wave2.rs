//! Short OPAQUE wave-2 transport experiment. No analog byte is written.
//! SOURCE: oer-notes/research/opaque-blobray-wifi.md P-I2C.
use super::{Bits, Session, contain, emit, pac};
use pac::analog_i2c::{self, Address, Host, Profile};

// Probe-only command deadline: 1 ms at the verified 40 MHz CPU XTAL.
const COMMAND_CYCLES: u32 = 40_000;

fn settle(cycles: u32) {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// Repeat the relevant production prerequisite edges, without running PHY
/// calibration. This does not prove that the ROM left the BBPLL calibrated.
fn prepare(s: &mut Session<'_>) {
    contain(s);
    // SOURCE: hal/src/power/chip_clocks.rs ClockCounts::adopt/ClockWords.
    s.clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_wifi_power_clock(true);
    // SOURCE: hal/src/power.rs execute_owned; modem/ops.rs. Preserve its
    // PMU update -> APB -> maps -> source -> BB reset -> clocks order.
    let p = &mut s.radio;
    p.pmu_radio.select_hp_active_modem_icg();
    p.pmu_radio.trigger_imm_modem_icg_update_dig_icg_modem_en();
    p.pmu_radio.trigger_imm_sleep_sysclk_update_dig_icg_switch();
    p.hp_sys_clkrst_radio.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled);
    });
    p.modem_syscon_radio.modify_clk_conf_power_st(|w| {
        w.set_clk_modem_apb_st_map(Bits::constant::<6>())
            .set_clk_modem_peri_st_map(Bits::constant::<4>())
            .set_clk_wifi_st_map(Bits::constant::<6>())
            .set_clk_bt_st_map(Bits::constant::<4>());
    });
    p.modem_lpcon_shared_clock.modify_clk_conf_power_st(|w| {
        w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
        w.set_clk_coex_st_map(Bits::truncate(w.clk_coex_st_map().get() | 6));
        w.set_clk_i2c_mst_st_map(Bits::truncate(w.clk_i2c_mst_st_map().get() | 6));
        w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
    });
    s.clocks.hp_sys_clkrst_pll160.modify_ref_160m_ctrl0(|w| {
        w.set_reg_ref_160m_clk_en(true);
    });
    p.hp_sys_clkrst_radio.modify_modem_conf(|w| {
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
        p.modem_syscon_radio.modify_modem_rst_conf(|w| {
            w.set_rst_wifibb(reset);
        });
    }
    p.modem_syscon_radio.modify_clk_conf1(|w| {
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
    p.modem_syscon_radio.modify_clk_conf(|w| {
        w.set_clk_i2c_mst_sel_160m(pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled);
    });
    s.clocks
        .modem_lpcon_chip_clocks
        .set_i2c_ana_mst_master_clock(true);

    // SOURCE: hal/src/phy/analog_i2c.rs prepare_open_i2c_pre_delay and
    // complete_open_i2c_power_and_reset; modem/ops.rs. The false BB tie
    // deliberately has no write in production (write-trigger field).
    p.pmu_radio.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(pac::pmu::rf_pwc::XpdRfCircuit::PoweredOff);
    });
    settle(4_000); // sourced 100 us pre-delay, once, in the standalone probe
    p.pmu_radio.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(pac::pmu::rf_pwc::XpdRfCircuit::PoweredOn);
    });
    p.pmu_radio.trigger_imm_hp_ck_power_0_tie_high_xpd_bb_i2c();
    if !p.pmu_radio.read_ana_peri_pwr_ctrl().xpd_perif_i2c() {
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_xpd_perif_i2c(true);
        });
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(false);
        });
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(true);
        });
    }
    if !p.pmu_radio.read_ana_peri_pwr_ctrl().rstb_perif_i2c() {
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(true);
        });
    }
    // SOURCE: hal/src/phy/i2c/master.rs configure_clock_selection and
    // configure_master_registers (six timing RMWs, mode then enable).
    let m = &mut s.analog.i2c_ana_mst;
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

fn routing(s: &Session<'_>, tag: u32) {
    let m = &s.analog.i2c_ana_mst;
    let mask = m.read_ana_conf1();
    emit(
        tag,
        mask.ana_conf1().get() | (u32::from(mask.ana_status1()) << 24),
        m.read_ana_conf2().phy_host_map().get(),
        m.read_ana_conf0().phy_register_mode().get(),
    );
}

fn busy(s: &Session<'_>, host: Host) -> bool {
    let m = &s.analog.i2c_ana_mst;
    match host {
        Host::Host0 => m.read_i2c0_ctrl().i2c0_busy(),
        Host::Host1 => m.read_i2c1_ctrl().i2c1_busy(),
    }
}

fn start(s: &mut Session<'_>, host: Host, address: Address) {
    let m = &mut s.analog.i2c_ana_mst;
    // SOURCE: hal/src/phy/i2c/master.rs publish_command; ROM_REV0_PHY_I2C.
    // READ_WRITE=false for every command; no analog write path exists here.
    match host {
        Host::Host0 => m.write_i2c0_ctrl(|w| {
            w.set_slave_addr(address.block())
                .set_slave_reg_addr(address.index())
                .set_data(0)
                .set_read_write(false)
                .set_start_or_reset(true);
        }),
        Host::Host1 => m.write_i2c1_ctrl(|w| {
            w.set_slave_addr(address.block())
                .set_slave_reg_addr(address.index())
                .set_data(0)
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
                tag,
                u32::MAX,
                polls,
                oer_riscv::cycle32().wrapping_sub(start),
            );
            return false;
        }
        core::hint::spin_loop();
    }
    let m = &s.analog.i2c_ana_mst;
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
        tag,
        u32::from(data),
        polls,
        oer_riscv::cycle32().wrapping_sub(start),
    );
    emit(
        tag + 1,
        u32::from(block),
        u32::from(index),
        u32::from(busy(s, host)),
    );
    true
}

fn serial(s: &mut Session<'_>, address: Address, tag: u32) -> bool {
    let Some(route) = analog_i2c::route(Profile::LibphyNormal, address.block()) else {
        return false;
    };
    if busy(s, route.host) {
        emit(tag, u32::MAX, 0, 0);
        return false;
    }
    s.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_phy_host_map(Bits::constant::<0x3fa0>());
    });
    s.analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(route.complemented_read_select()));
    routing(s, tag);
    start(s, route.host, address);
    finish(s, route.host, tag + 1)
}

pub(super) fn i2c(s: &mut Session<'_>) {
    if !super::opaque::xtal_cpu(s) {
        emit(0x14f, 14, 0, 0);
        return;
    }
    emit(0x140, 0, 0, 0);
    prepare(s);
    let p = &s.radio;
    let power = p.pmu_radio.read_ana_peri_pwr_ctrl();
    let gates = s.clocks.modem_lpcon_chip_clocks.read_clk_conf();
    emit(
        0x141,
        u32::from(power.xpd_perif_i2c()),
        u32::from(power.rstb_perif_i2c()),
        u32::from(gates.clk_i2c_mst_en()),
    );
    emit(
        0x142,
        p.modem_syscon_radio
            .read_clk_conf()
            .clk_i2c_mst_sel_160m()
            .field(),
        u32::from(
            s.clocks
                .hp_sys_clkrst_pll160
                .read_ref_160m_ctrl0()
                .reg_ref_160m_clk_en(),
        ),
        u32::from(s.analog.i2c_ana_mst.read_ana_conf0().phy_register_enable()),
    );
    let b = analog_i2c::bbtop::byte_04_opaque::ADDRESS;
    let t = analog_i2c::txrf::byte_08_opaque::ADDRESS;
    if !serial(s, b, 0x150) || !serial(s, t, 0x154) {
        return;
    }
    if busy(s, Host::Host0) || busy(s, Host::Host1) {
        emit(0x14f, 0, 1, 0);
        return;
    }
    // SOURCE: ROM_REV0_PHY_I2C, phy_i2c_paral_set_mst0 at 2f82a082:
    // OR BBTOP read_select<<2, then clear TXRF read_select<<2 in ANA_CONF2.
    // Within the modeled host-map field these are bits 0 and 1 respectively.
    s.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_phy_host_map(Bits::truncate(w.phy_host_map().get() | 1));
    });
    s.analog.i2c_ana_mst.modify_ana_conf2(|w| {
        w.set_phy_host_map(Bits::truncate(w.phy_host_map().get() & !2));
    });
    let Some(br) = analog_i2c::route(Profile::LibphyNormal, b.block()) else {
        return;
    };
    let Some(tr) = analog_i2c::route(Profile::LibphyNormal, t.block()) else {
        return;
    };
    // SOURCE: ROM phy_i2c_paral_set_read and phy_i2c_paral_read, no callbacks.
    s.analog
        .i2c_ana_mst
        .publish_phy_i2c_read_mask(Bits::truncate(
            !(u32::from(br.read_select) | u32::from(tr.read_select)),
        ));
    routing(s, 0x158);
    start(s, Host::Host0, b);
    start(s, Host::Host1, t);
    if !finish(s, Host::Host0, 0x159) || !finish(s, Host::Host1, 0x15b) {
        return;
    }
    if !serial(s, b, 0x160) || !serial(s, t, 0x164) {
        return;
    }
    emit(0x16f, 1, 0, 0);
    // The disposable session is returned to ROM download by the leased host.
}
