//! LP commands and the sourced shutdown register order, without a radio stack.
use super::{Bits, Session, ble, emit, mac, pac, window};
const LOW_MAX: u32 = pac::modem_pwr::word_0048_opaque::COUNTER_LOW_MAX;
const WRAP_LIMIT: u32 = 40_000_000;
fn counter(s: &Session<'_>) -> u32 {
    s.ble.timer.read_word_0048_opaque().counter_low().get()
}
fn state(s: &Session<'_>, tag: u32) {
    let t = &s.ble.timer;
    emit(
        tag,
        counter(s),
        s.ble
            .timer_status
            .read_word_0038_opaque()
            .bits_0_31_opaque(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        u32::from(t.read_word_0024_opaque().low_byte()),
        u32::from(t.read_word_002c_opaque().low_byte()),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    emit(
        tag + 2,
        u32::from(t.read_word_001c_opaque().bits_0_7_opaque()),
        u32::from(t.read_word_0020_opaque().bits_0_7_opaque()),
        u32::from(s.ble.task_irq.read_interrupt_enable().source_17()),
    );
    let m = t.read_word_0058_opaque();
    emit(
        tag + 3,
        u32::from(m.control_0()) + 4 * u32::from(m.control_2()) + 16 * u32::from(m.control_4()),
        t.read_word_0064_opaque().bits_0_23_opaque().get(),
        t.read_word_006c_opaque().bits_0_31_opaque(),
    );
}
fn quiet(s: &mut Session<'_>) {
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    oer_riscv::fence();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_riscv::fence();
}
fn enable18(s: &mut Session<'_>) {
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_18_opaque(true);
    });
    oer_riscv::fence();
}
fn enable1(s: &mut Session<'_>) {
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_1_opaque(true);
    });
    oer_riscv::fence();
}
fn wrap(s: &Session<'_>, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    let mut prior = counter(s);
    while oer_riscv::cycle32().wrapping_sub(start) < WRAP_LIMIT {
        let now = counter(s);
        if now < prior {
            emit(tag, prior, now, oer_riscv::cycle32().wrapping_sub(start));
            return true;
        }
        prior = now;
    }
    emit(tag, prior, counter(s), 0);
    false
}
pub(in crate::image) fn lp_clear(s: &mut Session<'_>) {
    lp_clear_run(s, true);
}
pub(in crate::image) fn lp_masked(s: &mut Session<'_>) {
    lp_clear_run(s, false);
}
fn lp_clear_run(s: &mut Session<'_>, initial_clear: bool) {
    ble::init_lp(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    if !wrap(s, 0x96ff) {
        quiet(s);
        return;
    }
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    enable18(s);
    state(s, 0x9600);
    if initial_clear {
        s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
        oer_riscv::fence();
        state(s, 0x9608);
    }
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    state(s, 0x9610);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    enable18(s);
    state(s, 0x9620);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    enable1(s);
    state(s, 0x9630);
    // A fresh software cause separately checks the same +0x34 image after
    // masked acknowledgement of the real rollover. No synthetic wrap byte.
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    enable18(s);
    state(s, 0x9640);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
    oer_riscv::fence();
    state(s, 0x9650);
    quiet(s);
    state(s, 0x9660);
}
fn future(s: &mut Session<'_>) -> u32 {
    let target = counter(s).wrapping_add(512) & LOW_MAX;
    s.ble.timer.modify_word_0064_opaque(|w| {
        w.set_bits_0_23_opaque(Bits::truncate(target));
    });
    s.ble.timer.modify_word_006c_opaque(|w| {
        w.set_bits_0_31_opaque(LOW_MAX);
    });
    target
}
fn wait_target(s: &Session<'_>, origin: u32) -> bool {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 200_000 {
        if counter(s).wrapping_sub(origin) & LOW_MAX >= 1024 {
            return true;
        }
    }
    false
}
pub(in crate::image) fn sleep16(s: &mut Session<'_>) {
    ble::init_lp(s);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    for mode in 0..4 {
        s.ble.timer.disable_bluetooth_lp_sleep_16();
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
        s.ble.timer.modify_word_0058_opaque(|w| {
            w.set_control_0(mode == 1 || mode == 3)
                .set_control_4(mode == 2 || mode == 3);
        });
        let origin = counter(s);
        let target = future(s);
        emit(0x970f + mode * 0x30, mode, origin, target);
        state(s, 0x9700 + mode * 0x30);
        s.ble.timer.enable_bluetooth_lp_sleep_16();
        oer_riscv::fence();
        let completed = wait_target(s, origin);
        emit(0x971e + mode * 0x30, u32::from(completed), counter(s), 0);
        state(s, 0x9710 + mode * 0x30);
        s.ble.timer.disable_bluetooth_lp_sleep_16();
        state(s, 0x9720 + mode * 0x30);
    }
    if s.ble.timer.read_word_0058_opaque().control_2() {
        s.ble.timer.trigger_word_0058_opaque_control_1();
        state(s, 0x97d0);
    }
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
    quiet(s);
    state(s, 0x97e0);
}
pub(in crate::image) fn sleep_other(s: &mut Session<'_>) {
    ble::init_lp(s);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    state(s, 0x9800);
    // SOURCE: complete boolean helper, including the MODEM_ETM writes that
    // precede/follow the MODEM_PWR operations. +0x30 has no read accessor.
    s.ble.etm.publish_bluetooth_lp_sleep_etm_0110();
    s.ble.timer.enable_bluetooth_lp_sleep_0();
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_bit_26_opaque(true);
    });
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0030();
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(true);
    });
    window(4096);
    state(s, 0x9810);
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(false);
    });
    s.ble.timer.disable_bluetooth_lp_sleep_0();
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_bit_26_opaque(false);
    });
    s.ble.etm.publish_bluetooth_lp_sleep_etm_0114();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0030();
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(true);
    });
    state(s, 0x9820);
    s.ble.etm.trigger_channel_enable_set_ch7();
    s.ble.etm.trigger_channel_enable_set_ch6();
    s.ble.etm.trigger_channel_enable_set_bit_11_opaque();
    s.ble.timer.enable_bluetooth_lp_sleep_17();
    oer_riscv::fence();
    window(4096);
    state(s, 0x9830);
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    state(s, 0x9840);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0020();
    s.ble.etm.trigger_channel_enable_clear_ch6();
    s.ble.etm.trigger_channel_enable_clear_ch7();
    state(s, 0x9850);
    let target = counter(s).wrapping_add(512) & LOW_MAX;
    s.ble.timer.write_word_0068_opaque(|w| {
        w.set_bits_0_23_opaque(Bits::truncate(target));
    });
    let origin = counter(s);
    emit(0x986f, target, origin, 0);
    let completed = wait_target(s, origin);
    emit(0x987f, u32::from(completed), counter(s), 0);
    state(s, 0x9870);
    quiet(s);
    state(s, 0x9880);
}
fn reset(s: &mut Session<'_>, tag: u32) {
    use pac::modem_syscon::modem_rst_conf as r;
    // SOURCE: production reset_bluetooth_controller_domains, unchanged order.
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(r::RstBtmac::Asserted);
    });
    state(s, tag);
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(r::RstBtmac::Released);
    });
    state(s, tag + 8);
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(r::RstBtmacApb::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(r::RstBtmacApb::Released);
    });
    state(s, tag + 16);
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(r::RstBleTimer::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(r::RstBleTimer::Released);
    });
    state(s, tag + 24);
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ecb(r::RstModemEcb::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ccm(r::RstModemCcm::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_bah(r::RstModemBah::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_sec(r::RstModemSec::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ecb(r::RstModemEcb::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ccm(r::RstModemCcm::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_bah(r::RstModemBah::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_sec(r::RstModemSec::Released);
    });
    oer_riscv::fence();
    state(s, tag + 32);
}
fn shutdown(s: &mut Session<'_>, rollover: bool) {
    ble::init_lp(s);
    if !mac::prepare(s) {
        quiet(s);
        mac::finish(s);
        return;
    }
    mac::enable(s, 2, true);
    if !mac::produce(s, 2, 0x99ff) {
        quiet(s);
        mac::finish(s);
        return;
    }
    if rollover {
        s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
        if !wrap(s, 0x99fe) {
            quiet(s);
            mac::finish(s);
            return;
        }
    } else {
        s.ble
            .timer
            .publish_bluetooth_modem_lp_timer_software_pending();
        enable18(s);
    }
    state(s, 0x9900);
    // Primary retirement keeps inherited source1, as in production 0x1300.
    s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_0();
    s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_1();
    state(s, 0x9910);
    mac::command(s, 1);
    state(s, 0x9920);
    mac::command(s, 2);
    state(s, 0x9930);
    mac::group(s, 0, false);
    mac::group(s, 1, false);
    state(s, 0x9940);
    // SOURCE: #685 execute_modem_lp_timer_stop, exact register order.
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    state(s, 0x9950);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    oer_riscv::fence();
    state(s, 0x9960);
    emit(
        0x996f,
        u32::from(s.ble.timer.read_word_0024_opaque().low_byte()),
        u32::from(s.ble.timer.read_word_002c_opaque().low_byte()),
        0,
    );
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_riscv::fence();
    state(s, 0x9970);
    reset(s, 0x9a00);
    // Production reunite performs no MMIO; this marks the corresponding PAC
    // boundary. No HAL/runtime owner is manufactured by this facts image.
    state(s, 0x9a30);
    window(4096);
    state(s, 0x9a40);
    // Digital clock release/restoration diagnostic. No gated MAC reads and
    // no START, publication or enable command accompany restoration.
    let saved0 = s.sar.radio.modem_syscon_radio.read_clk_conf();
    let saved1 = s.sar.radio.modem_syscon_radio.read_clk_conf1();
    s.sar
        .radio
        .modem_lpcon_shared_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_osc_slow(false)
                .set_clk_lp_timer_sel_osc_fast(false)
                .set_clk_lp_timer_sel_xtal32k(false)
                .set_clk_lp_timer_sel_xtal(false);
        });
    s.sar.radio.modem_syscon_radio.modify_clk_conf(|w| {
        w.set_clk_ble_timer_en(pac::modem_syscon::clk_conf::ClkBleTimerEn::Disabled);
    });
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_lp_timer_clock(false);
    // SOURCE: production Bluetooth module clock-device release actions; this
    // isolated image has no other radio client. Observe the matrix only until
    // its APB clock is restored. No RF front-end gate is touched.
    s.sar.radio.modem_syscon_radio.modify_clk_conf(|w| {
        w.set_clk_etm_en(pac::modem_syscon::clk_conf::ClkEtmEn::Disabled)
            .set_clk_modem_sec_apb_en(pac::modem_syscon::clk_conf::ClkModemSecApbEn::Disabled)
            .set_clk_modem_sec_ecb_en(pac::modem_syscon::clk_conf::ClkModemSecEcbEn::Disabled)
            .set_clk_modem_sec_ccm_en(pac::modem_syscon::clk_conf::ClkModemSecCcmEn::Disabled);
    });
    s.sar.radio.modem_syscon_radio.modify_clk_conf1(|w| {
        w.set_clk_btmac_en(pac::modem_syscon::clk_conf1::ClkBtmacEn::Disabled)
            .set_clk_bt_apb_en(pac::modem_syscon::clk_conf1::ClkBtApbEn::Disabled)
            .set_clk_btbb_en(pac::modem_syscon::clk_conf1::ClkBtbbEn::Disabled)
            .set_clk_wifibb_80x1_en(pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Disabled);
    });
    s.sar
        .radio
        .hp_sys_clkrst_radio
        .close_modem_pll_source_gate();

    oer_riscv::fence();
    emit(
        0x9a50,
        0,
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    s.sar.radio.hp_sys_clkrst_radio.open_modem_pll_source_gate();
    s.sar.radio.modem_syscon_radio.modify_clk_conf1(|w| {
        w.set_clk_btmac_en(if saved1.clk_btmac_en() {
            pac::modem_syscon::clk_conf1::ClkBtmacEn::Enabled
        } else {
            pac::modem_syscon::clk_conf1::ClkBtmacEn::Disabled
        })
        .set_clk_bt_apb_en(if saved1.clk_bt_apb_en() {
            pac::modem_syscon::clk_conf1::ClkBtApbEn::Enabled
        } else {
            pac::modem_syscon::clk_conf1::ClkBtApbEn::Disabled
        })
        .set_clk_btbb_en(if saved1.clk_btbb_en() {
            pac::modem_syscon::clk_conf1::ClkBtbbEn::Enabled
        } else {
            pac::modem_syscon::clk_conf1::ClkBtbbEn::Disabled
        })
        .set_clk_wifibb_80x1_en(if saved1.clk_wifibb_80x1_en() {
            pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled
        } else {
            pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Disabled
        });
    });
    s.sar.radio.modem_syscon_radio.modify_clk_conf(|w| {
        w.set_clk_etm_en(if saved0.clk_etm_en() {
            pac::modem_syscon::clk_conf::ClkEtmEn::Enabled
        } else {
            pac::modem_syscon::clk_conf::ClkEtmEn::Disabled
        })
        .set_clk_modem_sec_apb_en(if saved0.clk_modem_sec_apb_en() {
            pac::modem_syscon::clk_conf::ClkModemSecApbEn::Enabled
        } else {
            pac::modem_syscon::clk_conf::ClkModemSecApbEn::Disabled
        })
        .set_clk_modem_sec_ecb_en(if saved0.clk_modem_sec_ecb_en() {
            pac::modem_syscon::clk_conf::ClkModemSecEcbEn::Enabled
        } else {
            pac::modem_syscon::clk_conf::ClkModemSecEcbEn::Disabled
        })
        .set_clk_modem_sec_ccm_en(if saved0.clk_modem_sec_ccm_en() {
            pac::modem_syscon::clk_conf::ClkModemSecCcmEn::Enabled
        } else {
            pac::modem_syscon::clk_conf::ClkModemSecCcmEn::Disabled
        });
    });
    s.sar.radio.modem_syscon_radio.modify_clk_conf(|w| {
        w.set_clk_ble_timer_en(pac::modem_syscon::clk_conf::ClkBleTimerEn::Enabled);
    });
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_lp_timer_clock(true);
    s.sar
        .radio
        .modem_lpcon_shared_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_xtal(true);
        });
    oer_riscv::fence();
    state(s, 0x9a60);
    window(4096);
    state(s, 0x9a70);
    quiet(s);
    mac::finish(s);
}
pub(in crate::image) fn shutdown_soft(s: &mut Session<'_>) {
    shutdown(s, false);
}
pub(in crate::image) fn shutdown_wrap(s: &mut Session<'_>) {
    shutdown(s, true);
}
