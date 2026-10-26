//! Bounded BTLC prerequisite; no RF, scheduler RUN or DMA publication.
use super::{Session, ble, contain, emit, pac};
use oer_mmio::Bits;

mod lp;
mod mac;
pub(super) use lp::{lp_clear, shutdown_soft, shutdown_wrap, sleep_other, sleep16};
pub(super) use mac::{arrivals, controls, groups, mapping, masks};

const LIMIT: u32 = 200_000;

fn window(cycles: u32) {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < cycles {}
}

fn capture(s: &mut Session<'_>, tag: u32) -> Option<u32> {
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_latch_request(true);
    });
    oer_riscv::fence();
    let start = oer_riscv::cycle32();
    while s.ble.core.read_btlc_timer_control().latch_request()
        && oer_riscv::cycle32().wrapping_sub(start) < LIMIT
    {}
    let control = s.ble.core.read_btlc_timer_control();
    let value = s.ble.core.read_btlc_timer_capture_0().value();
    emit(
        tag,
        u32::from(control.latch_request()),
        u32::from(control.timer_arm()) + 2 * u32::from(control.config_24()),
        oer_riscv::cycle32().wrapping_sub(start),
    );
    emit(
        tag + 1,
        value,
        s.ble.core.read_btlc_timer_capture_1().value(),
        s.ble.core.read_btlc_timer_capture_2().value(),
    );
    if control.latch_request() {
        None
    } else {
        Some(value)
    }
}

fn setup(s: &mut Session<'_>) {
    oer_riscv::disable_interrupts();
    contain(s);
    ble::clocks(&mut s.ble);
    // SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, production
    // modem::clock_device and configure_modem_source_clocks. Digital clocks
    // only: no front-end ADC/DAC, analog calibration or radio operation.
    s.ble.upstream.open_modem_pll_source_gate();
    s.ble.clocks.modify_clk_conf1(|w| {
        w.set_clk_wifibb_80x1_en(pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled);
    });
    s.ble.clocks.modify_clk_conf(|w| {
        w.set_clk_etm_en(pac::modem_syscon::clk_conf::ClkEtmEn::Enabled)
            .set_clk_modem_sec_apb_en(pac::modem_syscon::clk_conf::ClkModemSecApbEn::Enabled)
            .set_clk_modem_sec_ecb_en(pac::modem_syscon::clk_conf::ClkModemSecEcbEn::Enabled)
            .set_clk_modem_sec_ccm_en(pac::modem_syscon::clk_conf::ClkModemSecCcmEn::Enabled);
    });
    // SOURCE: production Bluetooth controller reset domains. No shared BTBB
    // reset is needed; hart 1 remains contained and no client is running.
    s.ble.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Asserted);
    });
    s.ble.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Released)
            .set_rst_btmac_apb(pac::modem_syscon::modem_rst_conf::RstBtmacApb::Asserted);
    });
    s.ble.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(pac::modem_syscon::modem_rst_conf::RstBtmacApb::Released)
            .set_rst_ble_timer(pac::modem_syscon::modem_rst_conf::RstBleTimer::Asserted);
    });
    s.ble.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(pac::modem_syscon::modem_rst_conf::RstBleTimer::Released);
    });
    oer_riscv::fence();
    let m = s.ble.upstream.read_modem_conf();
    let c = s.ble.clocks.read_clk_conf1();
    let r = s.ble.clocks.read_modem_rst_conf();
    emit(
        0x9000,
        u32::from(m.modem_clk_source_sel()),
        u32::from(m.modem_pll_clk_en()),
        u32::from(c.clk_wifibb_80x1_en()),
    );
    emit(
        0x9001,
        u32::from(r.rst_btmac()),
        u32::from(r.rst_btmac_apb()),
        u32::from(r.rst_ble_timer()),
    );
    // SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS positional prefix, and
    // libbtdm_common[7.o] r_sym_bt_aGdrujd2MUAzWYH75baR+c0..de.
    s.ble.positional.write_value(|w| {
        w.set_bit_0_opaque(false).set_bit_3_opaque(false);
    });
    s.ble.positional.write_value(|w| {
        w.set_bit_0_opaque(true).set_bit_3_opaque(false);
    });
    s.ble.core.modify_hal_init_latch(|w| {
        w.set_config_6(true).set_enable_31(false);
    });
    s.ble.core.write_hal_init_low_20(|w| {
        w.set_config_low_20(Bits::constant::<
            { pac::bt_mac::hal_init_low_20::CONFIG_LOW_20_MAX },
        >());
    });
    s.ble.core.modify_hal_init_latch(|w| {
        w.set_enable_31(true);
    });
    // SOURCE: r_obd_update_btlc_timer +4/+8/+c/+1c. These are positional
    // seed values, not a declared physical time encoding.
    s.ble.core.modify_btlc_timer_value_0(|w| {
        w.set_value(0x10000);
    });
    s.ble.core.modify_btlc_timer_value_1(|w| {
        w.set_value(0);
    });
    s.ble.core.modify_btlc_timer_value_2(|w| {
        w.set_value(0);
    });
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_clock_shift(Bits::constant::<2>())
            .set_config_24(true)
            .set_timer_arm(true);
    });
    oer_riscv::fence();
}

pub(super) fn prerequisite(s: &mut Session<'_>) {
    setup(s);
    let first = capture(s, 0x9010);
    window(4096);
    let second = capture(s, 0x9020);
    emit(
        0x9030,
        u32::from(first.is_some()),
        u32::from(second.is_some()),
        match (first, second) {
            (Some(a), Some(b)) => b.wrapping_sub(a),
            _ => 0,
        },
    );
    // SOURCE: current HAL standalone shift=3; its upper byte is preserved.
    // The older scale-sixteen initialization clears CONFIG_24. Separate this
    // sourced mode from the earlier probe's scale-eight CONFIG_24=1 recipe.
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_clock_shift(Bits::constant::<3>())
            .set_config_24(false);
    });
    pair(s, 0x9040);
    // SOURCE: current controller-enable leaf writes CONTROL0=1 before its
    // public interrupt allocation. This command publishes no scheduler RUN.
    s.ble.bank.write_irq_control_0(|w| {
        w.set_command(pac::bt_mac::irq_control_0::Command::Command1.bits());
    });
    oer_riscv::fence();
    pair(s, 0x9080);
    // SOURCE: current BTDM runtime preparation. Establish its separate LP
    // counter prerequisite without PHY/RF initialization or an IRQ route.
    let b = &mut s.ble;
    b.lp_clock.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_osc_slow(false)
            .set_clk_lp_timer_sel_osc_fast(false)
            .set_clk_lp_timer_sel_xtal32k(false)
            .set_clk_lp_timer_sel_xtal(true)
            .set_clk_lp_timer_div_num(Bits::constant::<1>());
    });
    b.timer.write_word_004c_opaque(|w| {
        w.set_bits_0_31_opaque(0);
    });
    b.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(true);
    });
    b.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    b.timer.prepare_bluetooth_modem_lp_timer_command_0008();
    b.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    b.timer.prepare_bluetooth_modem_lp_timer_command_0034();
    b.timer.clear_bluetooth_modem_lp_timer_state_0024();
    b.timer.clear_bluetooth_modem_lp_timer_state_002c();
    b.timer.write_runtime_timer_start_command(
        pac::modem_pwr::runtime_timer_start_command::Command::Start,
        |_| {},
    );
    oer_riscv::fence();
    pair(s, 0x90c0);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_riscv::fence();
}

fn pair(s: &mut Session<'_>, tag: u32) {
    let first = capture(s, tag);
    window(4096);
    let second = capture(s, tag + 0x10);
    emit(
        tag + 0x20,
        u32::from(first.is_some()),
        u32::from(second.is_some()),
        match (first, second) {
            (Some(a), Some(b)) => b.wrapping_sub(a),
            _ => 0,
        },
    );
}
