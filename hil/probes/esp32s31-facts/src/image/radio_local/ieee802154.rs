//! The IEEE 802.15.4 experiments of `radio_local` (entry 82), out of the
//! build with their radio: no module declares this file.
use super::*;

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
    oer_portable_riscv::fence();
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
    oer_portable_riscv::fence();
}
/// Which of the IEEE 802.15.4 timers a start arms.
#[derive(Clone, Copy, PartialEq)]
enum ZbTimer {
    First,
    Second,
}

fn zb_start(s: &mut Session<'_>, second: ZbTimer) {
    use pac::ieee802154::command::Opcode;
    let mac = &mut s.radio.zb.ieee802154_mac_task;
    if second == ZbTimer::Second {
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
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let status = s.radio.zb.ieee802154_mac_irq.read_event_status();
        if if second == ZbTimer::Second {
            status.timer1_overflow()
        } else {
            status.timer0_overflow()
        } {
            break;
        }
    }
    s.radio.zb.ieee802154_mac_task.write_command(
        if second == ZbTimer::Second {
            Opcode::Timer1Stop
        } else {
            Opcode::Timer0Stop
        },
        |_| {},
    );
    oer_portable_riscv::fence();
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
        w.set_clk_zb_apb_en(pac::modem_syscon::dictionaries::ClockGate::Enabled)
            .set_clk_zbmac_en(pac::modem_syscon::dictionaries::ClockGate::Enabled);
    });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_zbmac(pac::modem_syscon::dictionaries::Reset::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_zbmac(pac::modem_syscon::dictionaries::Reset::Released)
                .set_rst_zbmac_apb(pac::modem_syscon::dictionaries::Reset::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_zbmac_apb(pac::modem_syscon::dictionaries::Reset::Released);
        });
}

pub(super) fn ieee_timers(s: &mut Session<'_>) {
    ieee_prepare(s);
    zb_mask(s, false);
    zb_clear(s);
    zb_state(s, 0x8210);
    zb_start(s, ZbTimer::First);
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
    oer_portable_riscv::fence();
    zb_mask(s, true);
    zb_state(s, 0x8264);
    zb_start(s, ZbTimer::Second);
    zb_state(s, 0x8270); // timer1 enabled expiry
    zb_start(s, ZbTimer::First);
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
    oer_portable_riscv::fence();
    zb_state(s, 0x8290); // timer0 clear preserves timer1
    zb_clear(s);
    zb_state(s, 0x82a0); // enabled W1C
    zb_mask(s, false);
    zb_start(s, ZbTimer::Second);
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
    oer_portable_riscv::fence();
    zb_mask(s, true);
    zb_state(s, 0x82e0);
    zb_mask(s, false);
}

