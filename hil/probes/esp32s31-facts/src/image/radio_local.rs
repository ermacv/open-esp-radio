//! Local timer causes only: no RF operation or DMA pointer publication.
use super::{Session, contain, emit, pac};
use oer_portable_mmio::{Bits, Index};
use pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode;

pub(super) struct Radio {
    pub local_time: pac::modem_pwr::WifiMacLocalTime,
    pub status: pac::modem_pwr::WifiMacTsfStatus,
    pub power: pac::modem_pwr::WifiMacPowerInterrupt,
    pub timer: pac::modem_pwr::WifiMacTsfTimer,
    pub load: pac::modem_pwr::WifiMacTsfLoad,
    pub rtc: pac::modem_pwr::WifiMacRtcTimerUpdate,
    pub aux: pac::modem_pwr::WifiMacAuxTsfControl,
    pub tbtt: pac::modem_pwr::WifiMacStaTbttTarget,
}
pub(super) mod hw30;
pub(super) mod wave2;

const LOCAL_LIMIT: u32 = 200_000;
const TIMER0: Index<{ pac::modem_pwr::control::COUNT }> = Index::constant::<0>();
const TIMER1: Index<{ pac::modem_pwr::control::COUNT }> = Index::constant::<1>();

fn window() {
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < 4096 {}
}

/// The modem's clocks as the Wi-Fi experiments were recorded with: the LP
/// watchdog fed, the modem's ICG, bus and power-state maps, the Wi-Fi power
/// and LP timer clocks.
/// It opens the Bluetooth gates too: their records were taken with them open.
fn modem_clocks(s: &mut Session<'_>) {
    let b = &mut s.watchdog;
    // Retain the ROM's reset backstop; feed once per bounded leaf so debugger
    // command dwell does not consume its boot deadline. No host WDT MMIO.
    b.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    b.trigger_feed_feed();
    b.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
    // SOURCE: HAL modem::platform / modem::ops upstream register-clock and
    // PMU ICG prerequisites. Use XTAL, without RF/PLL calibration or enable.
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
    s.sar.radio.hp_sys_clkrst_radio.modify_modem_conf(|w| {
        w.set_modem_apb_clk_en(true)
            .set_modem_rst_en(false)
            .set_modem_clk_en(true)
            .set_modem_clk_source_sel(false)
            .set_modem_xtal_clk_en(true);
    });
    // SOURCE: HAL modem::ops Bluetooth APB/MAC/timer gates and power-state map;
    // no reset and no bank enable write precede the inherited observation.
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_clk_conf_power_st(|w| {
            w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
            w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
            w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
        });
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf1(|w| {
        w.set_clk_bt_apb_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
        w.set_clk_btmac_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
        w.set_clk_btbb_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
    });
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf(|w| {
        w.set_clk_ble_timer_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
    });
    s.sar.radio.modem_lpcon_power.modify_clk_conf_power_st(|w| {
        w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
        w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
    });
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_wifi_power_clock(true);
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_lp_timer_clock(true);
    oer_portable_riscv::fence();
}

fn prepare(s: &mut Session<'_>) {
    oer_portable_riscv::disable_interrupts();
    contain(s);
    modem_clocks(s);
    // SOURCE: HAL modem::ops::configure_modem_source_clocks. Adopt the
    // production digital source selection without a PHY/RF bring-up.
    s.sar.radio.hp_sys_clkrst_radio.modify_modem_conf(|w| {
        w.set_modem_clk_source_sel(true).set_modem_pll_clk_en(true);
    });
    // SOURCE: HAL configure_fixed_xtal_40mhz_tick / BLOB_LIBPHY_PHY_GET_XTAL_FREQ.
    s.sar.radio.modem_lpcon_phy_tick.modify_tick_conf(|w| {
        w.set_modem_pwr_tick_target(Bits::constant::<39>());
    });
    emit(
        0x8203,
        s.sar
            .radio
            .modem_lpcon_phy_tick
            .read_tick_conf()
            .modem_pwr_tick_target()
            .get(),
        0,
        0,
    );
    // SOURCE: HAL modem::ops module maps and enable_wifi_mac_clocks. These
    // digital clock prerequisites do not start a MAC or enable RF ADC/DAC.
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_clk_conf_power_st(|w| {
            w.set_clk_wifi_st_map(Bits::truncate(w.clk_wifi_st_map().get() | 6));
        });
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf1(|w| {
        w.set_clk_wifimac_en(pac::modem_syscon::dictionaries::ClockGate::Enabled)
            .set_clk_wifi_apb_en(pac::modem_syscon::dictionaries::ClockGate::Enabled)
            .set_clk_wifibb_80x1_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
    });
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf(|w| {
        w.set_clk_etm_en(pac::modem_syscon::dictionaries::ClockGate::Enabled)
            .set_clk_modem_sec_apb_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
    });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_wifimac(pac::modem_syscon::dictionaries::Reset::Released);
        });
    oer_portable_riscv::fence();
    let c = s.sar.radio.modem_syscon_clock_reset.read_clk_conf1();
    let r = s.sar.radio.modem_syscon_clock_reset.read_modem_rst_conf();
    emit(
        0x8201,
        c.clk_wifimac_en().bits(),
        c.clk_wifibb_80x1_en().bits(),
        r.rst_wifimac().bits(),
    );
    let m = s.sar.radio.hp_sys_clkrst_radio.read_modem_conf();
    emit(
        0x8202,
        u32::from(m.modem_clk_source_sel()),
        u32::from(m.modem_pll_clk_en()),
        u32::from(m.modem_xtal_clk_en()),
    );
    for source in [pac::Interrupt::MODEM_ZB_MAC, pac::Interrupt::MODEM_WIFI_PWR] {
        let Ok(index) = Index::new(source as usize) else {
            return;
        };
        s.interrupt_matrix.interrupt_core0.modify_route(index, |w| {
            w.set_map(Bits::constant::<0>());
        });
        emit(
            0x8200 + source as u32,
            source as u32,
            s.interrupt_matrix
                .interrupt_core0
                .read_route(index)
                .map()
                .get(),
            0,
        );
    }
}

fn wifi_mask(s: &mut Session<'_>, enabled: bool) {
    s.radio.power.modify_enable(|w| {
        w.set_tbtt_0(false)
            .set_tbtt_1(false)
            .set_tbtt_2(false)
            .set_tbtt_3(false)
            .set_tsf_timer_0(enabled)
            .set_tsf_timer_1(enabled)
            .set_tsf_timer_2(false)
            .set_tsf_timer_3(false)
            .set_bits_8_16_opaque(Bits::constant::<0>())
            .set_beacon_filter_wake(false)
            .set_bit_18_opaque(false)
            .set_beacon_miss(false)
            .set_bits_20_31_opaque(Bits::constant::<0>());
    });
    oer_portable_riscv::fence();
}
fn wifi_state(s: &mut Session<'_>, tag: u32) {
    let raw = s.radio.power.read_raw();
    emit(
        tag,
        u32::from(raw.tsf_timer_0()) + 2 * u32::from(raw.tsf_timer_1()),
        s.radio.power.sample_mac_power_interrupt().bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(true);
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(false);
    });
    let enable = s.radio.power.read_enable();
    emit(
        tag + 1,
        low,
        high,
        u32::from(enable.tsf_timer_0()) + 2 * u32::from(enable.tsf_timer_1()),
    );
}
fn wifi_start(s: &mut Session<'_>, second: bool) {
    let i = if second { TIMER1 } else { TIMER0 };
    s.radio.timer.modify_control(i, |w| {
        w.set_timer_enable(false).set_wakeup_enable(false);
    });
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(!second)
            .set_snapshot_softap_tsf(second);
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(false)
            .set_snapshot_softap_tsf(false);
    });
    // The model exposes exactly the three high target bits. No raw MMIO mask.
    s.radio.timer.modify_target_n(i, |w| {
        w.set_value(low.wrapping_add(256));
    });
    s.radio.timer.modify_control(i, |w| {
        w.set_target_high(Bits::truncate(high))
            .set_timer_enable(true);
    });
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let raw = s.radio.power.read_raw();
        if if second {
            raw.tsf_timer_1()
        } else {
            raw.tsf_timer_0()
        } {
            break;
        }
    }
    s.radio.timer.modify_control(i, |w| {
        w.set_timer_enable(false);
    });
    oer_portable_riscv::fence();
}

fn wifi_prepare(s: &mut Session<'_>) {
    prepare(s);
    // SOURCE: HAL peripherals::system_timer::open_clock. The ROM leaves these
    // bus/function gates closed; bootstrap normally opens them before MAC use.
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_apb_clk_en(true);
    });
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(true);
    });
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(false);
    });
    s.generic.systimer_clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_clk_en(true);
    });
    s.generic
        .systimer
        .systimer_counter
        .trigger_unit0_op_timer_unit0_update();
    let start = oer_portable_riscv::cycle32();
    while !s
        .generic
        .systimer
        .systimer_counter
        .read_unit0_op()
        .timer_unit0_value_valid()
        && oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT
    {}
    emit(
        0x8300,
        s.generic
            .systimer
            .systimer_counter
            .read_unit0_value_lo()
            .timer_unit0_value_lo(),
        u32::from(
            s.generic
                .systimer
                .systimer_counter
                .read_unit0_op()
                .timer_unit0_value_valid(),
        ),
        s.radio.local_time.read_counter().value(),
    );
    // SOURCE: production select_wifi_low_power_clock(SlowOscillator), then
    // TSF enable/load transactions. Wi-Fi's divider is not the BLE divider.
    s.sar.radio.modem_lpcon_power.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_slow(true)
            .set_clk_wifipwr_lp_sel_osc_fast(false)
            .set_clk_wifipwr_lp_sel_xtal32k(false)
            .set_clk_wifipwr_lp_sel_xtal(false)
            .set_clk_wifipwr_lp_div_num(Bits::constant::<0>());
    });
    emit(0x8303, s.radio.local_time.read_counter().value(), 0, 0);
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tsf_enable_low(true)
            .set_sta_tsf_enable_high(true)
            .set_sta_tbtt_enable(false);
    });
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tsf_mode(pac::modem_pwr::sta_tsf_control::StaTsfMode::Enabled);
    });
    let c = s.radio.rtc.read_sta_tsf_control();
    emit(
        0x8304,
        s.radio.local_time.read_counter().value(),
        c.sta_tsf_mode().get(),
        u32::from(c.sta_tsf_enable_low()) + 2 * u32::from(c.sta_tsf_enable_high()),
    );
    s.radio.aux.modify_softap_control(|w| {
        w.set_tsf_enable(true);
    });
    s.radio.power.clear_all_mac_power_interrupts();
    s.radio.load.write_value_low(|w| {
        w.set_value(0x10000);
    });
    s.radio.load.write_value_high(|w| {
        w.set_value(0);
    });
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_load_station_tsf(true);
    });
    oer_portable_riscv::fence();
    window();
    emit(
        0x8302,
        u32::from(s.radio.status.read_tsf_status().station_active()),
        s.radio.local_time.read_counter().value(),
        u32::from(s.radio.load.read_tsf_load_control().load_station_tsf()),
    );
}

pub(super) fn wifi_timers(s: &mut Session<'_>) {
    wifi_prepare(s);
    wifi_state(s, 0x8310);
    wifi_start(s, false);
    wifi_state(s, 0x8320); // RAW with ENA=0
    wifi_mask(s, true);
    wifi_state(s, 0x8330);
    let selected = s.radio.power.sample_mac_power_interrupt();
    wifi_mask(s, false);
    wifi_state(s, 0x8340);
    s.radio.power.acknowledge_mac_power_interrupt(selected);
    oer_portable_riscv::fence();
    wifi_state(s, 0x8350); // sampled W1C with mask=0
    wifi_mask(s, true);
    wifi_state(s, 0x8360);
    wifi_start(s, true);
    wifi_state(s, 0x8370);
    let selected = s.radio.power.sample_mac_power_interrupt();
    s.radio.power.acknowledge_mac_power_interrupt(selected);
    oer_portable_riscv::fence();
    wifi_state(s, 0x8380); // enabled W1C
    wifi_mask(s, false);
    wifi_start(s, false);
    wifi_state(s, 0x8390);
    s.radio.power.clear_all_mac_power_interrupts();
    oer_portable_riscv::fence();
    wifi_state(s, 0x83a0); // production mask/all-clear policy
}
