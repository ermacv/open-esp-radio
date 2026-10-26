//! Bounded digital-only opaque/IRQ facts. No RF or controller scheduler starts.
//! SOURCE: oer-notes research/opaque-blobray-{ble,wifi}.md and
//! irq-blobray-wifi.md (P0, P-BA, BB-L, P-TSENS).
use super::{Bits, Index, Session, contain, emit, pac};

/// The opaque leaves' own PAC parts. The radio clock, reset and PMU parts are
/// shared with the BLE leaves through `Session::ble`.
pub(super) struct Hardware {
    pub wifi_bb: pac::wifi_bb::PhyBasebandConfigOracleWifiBb,
    pub block_ack: pac::wifi_mac::WifiMacBlockAck,
    pub core_enable: pac::wifi_mac::WifiMacCoreEnable,
    pub mac_interrupt: pac::wifi_mac::WifiMacInterrupt,
    pub tsens: pac::tsens::LpTsens,
    pub periclkrst: pac::lp_peri_clkrst::LpPericlkrst,
}

/// Named observation bound from BB-L; independent of modem clock progress.
fn wait_cycles(cycles: u32) {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < cycles {}
}

/// Refuse timing attribution unless the CPU is directly on the 40 MHz XTAL.
/// SOURCE: HP clock model, pinned IDF clk_tree_defs.h (40 MHz XTAL).
fn xtal_cpu(s: &Session<'_>) -> bool {
    let c = &s.boot_clock;
    let root = c.read_soc_clk_sel().reg_soc_clk_sel().get();
    let div = c.read_cpu_freq_ctrl0();
    emit(
        0xa0,
        root,
        u32::from(div.reg_cpu_clk_div_num()),
        div.reg_cpu_clk_div_numerator().get() | (div.reg_cpu_clk_div_denominator().get() << 8),
    );
    root == 0 && div.reg_cpu_clk_div_num() == 0 && div.reg_cpu_clk_div_numerator().get() == 0
}

/// Digital gates only. The reset-isolated session ends with a download reset.
/// SOURCE: existing HAL modem operations and ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP.
fn prepare(s: &mut Session<'_>) {
    contain(s);
    oer_riscv::disable_interrupts();
    let p = &mut s.ble;
    p.upstream.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled);
    });
    p.pmu.select_hp_active_modem_icg();
    p.clocks.modify_clk_conf_power_st(|w| {
        w.set_clk_modem_apb_st_map(Bits::constant::<6>())
            .set_clk_modem_peri_st_map(Bits::constant::<4>())
            .set_clk_wifi_st_map(Bits::constant::<6>())
            .set_clk_bt_st_map(Bits::constant::<4>());
    });
    p.lp_clock.modify_clk_conf_power_st(|w| {
        w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
        w.set_clk_coex_st_map(Bits::truncate(w.clk_coex_st_map().get() | 6));
        w.set_clk_i2c_mst_st_map(Bits::truncate(w.clk_i2c_mst_st_map().get() | 6));
        w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
    });
    p.pmu.trigger_imm_modem_icg_update_dig_icg_modem_en();
    p.pmu.trigger_imm_sleep_sysclk_update_dig_icg_switch();
    p.upstream.close_modem_pll_source_gate();
    p.clocks.modify_clk_conf1(|w| {
        use pac::modem_syscon::clk_conf1::*;
        w.set_clk_wifi_apb_en(ClkWifiApbEn::Enabled)
            .set_clk_wifimac_en(ClkWifimacEn::Enabled)
            .set_clk_bt_apb_en(ClkBtApbEn::Enabled)
            .set_clk_btmac_en(ClkBtmacEn::Enabled)
            .set_clk_fe_apb_en(true);
    });
    s.opaque.core_enable.modify_core_enable_control(|w| {
        w.set_bits_4_7_opaque(pac::wifi_mac::core_enable_control::Bits4_7Opaque::Disabled);
    });
    oer_riscv::fence();
}

fn clock_snapshot(s: &Session<'_>, tag: u32) {
    let p = &s.ble;
    emit(
        tag,
        modem_conf(p.upstream.read_modem_conf()),
        p.pmu
            .read_hp_active_icg_modem()
            .hp_active_dig_icg_modem_code()
            .get(),
        syscon_map(p.clocks.read_clk_conf_power_st()),
    );
    emit(
        tag + 1,
        lpcon_map(p.lp_clock.read_clk_conf_power_st()),
        syscon_clocks(p.clocks.read_clk_conf1()),
        syscon_reset(p.clocks.read_modem_rst_conf()),
    );
}

pub(super) fn pll(s: &mut Session<'_>) {
    if !xtal_cpu(s) {
        emit(0xaf, 10, 0, 0);
        return;
    }
    contain(s);
    // HP/LP register observations remain accessible while the modem source
    // changes. Do not read a modem-domain timer on an unestablished PLL.
    emit(
        0xa1,
        modem_conf(s.ble.upstream.read_modem_conf()),
        s.ble
            .pmu
            .read_hp_active_icg_modem()
            .hp_active_dig_icg_modem_code()
            .get(),
        0,
    );
    // Both sourced sequences once, readback immediately after each step.
    // The six booleans express sourced images, not a local MMIO mask.
    for (step, pll, source, xtal) in [
        (0, true, true, true),
        (1, false, false, true),
        (2, true, false, false),
        (3, true, true, false),
        (4, false, true, true),
        (5, false, false, true),
    ] {
        s.ble.upstream.modify_modem_conf(|w| {
            w.set_modem_apb_clk_en(true)
                .set_modem_rst_en(false)
                .set_modem_clk_en(true)
                .set_modem_pll_clk_en(pll)
                .set_modem_clk_source_sel(source)
                .set_modem_xtal_clk_en(xtal);
        });
        oer_riscv::fence();
        emit(
            0xa4 + step * 4,
            modem_conf(s.ble.upstream.read_modem_conf()),
            s.ble
                .pmu
                .read_hp_active_icg_modem()
                .hp_active_dig_icg_modem_code()
                .get(),
            oer_riscv::cycle32(),
        );
    }
    s.ble.upstream.close_modem_pll_source_gate();
}

fn ordinary_snapshot(s: &Session<'_>, tag: u32) {
    let b = &s.opaque.block_ack;
    let i = Index::constant::<7>(); // vendor hardware index 0, reverse-addressed
    emit(
        tag,
        ba_control(b.read_rx_block_ack_entry_control(i)),
        ba_peer(b.read_rx_block_ack_entry_peer_tail_and_policy(i)),
        b.read_rx_block_ack_entry_peer_head(i).peer_address_head(),
    );
    emit(
        tag + 1,
        b.read_rx_block_ack_entry_current_sequence(i)
            .sequence()
            .get(),
        b.read_rx_block_ack_entry_bitmap_low_status(i).bitmap(),
        b.read_rx_block_ack_entry_bitmap_high_status(i).bitmap(),
    );
}
fn extra_snapshot(s: &Session<'_>, tag: u32) {
    let b = &s.opaque.block_ack;
    emit(
        tag,
        extra_control(b.read_extra_softap_rx_block_ack_control()),
        extra_peer(b.read_extra_softap_rx_block_ack_peer_tail_and_policy()),
        b.read_extra_softap_rx_block_ack_peer_head()
            .peer_address_head(),
    );
    emit(
        tag + 1,
        b.read_extra_softap_rx_block_ack_start_sequence()
            .sequence()
            .get(),
        b.read_extra_softap_rx_block_ack_bitmap_low().bitmap(),
        b.read_extra_softap_rx_block_ack_bitmap_high().bitmap(),
    );
}
pub(super) fn block_ack(s: &mut Session<'_>) {
    prepare(s);
    enable_baseband(s);
    clock_snapshot(s, 0xb0);
    ordinary_snapshot(s, 0xc0);
    extra_snapshot(s, 0xc2);
    let b = &mut s.opaque.block_ack;
    let i = Index::constant::<7>();
    b.modify_rx_block_ack_entry_peer_head(i, |w| {
        w.set_peer_address_head(0x03020102);
    });
    b.modify_rx_block_ack_entry_peer_tail_and_policy(i, |w| {
        w.set_peer_address_tail(0x0504)
            .set_interface(
                pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::Interface::from_field(0),
            )
            .set_window(Bits::constant::<32>())
            .set_bits_25_31_opaque(Bits::constant::<0>());
    });
    b.modify_rx_block_ack_entry_start_sequence_load(i, |w| {
        w.set_sequence(Bits::constant::<0x123>());
    });
    b.modify_rx_block_ack_entry_control(i, |w| {
        w.set_valid(false);
    });
    b.modify_rx_block_ack_entry_bitmap_low_load(i, |w| {
        w.set_bitmap(0);
    });
    b.modify_rx_block_ack_entry_bitmap_high_load(i, |w| {
        w.set_bitmap(0);
    });
    b.modify_rx_block_ack_agreement_update(|w| {
        w.set_ordinary_entry_update(w.ordinary_entry_update() | 1);
    });
    b.modify_rx_block_ack_entry_control(i, |w| {
        w.set_enable(true)
            .set_tid(Bits::constant::<3>())
            .set_write(true)
            .set_valid(true)
            .set_bits_1_11_opaque(Bits::constant::<0>())
            .set_bits_16_29_opaque(Bits::constant::<0>());
    });
    ordinary_snapshot(s, 0xc4);
    ordinary_snapshot(s, 0xc6);
    s.opaque
        .block_ack
        .modify_rx_block_ack_agreement_update(|w| {
            w.set_ordinary_entry_update(w.ordinary_entry_update() | 1);
        });
    oer_riscv::fence();
    wait_cycles(4096);
    ordinary_snapshot(s, 0xc8);
    let b = &mut s.opaque.block_ack;
    b.modify_extra_softap_rx_block_ack_control(|w| {
        w.set_index(Bits::constant::<3>());
    });
    b.modify_extra_softap_rx_block_ack_peer_head(|w| {
        w.set_peer_address_head(0x03020102);
    });
    b.modify_extra_softap_rx_block_ack_peer_tail_and_policy(|w| {
        w.set_peer_address_tail(0x0504).set_interface(pac::wifi_mac::extra_softap_rx_block_ack_peer_tail_and_policy::Interface::from_field(0))
            .set_window(Bits::constant::<32>()).set_bits_25_31_opaque(Bits::constant::<0>());
    });
    b.modify_extra_softap_rx_block_ack_start_sequence(|w| {
        w.set_sequence(Bits::constant::<0x123>());
    });
    b.modify_extra_softap_rx_block_ack_bitmap_low(|w| {
        w.set_bitmap(0);
    });
    b.modify_extra_softap_rx_block_ack_bitmap_high(|w| {
        w.set_bitmap(0);
    });
    b.modify_extra_softap_rx_block_ack_control(|w| {
        w.set_enable(true)
            .set_index(Bits::constant::<3>())
            .set_tid(Bits::constant::<3>())
            .set_write(true)
            .set_valid(true)
            .set_bits_1_4_opaque(Bits::constant::<0>())
            .set_bits_10_11_opaque(Bits::constant::<0>())
            .set_bits_16_29_opaque(Bits::constant::<0>());
    });
    extra_snapshot(s, 0xca);
    extra_snapshot(s, 0xcc);
    let b = &mut s.opaque.block_ack;
    b.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_commit(true);
    });
    b.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_commit(false);
    });
    b.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_readback_latch(true);
    });
    oer_riscv::fence();
    extra_snapshot(s, 0xce);
    s.opaque
        .block_ack
        .modify_rx_block_ack_agreement_update(|w| {
            w.set_extra_softap_readback_latch(false);
        });
}

// WORD_004C_OPAQUE has unknown read side effects and no PAC accessor.
// This snapshot deliberately makes no handler-word/read-to-ack observation.
fn bb_snapshot(s: &Session<'_>, tag: u32) -> u32 {
    let b = &s.opaque.wifi_bb;
    let status = b.read_baseband_watchdog_status().status();
    let raw = mac_raw(s.opaque.mac_interrupt.read_raw());
    let st = mac_status(s.opaque.mac_interrupt.read_status());
    let matrix = matrix3(
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3(),
    );
    emit(
        tag,
        status,
        bb_control(b.read_baseband_watchdog_control()),
        bb_enable(b.read_baseband_watchdog_enable()),
    );
    emit(tag + 1, raw, st, matrix);
    emit(
        tag + 2,
        s.opaque
            .core_enable
            .read_core_enable_control()
            .bits_4_7_opaque()
            .get(),
        s.opaque
            .block_ack
            .read_rx_block_ack_entry_peer_head(Index::constant::<7>())
            .peer_address_head(),
        0,
    );
    status
}
fn set_bb(s: &mut Session<'_>, irq: bool, clear: bool, reset: bool) {
    s.opaque.wifi_bb.modify_baseband_watchdog_enable(|w| {
        w.set_watchdog_reset_enable(reset)
            .set_watchdog_interrupt_enable(irq)
            .set_watchdog_timeout_clear_request(clear);
    });
    oer_riscv::fence();
}
fn enable_baseband(s: &mut Session<'_>) {
    // SOURCE: HAL modem clock-device BB reset-before-enable sequence and
    // ROM phy_mac_enable_bb's digital interface / Wi-Fi-enable transition.
    s.ble.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_wifibb(pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted);
    });
    s.ble.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_wifibb(pac::modem_syscon::modem_rst_conf::RstWifibb::Released);
    });
    // SOURCE: ROM open_fe_bb clocks; enable digital BB clocks only, RF stays off.
    s.ble.clocks.modify_clk_conf1(|w| {
        use pac::modem_syscon::clk_conf1::*;
        w.set_clk_wifibb_22m_en(ClkWifibb22mEn::Enabled)
            .set_clk_wifibb_40m_en(ClkWifibb40mEn::Enabled)
            .set_clk_wifibb_44m_en(ClkWifibb44mEn::Enabled)
            .set_clk_wifibb_80m_en(ClkWifibb80mEn::Enabled)
            .set_clk_wifibb_40x_en(ClkWifibb40xEn::Enabled)
            .set_clk_wifibb_80x_en(ClkWifibb80xEn::Enabled)
            .set_clk_wifibb_40x1_en(ClkWifibb40x1En::Enabled)
            .set_clk_wifibb_80x1_en(ClkWifibb80x1En::Enabled)
            .set_clk_wifibb_160x1_en(ClkWifibb160x1En::Enabled);
    });
    s.ble.clocks.modify_wifi_bb_cfg(|w| {
        w.set_bit_28_opaque(pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Enabled)
            .set_wifi_enable(pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled);
    });
    s.ble.clocks.modify_wifi_bb_cfg(|w| {
        w.set_wifi_enable(pac::modem_syscon::wifi_bb_cfg::WifiEnable::Enabled);
    });
}

pub(super) fn watchdog(s: &mut Session<'_>) {
    prepare(s);
    enable_baseband(s);
    let b = &mut s.opaque.wifi_bb;
    let old_control = b.read_baseband_watchdog_control();
    let old_enable = b.read_baseband_watchdog_enable();
    set_bb(s, false, false, false);
    bb_snapshot(s, 0xd0);
    s.opaque.wifi_bb.modify_baseband_watchdog_control(|w| {
        w.set_bits_0_15_opaque(1)
            .set_bits_16_29_opaque(Bits::constant::<1>())
            .set_bit_30_opaque(true)
            .set_bit_31_opaque(true);
    });
    set_bb(s, true, true, false);
    bb_snapshot(s, 0xd3);
    set_bb(s, true, false, false);
    wait_cycles(4096);
    let positive = bb_snapshot(s, 0xd6);
    s.opaque.wifi_bb.modify_baseband_watchdog_control(|w| {
        w.set_bits_0_15_opaque(old_control.bits_0_15_opaque())
            .set_bits_16_29_opaque(old_control.bits_16_29_opaque())
            .set_bit_30_opaque(old_control.bit_30_opaque())
            .set_bit_31_opaque(old_control.bit_31_opaque());
    });
    set_bb(s, false, false, false);
    bb_snapshot(s, 0xd9);
    set_bb(s, true, false, false);
    bb_snapshot(s, 0xdc);
    set_bb(s, true, true, false);
    bb_snapshot(s, 0xdf);
    set_bb(s, false, false, false);
    set_bb(s, false, true, false);
    bb_snapshot(s, 0xe2);
    set_bb(
        s,
        old_enable.watchdog_interrupt_enable(),
        old_enable.watchdog_timeout_clear_request(),
        old_enable.watchdog_reset_enable(),
    );
    emit(0xe5, positive, 0, 0); // zero leaves reset scope untested, never invent a producer
}

pub(super) fn temperature(s: &mut Session<'_>) {
    if !xtal_cpu(s) {
        emit(0xaf, 13, 0, 0);
        return;
    }
    let p = &mut s.ble;
    let old_clk = s.opaque.tsens.read_clk_conf();
    let old_ctrl = s.opaque.tsens.read_ctrl();
    let old_peri = s.opaque.periclkrst.read_tsens_ctrl();
    let old_power = p.pmu.read_ana_peri_pwr_ctrl();
    p.pmu.modify_ana_peri_pwr_ctrl(|w| {
        w.set_rstb_perif_i2c(false);
    });
    wait_cycles(40); // sourced 1 us SAR-I2C reset settle
    p.pmu.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(true).set_rstb_perif_i2c(true);
    });
    s.opaque.periclkrst.modify_tsens_ctrl(|w| {
        w.set_lp_tsens_clk_en(pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Enabled)
            .set_lp_tsens_rst_en(false);
    });
    for (case, bit9, bit23) in [
        (0, true, true),
        (1, false, true),
        (2, true, false),
        (3, false, false),
    ] {
        s.opaque.tsens.modify_clk_conf(|w| {
            w.set_clk_en(true)
                .set_bit_9_opaque(bit9)
                .set_bit_23_opaque(bit23);
        });
        s.opaque.tsens.modify_ctrl(|w| {
            w.set_power_up(pac::tsens::ctrl::PowerUp::Enabled)
                .set_power_up_force(true)
                .set_clk_div(15);
        });
        let configured = s.opaque.tsens.read_clk_conf();
        let control = s.opaque.tsens.read_ctrl();
        let peripheral = s.opaque.periclkrst.read_tsens_ctrl();
        emit(
            0x110 + case,
            u32::from(configured.bit_9_opaque())
                | (u32::from(configured.bit_23_opaque()) << 1)
                | (u32::from(configured.clk_en()) << 2),
            u32::from(control.clk_div()),
            u32::from(control.power_up())
                | (u32::from(control.power_up_force()) << 1)
                | (u32::from(peripheral.lp_tsens_clk_en()) << 2)
                | (u32::from(peripheral.lp_tsens_rst_en()) << 3),
        );
        for repeat in 0..2 {
            s.opaque.tsens.modify_ctrl(|w| {
                w.set_sample_en(false);
            });
            wait_cycles(400);
            let before = s.opaque.tsens.read_ctrl();
            s.opaque.tsens.modify_ctrl(|w| {
                w.set_sample_en(true);
            });
            let mut ready_count = 0u32;
            let mut changes = 0u32;
            let mut prior = before.out();
            let mut last = before;
            let start = oer_riscv::cycle32();
            for sample in 1..=100u32 {
                // At most 99 reads at 10..990 us: the 1000 us iteration
                // reaches the deadline and exits before its read.
                while oer_riscv::cycle32().wrapping_sub(start) < sample * 400 {}
                if oer_riscv::cycle32().wrapping_sub(start) >= 40_000 {
                    break;
                }
                last = s.opaque.tsens.read_ctrl();
                ready_count += u32::from(last.ready());
                changes += u32::from(last.out() != prior);
                prior = last.out();
            }
            emit(
                0xf0 + case * 4 + repeat * 2,
                u32::from(before.ready()),
                u32::from(before.out()),
                ready_count,
            );
            emit(
                0xf1 + case * 4 + repeat * 2,
                u32::from(last.ready()),
                u32::from(last.out()),
                changes,
            );
        }
        s.opaque.tsens.modify_clk_conf(|w| {
            w.set_bit_9_opaque(old_clk.bit_9_opaque())
                .set_bit_23_opaque(old_clk.bit_23_opaque());
        });
    }
    s.opaque.tsens.modify_ctrl(|w| {
        w.set_sample_en(old_ctrl.sample_en())
            .set_power_up(pac::tsens::ctrl::PowerUp::from_field(u32::from(
                old_ctrl.power_up(),
            )))
            .set_power_up_force(old_ctrl.power_up_force())
            .set_clk_div(old_ctrl.clk_div());
    });
    s.opaque.tsens.modify_clk_conf(|w| {
        w.set_clk_en(old_clk.clk_en())
            .set_bit_9_opaque(old_clk.bit_9_opaque())
            .set_bit_23_opaque(old_clk.bit_23_opaque());
    });
    s.opaque.periclkrst.modify_tsens_ctrl(|w| {
        w.set_lp_tsens_clk_en(pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::from_field(
            u32::from(old_peri.lp_tsens_clk_en()),
        ))
        .set_lp_tsens_rst_en(old_peri.lp_tsens_rst_en());
    });
    p.pmu.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(old_power.xpd_perif_i2c())
            .set_rstb_perif_i2c(old_power.rstb_perif_i2c());
    });
}

fn modem_conf(r: pac::hp_sys_clkrst::modem_conf::R) -> u32 {
    (u32::from(r.modem_apb_clk_en())
        << pac::hp_sys_clkrst::modem_conf::MODEM_APB_CLK_EN_MASK.trailing_zeros())
        | (u32::from(r.modem_rst_en())
            << pac::hp_sys_clkrst::modem_conf::MODEM_RST_EN_MASK.trailing_zeros())
        | (u32::from(r.modem_clk_en())
            << pac::hp_sys_clkrst::modem_conf::MODEM_CLK_EN_MASK.trailing_zeros())
        | (u32::from(r.modem_clk_source_sel())
            << pac::hp_sys_clkrst::modem_conf::MODEM_CLK_SOURCE_SEL_MASK.trailing_zeros())
        | (u32::from(r.modem_pll_clk_en())
            << pac::hp_sys_clkrst::modem_conf::MODEM_PLL_CLK_EN_MASK.trailing_zeros())
        | (u32::from(r.modem_xtal_clk_en())
            << pac::hp_sys_clkrst::modem_conf::MODEM_XTAL_CLK_EN_MASK.trailing_zeros())
}

fn syscon_map(r: pac::modem_syscon::clk_conf_power_st::R) -> u32 {
    (u32::from(r.bits_0_7_opaque())
        << pac::modem_syscon::clk_conf_power_st::BITS_0_7_OPAQUE_MASK.trailing_zeros())
        | (r.clk_zb_st_map().get()
            << pac::modem_syscon::clk_conf_power_st::CLK_ZB_ST_MAP_MASK.trailing_zeros())
        | (r.clk_fe_st_map().get()
            << pac::modem_syscon::clk_conf_power_st::CLK_FE_ST_MAP_MASK.trailing_zeros())
        | (r.clk_bt_st_map().get()
            << pac::modem_syscon::clk_conf_power_st::CLK_BT_ST_MAP_MASK.trailing_zeros())
        | (r.clk_wifi_st_map().get()
            << pac::modem_syscon::clk_conf_power_st::CLK_WIFI_ST_MAP_MASK.trailing_zeros())
        | (r.clk_modem_peri_st_map().get()
            << pac::modem_syscon::clk_conf_power_st::CLK_MODEM_PERI_ST_MAP_MASK.trailing_zeros())
        | (r.clk_modem_apb_st_map().get()
            << pac::modem_syscon::clk_conf_power_st::CLK_MODEM_APB_ST_MAP_MASK.trailing_zeros())
}

fn lpcon_map(r: pac::modem_lpcon::clk_conf_power_st::R) -> u32 {
    (u32::from(r.bits_0_15_opaque())
        << pac::modem_lpcon::clk_conf_power_st::BITS_0_15_OPAQUE_MASK.trailing_zeros())
        | (r.clk_wifipwr_st_map().get()
            << pac::modem_lpcon::clk_conf_power_st::CLK_WIFIPWR_ST_MAP_MASK.trailing_zeros())
        | (r.clk_coex_st_map().get()
            << pac::modem_lpcon::clk_conf_power_st::CLK_COEX_ST_MAP_MASK.trailing_zeros())
        | (r.clk_i2c_mst_st_map().get()
            << pac::modem_lpcon::clk_conf_power_st::CLK_I2C_MST_ST_MAP_MASK.trailing_zeros())
        | (r.clk_lp_apb_st_map().get()
            << pac::modem_lpcon::clk_conf_power_st::CLK_LP_APB_ST_MAP_MASK.trailing_zeros())
}

fn syscon_clocks(r: pac::modem_syscon::clk_conf1::R) -> u32 {
    (u32::from(r.clk_wifibb_22m_en())
        << pac::modem_syscon::clk_conf1::CLK_WIFIBB_22M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_40m_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_40M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_44m_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_44M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_80m_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_80M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_40x_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_40X_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_80x_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_80X_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_40x1_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_40X1_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_80x1_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_80X1_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifibb_160x1_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIBB_160X1_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifimac_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFIMAC_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_wifi_apb_en())
            << pac::modem_syscon::clk_conf1::CLK_WIFI_APB_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_20m_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_20M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_40m_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_40M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_80m_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_80M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_160m_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_160M_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_apb_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_APB_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_bt_apb_en())
            << pac::modem_syscon::clk_conf1::CLK_BT_APB_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_btbb_en())
            << pac::modem_syscon::clk_conf1::CLK_BTBB_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_btmac_en())
            << pac::modem_syscon::clk_conf1::CLK_BTMAC_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_pwdet_adc_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_PWDET_ADC_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_adc_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_ADC_EN_MASK.trailing_zeros())
        | (u32::from(r.clk_fe_dac_en())
            << pac::modem_syscon::clk_conf1::CLK_FE_DAC_EN_MASK.trailing_zeros())
        | (r.bits_22_31_opaque().get()
            << pac::modem_syscon::clk_conf1::BITS_22_31_OPAQUE_MASK.trailing_zeros())
}

fn syscon_reset(r: pac::modem_syscon::modem_rst_conf::R) -> u32 {
    (u32::from(r.rst_wifibb())
        << pac::modem_syscon::modem_rst_conf::RST_WIFIBB_MASK.trailing_zeros())
        | (u32::from(r.rst_wifimac())
            << pac::modem_syscon::modem_rst_conf::RST_WIFIMAC_MASK.trailing_zeros())
        | (u32::from(r.rst_fe_pwdet_adc())
            << pac::modem_syscon::modem_rst_conf::RST_FE_PWDET_ADC_MASK.trailing_zeros())
        | (u32::from(r.rst_fe_dac())
            << pac::modem_syscon::modem_rst_conf::RST_FE_DAC_MASK.trailing_zeros())
        | (u32::from(r.rst_fe_adc())
            << pac::modem_syscon::modem_rst_conf::RST_FE_ADC_MASK.trailing_zeros())
        | (u32::from(r.rst_fe_ahb())
            << pac::modem_syscon::modem_rst_conf::RST_FE_AHB_MASK.trailing_zeros())
        | (u32::from(r.rst_fe()) << pac::modem_syscon::modem_rst_conf::RST_FE_MASK.trailing_zeros())
        | (u32::from(r.rst_btmac_apb())
            << pac::modem_syscon::modem_rst_conf::RST_BTMAC_APB_MASK.trailing_zeros())
        | (u32::from(r.rst_btmac())
            << pac::modem_syscon::modem_rst_conf::RST_BTMAC_MASK.trailing_zeros())
        | (u32::from(r.rst_btbb_apb())
            << pac::modem_syscon::modem_rst_conf::RST_BTBB_APB_MASK.trailing_zeros())
        | (u32::from(r.rst_btbb())
            << pac::modem_syscon::modem_rst_conf::RST_BTBB_MASK.trailing_zeros())
        | (u32::from(r.rst_coex_btmac_apb())
            << pac::modem_syscon::modem_rst_conf::RST_COEX_BTMAC_APB_MASK.trailing_zeros())
        | (u32::from(r.rst_coex_btmac())
            << pac::modem_syscon::modem_rst_conf::RST_COEX_BTMAC_MASK.trailing_zeros())
        | (u32::from(r.rst_etm())
            << pac::modem_syscon::modem_rst_conf::RST_ETM_MASK.trailing_zeros())
        | (u32::from(r.rst_zbmac_apb())
            << pac::modem_syscon::modem_rst_conf::RST_ZBMAC_APB_MASK.trailing_zeros())
        | (u32::from(r.rst_zbmac())
            << pac::modem_syscon::modem_rst_conf::RST_ZBMAC_MASK.trailing_zeros())
        | (u32::from(r.rst_modem_ecb())
            << pac::modem_syscon::modem_rst_conf::RST_MODEM_ECB_MASK.trailing_zeros())
        | (u32::from(r.rst_modem_ccm())
            << pac::modem_syscon::modem_rst_conf::RST_MODEM_CCM_MASK.trailing_zeros())
        | (u32::from(r.rst_modem_bah())
            << pac::modem_syscon::modem_rst_conf::RST_MODEM_BAH_MASK.trailing_zeros())
        | (u32::from(r.rst_modem_sec())
            << pac::modem_syscon::modem_rst_conf::RST_MODEM_SEC_MASK.trailing_zeros())
        | (u32::from(r.rst_ble_timer())
            << pac::modem_syscon::modem_rst_conf::RST_BLE_TIMER_MASK.trailing_zeros())
        | (u32::from(r.rst_data_dump())
            << pac::modem_syscon::modem_rst_conf::RST_DATA_DUMP_MASK.trailing_zeros())
}

fn ba_control(r: pac::wifi_mac::rx_block_ack_entry_control::R) -> u32 {
    (u32::from(r.enable())
        << pac::wifi_mac::rx_block_ack_entry_control::ENABLE_MASK.trailing_zeros())
        | (r.bits_1_11_opaque().get()
            << pac::wifi_mac::rx_block_ack_entry_control::BITS_1_11_OPAQUE_MASK.trailing_zeros())
        | (r.tid().get() << pac::wifi_mac::rx_block_ack_entry_control::TID_MASK.trailing_zeros())
        | (r.bits_16_29_opaque().get()
            << pac::wifi_mac::rx_block_ack_entry_control::BITS_16_29_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.write())
            << pac::wifi_mac::rx_block_ack_entry_control::WRITE_MASK.trailing_zeros())
        | (u32::from(r.valid())
            << pac::wifi_mac::rx_block_ack_entry_control::VALID_MASK.trailing_zeros())
}

fn ba_peer(r: pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::R) -> u32 {
    (u32::from(r.peer_address_tail())
        << pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::PEER_ADDRESS_TAIL_MASK
            .trailing_zeros())
        | (r.interface().get()
            << pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::INTERFACE_MASK
                .trailing_zeros())
        | (r.window().get()
            << pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::WINDOW_MASK.trailing_zeros())
        | (r.bits_25_31_opaque().get()
            << pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::BITS_25_31_OPAQUE_MASK
                .trailing_zeros())
}

fn extra_control(r: pac::wifi_mac::extra_softap_rx_block_ack_control::R) -> u32 {
    (u32::from(r.enable())
        << pac::wifi_mac::extra_softap_rx_block_ack_control::ENABLE_MASK.trailing_zeros())
        | (r.index().get()
            << pac::wifi_mac::extra_softap_rx_block_ack_control::INDEX_MASK.trailing_zeros())
        | (r.bits_10_11_opaque().get()
            << pac::wifi_mac::extra_softap_rx_block_ack_control::BITS_10_11_OPAQUE_MASK
                .trailing_zeros())
        | (r.tid().get()
            << pac::wifi_mac::extra_softap_rx_block_ack_control::TID_MASK.trailing_zeros())
        | (r.bits_16_29_opaque().get()
            << pac::wifi_mac::extra_softap_rx_block_ack_control::BITS_16_29_OPAQUE_MASK
                .trailing_zeros())
        | (u32::from(r.write())
            << pac::wifi_mac::extra_softap_rx_block_ack_control::WRITE_MASK.trailing_zeros())
        | (u32::from(r.valid())
            << pac::wifi_mac::extra_softap_rx_block_ack_control::VALID_MASK.trailing_zeros())
        | (r.bits_1_4_opaque().get()
            << pac::wifi_mac::extra_softap_rx_block_ack_control::BITS_1_4_OPAQUE_MASK
                .trailing_zeros())
}

fn extra_peer(r: pac::wifi_mac::extra_softap_rx_block_ack_peer_tail_and_policy::R) -> u32 {
    (u32::from(r.peer_address_tail()) << pac::wifi_mac::extra_softap_rx_block_ack_peer_tail_and_policy::PEER_ADDRESS_TAIL_MASK.trailing_zeros()) |
(r.interface().get() << pac::wifi_mac::extra_softap_rx_block_ack_peer_tail_and_policy::INTERFACE_MASK.trailing_zeros()) |
(r.window().get() << pac::wifi_mac::extra_softap_rx_block_ack_peer_tail_and_policy::WINDOW_MASK.trailing_zeros()) |
(r.bits_25_31_opaque().get() << pac::wifi_mac::extra_softap_rx_block_ack_peer_tail_and_policy::BITS_25_31_OPAQUE_MASK.trailing_zeros())
}

fn bb_control(r: pac::wifi_bb::baseband_watchdog_control::R) -> u32 {
    (u32::from(r.bits_0_15_opaque())
        << pac::wifi_bb::baseband_watchdog_control::BITS_0_15_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.bit_30_opaque())
            << pac::wifi_bb::baseband_watchdog_control::BIT_30_OPAQUE_MASK.trailing_zeros())
        | (r.bits_16_29_opaque().get()
            << pac::wifi_bb::baseband_watchdog_control::BITS_16_29_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.bit_31_opaque())
            << pac::wifi_bb::baseband_watchdog_control::BIT_31_OPAQUE_MASK.trailing_zeros())
}

fn bb_enable(r: pac::wifi_bb::baseband_watchdog_enable::R) -> u32 {
    (u32::from(r.watchdog_timeout_clear_request())
        << pac::wifi_bb::baseband_watchdog_enable::WATCHDOG_TIMEOUT_CLEAR_REQUEST_MASK
            .trailing_zeros())
        | (u32::from(r.watchdog_interrupt_enable())
            << pac::wifi_bb::baseband_watchdog_enable::WATCHDOG_INTERRUPT_ENABLE_MASK
                .trailing_zeros())
        | (u32::from(r.watchdog_reset_enable())
            << pac::wifi_bb::baseband_watchdog_enable::WATCHDOG_RESET_ENABLE_MASK.trailing_zeros())
        | (r.bits_0_28_opaque().get()
            << pac::wifi_bb::baseband_watchdog_enable::BITS_0_28_OPAQUE_MASK.trailing_zeros())
}

fn mac_raw(r: pac::wifi_mac::raw::R) -> u32 {
    (r.bits_0_20_opaque().get() << pac::wifi_mac::raw::BITS_0_20_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.fatal_assertion())
            << pac::wifi_mac::raw::FATAL_ASSERTION_MASK.trailing_zeros())
        | (r.bits_22_31_opaque().get()
            << pac::wifi_mac::raw::BITS_22_31_OPAQUE_MASK.trailing_zeros())
}
fn mac_status(r: pac::wifi_mac::status::R) -> u32 {
    (r.bits_0_4_opaque().get() << pac::wifi_mac::status::BITS_0_4_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.rx_associated_auxiliary_5())
            << pac::wifi_mac::status::RX_ASSOCIATED_AUXILIARY_5_MASK.trailing_zeros())
        | (u32::from(r.bit_6_opaque()) << pac::wifi_mac::status::BIT_6_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.tx_complete()) << pac::wifi_mac::status::TX_COMPLETE_MASK.trailing_zeros())
        | (u32::from(r.bss_color_collision())
            << pac::wifi_mac::status::BSS_COLOR_COLLISION_MASK.trailing_zeros())
        | (r.bits_9_10_opaque().get()
            << pac::wifi_mac::status::BITS_9_10_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.watchdog()) << pac::wifi_mac::status::WATCHDOG_MASK.trailing_zeros())
        | (u32::from(r.bit_12_opaque())
            << pac::wifi_mac::status::BIT_12_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.bit_13_opaque())
            << pac::wifi_mac::status::BIT_13_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.rx_success()) << pac::wifi_mac::status::RX_SUCCESS_MASK.trailing_zeros())
        | (u32::from(r.sta_beacon_filter())
            << pac::wifi_mac::status::STA_BEACON_FILTER_MASK.trailing_zeros())
        | (r.bits_16_18_opaque().get()
            << pac::wifi_mac::status::BITS_16_18_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.tx_timeout()) << pac::wifi_mac::status::TX_TIMEOUT_MASK.trailing_zeros())
        | (u32::from(r.bit_20_opaque())
            << pac::wifi_mac::status::BIT_20_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.fatal_assertion())
            << pac::wifi_mac::status::FATAL_ASSERTION_MASK.trailing_zeros())
        | (u32::from(r.bit_22_opaque())
            << pac::wifi_mac::status::BIT_22_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.bit_23_opaque())
            << pac::wifi_mac::status::BIT_23_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.rx_associated_auxiliary_24())
            << pac::wifi_mac::status::RX_ASSOCIATED_AUXILIARY_24_MASK.trailing_zeros())
        | (r.bits_25_26_opaque().get()
            << pac::wifi_mac::status::BITS_25_26_OPAQUE_MASK.trailing_zeros())
        | (u32::from(r.wdev_timer_0()) << pac::wifi_mac::status::WDEV_TIMER_0_MASK.trailing_zeros())
        | (u32::from(r.wdev_timer_1()) << pac::wifi_mac::status::WDEV_TIMER_1_MASK.trailing_zeros())
        | (r.bits_29_31_opaque().get()
            << pac::wifi_mac::status::BITS_29_31_OPAQUE_MASK.trailing_zeros())
}

fn matrix3(r: pac::interrupt_core::int_status_reg_3::R) -> u32 {
    r.int_status_3() << pac::interrupt_core::int_status_reg_3::INT_STATUS_3_MASK.trailing_zeros()
}
