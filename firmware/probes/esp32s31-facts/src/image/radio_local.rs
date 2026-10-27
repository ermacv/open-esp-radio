//! Local timer causes only: no RF operation or DMA pointer publication.
use super::{Session, ble, contain, emit, pac};
use oer_mmio::{Bits, Index};

pub(super) struct Radio {
    pub local_time: pac::modem_pwr::WifiMacLocalTime,
    pub status: pac::modem_pwr::WifiMacTsfStatus,
    pub zb: pac::ownership::Ieee802154Peripherals,
    pub power: pac::modem_pwr::WifiMacPowerInterrupt,
    pub timer: pac::modem_pwr::WifiMacTsfTimer,
    pub load: pac::modem_pwr::WifiMacTsfLoad,
    pub rtc: pac::modem_pwr::WifiMacRtcTimerUpdate,
    pub aux: pac::modem_pwr::WifiMacAuxTsfControl,
    pub idle: pac::ieee802154::SharedBasebandResetIdleStatus,
    pub tbtt: pac::modem_pwr::WifiMacStaTbttTarget,
}
pub(super) mod hw30;
pub(super) mod wave2;

const LOCAL_LIMIT: u32 = 200_000;
const TIMER0: Index<{ pac::modem_pwr::control::COUNT }> = Index::constant::<0>();
const TIMER1: Index<{ pac::modem_pwr::control::COUNT }> = Index::constant::<1>();

fn window() {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}

fn prepare(s: &mut Session<'_>) {
    oer_riscv::disable_interrupts();
    contain(s);
    ble::clocks(s);
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
        w.set_clk_wifimac_en(pac::modem_syscon::clk_conf1::ClkWifimacEn::Enabled)
            .set_clk_wifi_apb_en(pac::modem_syscon::clk_conf1::ClkWifiApbEn::Enabled)
            .set_clk_wifibb_80x1_en(pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled);
    });
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf(|w| {
        w.set_clk_etm_en(pac::modem_syscon::clk_conf::ClkEtmEn::Enabled)
            .set_clk_modem_sec_apb_en(pac::modem_syscon::clk_conf::ClkModemSecApbEn::Enabled);
    });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_wifimac(pac::modem_syscon::modem_rst_conf::RstWifimac::Released);
        });
    oer_riscv::fence();
    let c = s.sar.radio.modem_syscon_clock_reset.read_clk_conf1();
    let r = s.sar.radio.modem_syscon_clock_reset.read_modem_rst_conf();
    emit(
        0x8201,
        u32::from(c.clk_wifimac_en()),
        u32::from(c.clk_wifibb_80x1_en()),
        u32::from(r.rst_wifimac()),
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

fn zb_mask(s: &mut Session<'_>, enabled: bool) {
    s.radio.zb.ieee802154_mac_task.modify_event_enable(|w| {
        w.set_tx_done(false)
            .set_rx_done(false)
            .set_ack_tx_done(false)
            .set_ack_rx_done(false)
            .set_rx_abort(false)
            .set_tx_abort(false)
            .set_ed_done(false)
            .set_events_7(false)
            .set_timer0_overflow(enabled)
            .set_timer1_overflow(enabled)
            .set_clock_count_match(false)
            .set_tx_sfd_done(false)
            .set_rx_sfd_done(false)
            .set_events_13(false);
    });
    oer_riscv::fence();
}
fn zb_state(s: &Session<'_>, tag: u32) {
    let mask = s.radio.zb.ieee802154_mac_task.read_event_enable();
    let status = s.radio.zb.ieee802154_mac_irq.read_event_status();
    emit(
        tag,
        u32::from(status.timer0_overflow()) + 2 * u32::from(status.timer1_overflow()),
        u32::from(mask.timer0_overflow()) + 2 * u32::from(mask.timer1_overflow()),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    emit(
        tag + 1,
        s.radio.zb.ieee802154_mac_task.read_timer0_value().value(),
        s.radio.zb.ieee802154_mac_task.read_timer1_value().value(),
        0,
    );
}
fn zb_clear(s: &mut Session<'_>) {
    let snapshot = s.radio.zb.ieee802154_mac_irq.sample_event_status();
    s.radio
        .zb
        .ieee802154_mac_irq
        .acknowledge_event_status(snapshot);
    oer_riscv::fence();
}
fn zb_start(s: &mut Session<'_>, second: bool) {
    use pac::ieee802154::command::Opcode;
    let mac = &mut s.radio.zb.ieee802154_mac_task;
    if second {
        mac.write_timer1_threshold(|w| {
            w.set_threshold(32);
        });
        mac.write_command(Opcode::Timer1Start, |_| {});
    } else {
        mac.write_timer0_threshold(|w| {
            w.set_threshold(32);
        });
        mac.write_command(Opcode::Timer0Start, |_| {});
    }
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let status = s.radio.zb.ieee802154_mac_irq.read_event_status();
        if if second {
            status.timer1_overflow()
        } else {
            status.timer0_overflow()
        } {
            break;
        }
    }
    s.radio.zb.ieee802154_mac_task.write_command(
        if second {
            Opcode::Timer1Stop
        } else {
            Opcode::Timer0Stop
        },
        |_| {},
    );
    oer_riscv::fence();
}

fn ieee_prepare(s: &mut Session<'_>) {
    prepare(s);
    // SOURCE: HAL modem::ops IEEE MAC/APB clocks and reset order. No RF gate,
    // PHY calibration, RX_START, ED_START or TX_START is needed for this leaf.
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_clk_conf_power_st(|w| {
            w.set_clk_zb_st_map(Bits::truncate(w.clk_zb_st_map().get() | 4));
        });
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf(|w| {
        w.set_clk_zb_apb_en(pac::modem_syscon::clk_conf::ClkZbApbEn::Enabled)
            .set_clk_zbmac_en(pac::modem_syscon::clk_conf::ClkZbmacEn::Enabled);
    });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_zbmac(pac::modem_syscon::modem_rst_conf::RstZbmac::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_zbmac(pac::modem_syscon::modem_rst_conf::RstZbmac::Released)
                .set_rst_zbmac_apb(pac::modem_syscon::modem_rst_conf::RstZbmacApb::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_zbmac_apb(pac::modem_syscon::modem_rst_conf::RstZbmacApb::Released);
        });
}

pub(super) fn ieee_timers(s: &mut Session<'_>) {
    ieee_prepare(s);
    zb_mask(s, false);
    zb_clear(s);
    zb_state(s, 0x8210);
    zb_start(s, false);
    zb_state(s, 0x8220); // expiry while ENA=0
    zb_mask(s, true);
    zb_state(s, 0x8230); // retained cause delivers while matrix route is parked
    zb_mask(s, false);
    zb_state(s, 0x8240);
    zb_clear(s);
    zb_state(s, 0x8250); // W1C while masked
    zb_mask(s, true);
    zb_state(s, 0x8260);
    // EVENT_STATUS may be masked on read. Hold a positive affine sample
    // across masking to distinguish its acknowledgement from a zero sample.
    let retained = s.radio.zb.ieee802154_mac_irq.sample_event_status();
    zb_mask(s, false);
    s.radio
        .zb
        .ieee802154_mac_irq
        .acknowledge_event_status(retained);
    oer_riscv::fence();
    zb_mask(s, true);
    zb_state(s, 0x8264);
    zb_start(s, true);
    zb_state(s, 0x8270); // timer1 enabled expiry
    zb_start(s, false);
    zb_state(s, 0x8280); // distinct pending bits
    let selected = s
        .radio
        .zb
        .ieee802154_mac_irq
        .sample_event_status()
        .retain_timer1_overflow(false);
    s.radio
        .zb
        .ieee802154_mac_irq
        .acknowledge_event_status(selected);
    oer_riscv::fence();
    zb_state(s, 0x8290); // timer0 clear preserves timer1
    zb_clear(s);
    zb_state(s, 0x82a0); // enabled W1C
    zb_mask(s, false);
    zb_start(s, true);
    zb_state(s, 0x82b0); // timer1 also latches while masked
    zb_clear(s);
    zb_state(s, 0x82c0);
    zb_mask(s, true);
    zb_state(s, 0x82d0);
    let retained = s.radio.zb.ieee802154_mac_irq.sample_event_status();
    zb_mask(s, false);
    s.radio
        .zb
        .ieee802154_mac_irq
        .acknowledge_event_status(retained);
    oer_riscv::fence();
    zb_mask(s, true);
    zb_state(s, 0x82e0);
    zb_mask(s, false);
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
    oer_riscv::fence();
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
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
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
    oer_riscv::fence();
}

fn wifi_prepare(s: &mut Session<'_>) {
    prepare(s);
    // SOURCE: HAL platform::system_timer::open_clock. The ROM leaves these
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
    let start = oer_riscv::cycle32();
    while !s
        .generic
        .systimer
        .systimer_counter
        .read_unit0_op()
        .timer_unit0_value_valid()
        && oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT
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
    oer_riscv::fence();
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
    oer_riscv::fence();
    wifi_state(s, 0x8350); // sampled W1C with mask=0
    wifi_mask(s, true);
    wifi_state(s, 0x8360);
    wifi_start(s, true);
    wifi_state(s, 0x8370);
    let selected = s.radio.power.sample_mac_power_interrupt();
    s.radio.power.acknowledge_mac_power_interrupt(selected);
    oer_riscv::fence();
    wifi_state(s, 0x8380); // enabled W1C
    wifi_mask(s, false);
    wifi_start(s, false);
    wifi_state(s, 0x8390);
    s.radio.power.clear_all_mac_power_interrupts();
    oer_riscv::fence();
    wifi_state(s, 0x83a0); // production mask/all-clear policy
}
