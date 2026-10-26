//! Isolated BLE interrupt facts. No RF engine, DMA publication or CPU rerouting.
use super::{Session, contain, emit};
use oer_esp32s31_pac as pac;
use oer_mmio::Bits;

mod arrival;
mod reset;

pub(super) use arrival::{compare_ack, two_wraps};

pub(super) fn timer_reset_on(s: &mut Session<'_>) {
    reset::run(s, false, false);
}
pub(super) fn timer_reset_off(s: &mut Session<'_>) {
    reset::run(s, false, true);
}
pub(super) fn controller_reset_on(s: &mut Session<'_>) {
    reset::run(s, true, false);
}
pub(super) fn controller_reset_off(s: &mut Session<'_>) {
    reset::run(s, true, true);
}

pub(super) struct Ble {
    pub bank: pac::bt_mac::BluetoothInterruptBank,
    pub core: pac::bt_mac::BluetoothControllerCore,
    pub timer: pac::modem_pwr::BtdmRuntimeControl,
    pub timer_status: pac::modem_pwr::BtdmLpTimerStatus,
    pub clocks: pac::modem_syscon::ModemSysconRadio,
    pub lp_clock: pac::modem_lpcon::ModemLpconSharedClock,
    pub gates: pac::modem_lpcon::ModemLpconChipClocks,
    pub upstream: pac::hp_sys_clkrst::HpSysClkrstRadio,
    pub pmu: pac::pmu::PmuRadio,
    pub watchdog: pac::rtc_wdt::LpWdt,
    pub positional: pac::bt_mac::BleHwPositionalWord1ff0,
    pub etm: pac::modem_etm::ModemEtmBluetooth,
    pub task_irq: pac::bt_mac::BtmacBlePhyInit,
}
const COUNTER_MAX: u32 = pac::modem_pwr::word_0048_opaque::COUNTER_LOW_MAX;
const HALF: u32 = (COUNTER_MAX / 2) + 1;
// A failed stimulus terminates after a finite observation, never a soak.
const WAIT_LIMIT: u32 = 40_000_000;

fn window(cycles: u32) {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < cycles {}
}

pub(super) fn clocks(b: &mut Ble) {
    // Retain the ROM's reset backstop; feed once per bounded leaf so debugger
    // command dwell does not consume its boot deadline. No host WDT MMIO.
    b.watchdog.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    b.watchdog.trigger_feed_feed();
    b.watchdog.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
    // SOURCE: HAL modem::platform / modem::ops upstream register-clock and
    // PMU ICG prerequisites. Use XTAL, without RF/PLL calibration or enable.
    b.pmu.select_hp_active_modem_icg();
    b.pmu.trigger_imm_modem_icg_update_dig_icg_modem_en();
    b.pmu.trigger_imm_sleep_sysclk_update_dig_icg_switch();
    b.upstream.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled);
    });
    b.upstream.modify_modem_conf(|w| {
        w.set_modem_apb_clk_en(true)
            .set_modem_rst_en(false)
            .set_modem_clk_en(true)
            .set_modem_clk_source_sel(false)
            .set_modem_xtal_clk_en(true);
    });
    // SOURCE: HAL modem::ops Bluetooth APB/MAC/timer gates and power-state map;
    // no reset and no bank enable write precede the inherited observation.
    b.clocks.modify_clk_conf_power_st(|w| {
        w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
        w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
        w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
    });
    b.clocks.modify_clk_conf1(|w| {
        w.set_clk_bt_apb_en(pac::modem_syscon::clk_conf1::ClkBtApbEn::Enabled);
        w.set_clk_btmac_en(pac::modem_syscon::clk_conf1::ClkBtmacEn::Enabled);
        w.set_clk_btbb_en(pac::modem_syscon::clk_conf1::ClkBtbbEn::Enabled);
    });
    b.clocks.modify_clk_conf(|w| {
        w.set_clk_ble_timer_en(pac::modem_syscon::clk_conf::ClkBleTimerEn::Enabled);
    });
    b.lp_clock.modify_clk_conf_power_st(|w| {
        w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
        w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
    });
    b.gates.set_modem_pwr_wifi_power_clock(true);
    b.gates.set_modem_pwr_lp_timer_clock(true);
    oer_riscv::fence();
}

fn enables(b: &Ble, tag: u32) {
    let e = b.bank.read_irq_enable_1();
    emit(
        tag,
        u32::from(e.source_1()),
        u32::from(e.source_8()) + 2 * u32::from(e.source_9()) + 4 * u32::from(e.source_12()),
        u32::from(b.bank.read_irq_enable_0().source_15()),
    );
}

pub(super) fn inherited(s: &mut Session<'_>) {
    contain(s);
    let m = s.ble.upstream.read_modem_conf();
    emit(
        0x200f,
        u32::from(m.modem_apb_clk_en()),
        u32::from(m.modem_rst_en()),
        u32::from(s.ble.upstream.read_modem_ctrl0().reg_modem_clk_en()),
    );
    enables(&s.ble, 0x2000);
    clocks(&mut s.ble);
    enables(&s.ble, 0x2001);
    nrt_state(s, 0x2002);
    let m = s.ble.upstream.read_modem_conf();
    emit(
        0x201f,
        u32::from(m.modem_apb_clk_en()),
        u32::from(m.modem_rst_en()),
        u32::from(s.ble.upstream.read_modem_ctrl0().reg_modem_clk_en()),
    );
    baseline(&mut s.ble, true);
    enables(&s.ble, 0x2003);
    baseline(&mut s.ble, false);
    enables(&s.ble, 0x2004);
}

fn nrt_state(s: &Session<'_>, tag: u32) {
    emit(
        tag + 0x100,
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
        tag,
        s.ble.bank.read_irq_status_snapshot_0().pending_bits(),
        s.ble.bank.read_irq_status_snapshot_1().pending_bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
}

pub(super) fn bit_one(s: &mut Session<'_>) {
    contain(s);
    clocks(&mut s.ble);
    let inherited = s.ble.bank.read_irq_enable_1().source_1();
    let e0 = s.ble.bank.read_irq_enable_0();
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_0(true).set_source_1(true);
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_source_1(true);
    });
    enables(&s.ble, 0x2700);
    timer_stimulus(&mut s.ble, false);
    nrt_state(s, 0x2701);
    clear(&mut s.ble);
    nrt_state(s, 0x2702);
    timer_stimulus(&mut s.ble, true);
    nrt_state(s, 0x2703);
    clear(&mut s.ble);
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_source_1(inherited);
    });
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_0(e0.source_0()).set_source_1(e0.source_1());
    });
    nrt_state(s, 0x2704);
}

fn baseline(b: &mut Ble, enabled: bool) {
    b.bank.modify_irq_enable_0(|w| {
        w.set_source_15(enabled);
    });
    b.bank.modify_irq_enable_1(|w| {
        w.set_source_8(enabled)
            .set_source_9(enabled)
            .set_source_12(enabled);
    });
    oer_riscv::fence();
}

fn clear(b: &mut Ble) {
    let a = b.bank.sample_bluetooth_interrupt_bank_0();
    let c = b.bank.sample_bluetooth_interrupt_bank_1();
    b.bank.acknowledge_bluetooth_interrupt_bank_0(a);
    b.bank.acknowledge_bluetooth_interrupt_bank_1(c);
    oer_riscv::fence();
}

fn timer_stimulus(b: &mut Ble, second: bool) {
    // SOURCE: BT_MAC timer model. This is a local candidate producer, not an
    // assumed cause identity. No scheduler RUN or controller pointer is used.
    if second {
        b.core.write_btlc_timer_cc_1(|w| {
            w.set_value(64);
        });
    } else {
        b.core.write_btlc_timer_cc_0(|w| {
            w.set_value(64);
        });
    }
    b.core.modify_btlc_timer_value_0(|w| {
        w.set_value(0);
    });
    b.core.modify_btlc_timer_value_1(|w| {
        w.set_value(0);
    });
    b.core.modify_btlc_timer_value_2(|w| {
        w.set_value(0);
    });
    b.core.modify_btlc_timer_control(|w| {
        w.set_clock_shift(Bits::constant::<2>())
            .set_config_24(true)
            .set_timer_arm(true);
    });
    window(4096);
    b.core.modify_btlc_timer_control(|w| {
        w.set_latch_request(true);
    });
    window(4096);
    let c = b.core.read_btlc_timer_control();
    emit(
        0x21f0,
        u32::from(c.timer_arm()),
        u32::from(c.latch_request()),
        b.core.read_btlc_timer_capture_0().value(),
    );
}

pub(super) fn nrt_controls(s: &mut Session<'_>) {
    contain(s);
    clocks(&mut s.ble);
    enables(&s.ble, 0x2100);
    baseline(&mut s.ble, true);
    s.ble.bank.write_irq_control_0(|w| {
        w.set_command(pac::bt_mac::irq_control_0::Command::Command1.bits());
    });
    timer_stimulus(&mut s.ble, false);
    nrt_state(s, 0x2101);
    baseline(&mut s.ble, false);
    nrt_state(s, 0x2102);
    baseline(&mut s.ble, true);
    nrt_state(s, 0x2103);
    s.ble.bank.write_irq_control_1(|w| {
        w.set_command(pac::bt_mac::irq_control_1::Command::Command1.bits());
    });
    nrt_state(s, 0x2104);
    s.ble.bank.write_irq_control_2(|w| {
        w.set_command(pac::bt_mac::irq_control_2::Command::Command1.bits());
    });
    nrt_state(s, 0x2105);
    clear(&mut s.ble);
    nrt_state(s, 0x2106);
    s.ble.bank.write_irq_control_0(|w| {
        w.set_command(pac::bt_mac::irq_control_0::Command::Command1.bits());
    });
    timer_stimulus(&mut s.ble, false);
    nrt_state(s, 0x2107);
    s.ble.bank.write_irq_control_2(|w| {
        w.set_command(pac::bt_mac::irq_control_2::Command::Command1.bits());
    });
    nrt_state(s, 0x2108);
    clear(&mut s.ble);
    baseline(&mut s.ble, false);
}

pub(super) fn nrt_arrivals(s: &mut Session<'_>) {
    contain(s);
    clocks(&mut s.ble);
    clear(&mut s.ble);
    timer_stimulus(&mut s.ble, false);
    let a = s.ble.bank.sample_bluetooth_interrupt_bank_0();
    timer_stimulus(&mut s.ble, true);
    let b = s.ble.bank.sample_bluetooth_interrupt_bank_1();
    emit(0x2200, a.bits(), b.bits(), 0);
    nrt_state(s, 0x2201);
    timer_stimulus(&mut s.ble, false);
    s.ble.bank.acknowledge_bluetooth_interrupt_bank_0(a);
    s.ble.bank.acknowledge_bluetooth_interrupt_bank_1(b);
    nrt_state(s, 0x2202);
    clear(&mut s.ble);
    nrt_state(s, 0x2203);
}

fn counter(b: &Ble) -> u32 {
    b.timer.read_word_0048_opaque().counter_low().get()
}
fn pending(s: &Session<'_>) -> u32 {
    s.interrupt_matrix
        .interrupt_core0_status
        .read_int_status_reg_3()
        .int_status_3()
}
fn lp_state(s: &Session<'_>, tag: u32) {
    // Also test whether these known local LP causes appear in either MAC
    // bank/NRT output, without changing any source's route or acknowledge.
    nrt_state(s, tag + 0x200);
    let b = &s.ble;
    emit(
        tag,
        counter(b),
        b.timer_status.read_word_0038_opaque().bits_0_31_opaque(),
        pending(s),
    );
    emit(
        tag + 0x100,
        u32::from(b.timer.read_word_0024_opaque().low_byte()),
        u32::from(b.timer.read_word_002c_opaque().low_byte()),
        b.timer.read_word_006c_opaque().bits_0_31_opaque(),
    );
}
pub(super) fn init_lp(s: &mut Session<'_>) {
    contain(s);
    let b = &mut s.ble;
    clocks(b);
    b.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(pac::modem_syscon::modem_rst_conf::RstBleTimer::Asserted);
    });
    b.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(pac::modem_syscon::modem_rst_conf::RstBleTimer::Released);
    });
    // SOURCE: HAL modem LP-clock selectors. Divider 1, selected by the
    // counter-controls silicon observation, advances the counter. Divider 0
    // does not: do not assume a divide-by-one interpretation for zero.
    b.lp_clock.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_osc_slow(false)
            .set_clk_lp_timer_sel_osc_fast(false)
            .set_clk_lp_timer_sel_xtal32k(false)
            .set_clk_lp_timer_sel_xtal(true)
            .set_clk_lp_timer_div_num(Bits::constant::<1>());
    });
    // SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL / current L1 preparation prefix.
    let conf = b.lp_clock.read_lp_timer_conf();
    let gates = b.gates.read_clk_conf();
    emit(
        0x2f00,
        u32::from(conf.clk_lp_timer_sel_xtal()),
        conf.clk_lp_timer_div_num().get(),
        u32::from(gates.clk_wifipwr_en()) + 2 * u32::from(gates.clk_lp_timer_en()),
    );
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
    b.timer.write_word_0010_opaque(|w| {
        w.set_bit_1_opaque(true);
    });
    b.timer.clear_bluetooth_modem_lp_timer_state_0024();
    b.timer.clear_bluetooth_modem_lp_timer_state_002c();
    start_counter(b);
    emit(
        0x2f01,
        u32::from(b.timer.read_word_0078_opaque().control_25()),
        b.lp_clock
            .read_clk_conf_power_st()
            .clk_lp_apb_st_map()
            .get(),
        0,
    );
    oer_riscv::fence();
}
fn start_counter(b: &mut Ble) {
    b.timer.write_runtime_timer_start_command(
        pac::modem_pwr::runtime_timer_start_command::Command::Start,
        |_| {},
    );
}
fn target(b: &mut Ble, delta: u32) {
    let value = counter(b).wrapping_add(delta) & COUNTER_MAX;
    b.timer.modify_word_006c_opaque(|w| {
        w.set_bits_0_31_opaque(value);
    });
}
fn command18(b: &mut Ble) {
    b.timer.write_word_0010_opaque(|w| {
        w.set_bit_18_opaque(true);
    });
    oer_riscv::fence();
}
fn wait_cause(s: &Session<'_>) {
    let start = oer_riscv::cycle32();
    while s
        .ble
        .timer_status
        .read_word_0038_opaque()
        .bits_0_31_opaque()
        == 0
        && oer_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT
    {}
    emit(0x23ff, oer_riscv::cycle32().wrapping_sub(start), 0, 0);
}

pub(super) fn lp_causes(s: &mut Session<'_>) {
    init_lp(s);
    lp_state(s, 0x2300);
    target(&mut s.ble, 4096);
    command18(&mut s.ble);
    wait_cause(s);
    lp_state(s, 0x2301);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    lp_state(s, 0x2302);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    lp_state(s, 0x2303);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    lp_state(s, 0x2304);
    command18(&mut s.ble);
    lp_state(s, 0x2305);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    lp_state(s, 0x2306);
    command18(&mut s.ble);
    lp_state(s, 0x2308);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    lp_state(s, 0x2307);
}

pub(super) fn lp_stop(s: &mut Session<'_>) {
    init_lp(s);
    target(&mut s.ble, 4096);
    command18(&mut s.ble);
    wait_cause(s);
    lp_state(s, 0x2400);
    let before = counter(&s.ble);
    window(4096);
    let running = counter(&s.ble);
    let start = oer_riscv::cycle32();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    oer_riscv::fence();
    let duration = oer_riscv::cycle32().wrapping_sub(start);
    let stopped = counter(&s.ble);
    window(4096);
    emit(0x2401, before, running, duration);
    emit(0x2402, stopped, counter(&s.ble), 0);
    lp_state(s, 0x2403);
    start_counter(&mut s.ble);
    let resume = counter(&s.ble);
    window(4096);
    emit(0x2404, resume, counter(&s.ble), 0);
    lp_state(s, 0x2405);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
}

pub(super) fn lp_wrap(s: &mut Session<'_>) {
    init_lp(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    lp_state(s, 0x2500);
    let start = oer_riscv::cycle32();
    let mut prior = counter(&s.ble);
    let mut wraps = 0_u32;
    while wraps < 1 && oer_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT {
        let now = counter(&s.ble);
        if now < prior {
            wraps = wraps.saturating_add(1);
            lp_state(s, 0x2500 + wraps);
        }
        prior = now;
    }
    emit(0x250f, wraps, oer_riscv::cycle32().wrapping_sub(start), 0);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    lp_state(s, 0x2503);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    lp_state(s, 0x2504);
}

pub(super) fn lp_modes(s: &mut Session<'_>) {
    init_lp(s);
    target(&mut s.ble, HALF);
    command18(&mut s.ble);
    wait_cause(s);
    lp_state(s, 0x2600);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    lp_state(s, 0x2601);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_1_opaque(true);
    });
    lp_state(s, 0x2602);
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(true).set_control_4(true);
    });
    lp_state(s, 0x2603);
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_1(true);
    });
    lp_state(s, 0x2604);
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(false).set_control_4(false);
    });
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    target(&mut s.ble, COUNTER_MAX.saturating_sub(1024));
    command18(&mut s.ble);
    window(4096);
    lp_state(s, 0x2605);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
}

/// Distinguish a stopped counter from an invalid accelerated clock setup.
pub(super) fn counter_controls(s: &mut Session<'_>) {
    init_lp(s);
    for (index, divider) in [399_u32, 1, 0].into_iter().enumerate() {
        s.ble.lp_clock.modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_div_num(Bits::truncate(divider));
        });
        start_counter(&mut s.ble);
        let a = counter(&s.ble);
        window(4096);
        emit(0x2800 + index as u32, divider, a, counter(&s.ble));
        s.ble.timer.modify_word_0058_opaque(|w| {
            w.set_control_0(true).set_control_4(true);
        });
        let a = counter(&s.ble);
        window(4096);
        emit(0x2810 + index as u32, divider, a, counter(&s.ble));
        s.ble.timer.modify_word_0058_opaque(|w| {
            w.set_control_0(false).set_control_4(false);
        });
    }
    lp_state(s, 0x2820);
}

/// A positive, peer-free source isolates masks, retained pending and A3.
pub(super) fn software_pending(s: &mut Session<'_>) {
    init_lp(s);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    command18(&mut s.ble);
    lp_state(s, 0x2900);
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(false).set_control_4(false);
    });
    lp_state(s, 0x2901);
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(true).set_control_4(true);
    });
    lp_state(s, 0x2902);
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_1(true);
    });
    lp_state(s, 0x2903);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    lp_state(s, 0x2904);
    command18(&mut s.ble);
    lp_state(s, 0x2905);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    lp_state(s, 0x2906);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    lp_state(s, 0x2907);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    lp_state(s, 0x2908);
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_1_opaque(true);
    });
    lp_state(s, 0x2909);
    command18(&mut s.ble);
    lp_state(s, 0x290a);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    oer_riscv::fence();
    lp_state(s, 0x290b);
    start_counter(&mut s.ble);
    window(4096);
    lp_state(s, 0x290c);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
}

/// One natural wrap, followed by independent tests of the two live causes.
pub(super) fn rollover_gates(s: &mut Session<'_>) {
    init_lp(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    let start = oer_riscv::cycle32();
    let mut prior = counter(&s.ble);
    let mut wrapped = false;
    while oer_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT {
        let now = counter(&s.ble);
        if now < prior {
            wrapped = true;
            break;
        }
        prior = now;
    }
    emit(
        0x30ff,
        u32::from(wrapped),
        oer_riscv::cycle32().wrapping_sub(start),
        0,
    );
    lp_state(s, 0x3000);
    if !wrapped {
        return;
    }
    target(&mut s.ble, HALF);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    command18(&mut s.ble);
    lp_state(s, 0x3001);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    lp_state(s, 0x3002);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    oer_riscv::fence();
    lp_state(s, 0x3003);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    lp_state(s, 0x3004);
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_1_opaque(true);
    });
    lp_state(s, 0x3005);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    lp_state(s, 0x3006);
    command18(&mut s.ble);
    lp_state(s, 0x3007);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    lp_state(s, 0x3008);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
}

/// Minimal sourced controller prefix for the otherwise inactive BTLC candidate.
/// No descriptor, SRAM pointer, scheduler RUN, PHY route or RF publication.
pub(super) fn btlc_prefix(s: &mut Session<'_>) {
    contain(s);
    clocks(&mut s.ble);
    let b = &mut s.ble;
    b.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Asserted);
    });
    b.clocks.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Released);
    });
    // SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS and the HAL controller-init
    // latch prefix. These observed images do not imply a cause identity.
    b.positional.write_value(|w| {
        w.set_bit_0_opaque(false).set_bit_3_opaque(false);
    });
    b.positional.write_value(|w| {
        w.set_bit_0_opaque(true).set_bit_3_opaque(false);
    });
    b.core.modify_hal_init_latch(|w| {
        w.set_config_6(true).set_enable_31(false);
    });
    b.core.write_hal_init_low_20(|w| {
        w.set_config_low_20(Bits::constant::<
            { pac::bt_mac::hal_init_low_20::CONFIG_LOW_20_MAX },
        >());
    });
    b.core.modify_hal_init_latch(|w| {
        w.set_enable_31(true);
    });
    baseline(b, true);
    b.bank.modify_irq_enable_1(|w| {
        w.set_source_1(true);
    });
    b.bank.modify_irq_enable_0(|w| {
        w.set_source_0(true).set_source_1(true);
    });
    b.bank.write_irq_control_0(|w| {
        w.set_command(pac::bt_mac::irq_control_0::Command::Command1.bits());
    });
    timer_stimulus(b, false);
    nrt_state(s, 0x3100);
    clear(&mut s.ble);
    nrt_state(s, 0x3101);
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_source_1(false);
    });
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_0(false).set_source_1(false);
    });
    baseline(&mut s.ble, false);
}
