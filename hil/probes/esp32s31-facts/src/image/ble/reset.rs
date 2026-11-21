//! Reset coverage of the two positively established LP delivery lanes.
use super::{
    Ble, HALF, Session, WAIT_LIMIT, command18, counter, emit, init_lp, pac, pending, target, window,
};

fn state(s: &Session<'_>, tag: u32) {
    let b = &s.ble;
    emit(
        tag,
        counter(b),
        b.timer_status.read_word_0038_opaque().record_word(),
        pending(s),
    );
    emit(
        tag + 1,
        u32::from(b.timer.read_word_0024_opaque().low_byte()),
        u32::from(b.timer.read_word_002c_opaque().low_byte()),
        u32::from(
            s.sar
                .radio
                .modem_syscon_clock_reset
                .read_modem_rst_conf()
                .rst_ble_timer(),
        ),
    );
    emit(
        tag + 2,
        u32::from(
            s.sar
                .radio
                .modem_syscon_clock_reset
                .read_clk_conf()
                .clk_ble_timer_en(),
        ),
        u32::from(
            s.sar
                .clocks
                .modem_lpcon_chip_clocks
                .read_clk_conf()
                .clk_lp_timer_en(),
        ),
        u32::from(
            s.sar
                .radio
                .bluetooth_lp_timer_clock
                .read_lp_timer_conf()
                .clk_lp_timer_sel_xtal(),
        ),
    );
}

/// Gate the Bluetooth LP timer's clocks: every source deselected, the BLE
/// timer clock and the MODEM_PWR LP-timer gate off.
fn lp_clocks_off(s: &mut Session<'_>) {
    // SOURCE: production deselect_bluetooth_low_power_timer_sources:
    // one fresh RMW per selector, in this order. Keep MODEM_PWR APB live.
    s.sar
        .radio
        .bluetooth_lp_timer_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_osc_slow(false);
        });
    s.sar
        .radio
        .bluetooth_lp_timer_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_osc_fast(false);
        });
    s.sar
        .radio
        .bluetooth_lp_timer_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_xtal32k(false);
        });
    s.sar
        .radio
        .bluetooth_lp_timer_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_xtal(false);
        });
    // SOURCE: HAL restore_bluetooth_peripheral_clocks, BLE timer field;
    // chip-clock policy's already-sourced MODEM_PWR LP-timer gate.
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf(|w| {
        w.set_clk_ble_timer_en(pac::modem_syscon::clk_conf::ClkBleTimerEn::Disabled);
    });
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_lp_timer_clock(false);
    oer_portable_riscv::fence();
}

/// Restore the Bluetooth LP timer's clocks: the BLE timer clock, the
/// MODEM_PWR LP-timer gate and the XTAL source.
fn lp_clocks_on(s: &mut Session<'_>) {
    // SOURCE: HAL restore_bluetooth_peripheral_clocks, BLE timer field;
    // chip-clock policy's already-sourced MODEM_PWR LP-timer gate.
    s.sar.radio.modem_syscon_clock_reset.modify_clk_conf(|w| {
        w.set_clk_ble_timer_en(pac::modem_syscon::clk_conf::ClkBleTimerEn::Enabled);
    });
    s.sar
        .clocks
        .modem_lpcon_chip_clocks
        .set_modem_pwr_lp_timer_clock(true);
    s.sar
        .radio
        .bluetooth_lp_timer_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_sel_xtal(true);
        });
    oer_portable_riscv::fence();
}

/// Which Bluetooth domains a case resets: the BLE timer alone, or the
/// controller's domains with it.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Reset {
    Timer,
    Controller,
}

impl Reset {
    /// The record's code: 1 for the controller.
    const fn code(self) -> u32 {
        match self {
            Self::Timer => 0,
            Self::Controller => 1,
        }
    }
}

/// Whether the LP timer's clocks run through the reset or are gated first.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Clocks {
    Running,
    Gated,
}

impl Clocks {
    /// The record's code: 1 when gated.
    const fn code(self) -> u32 {
        match self {
            Self::Running => 0,
            Self::Gated => 1,
        }
    }
}

fn reset(s: &mut Session<'_>, domains: Reset) {
    use pac::modem_syscon::modem_rst_conf as r;
    // SOURCE: HAL modem/syscon.rs::reset_bluetooth_controller_domains.
    // Each RMW and the final device fence retain the production ordering.
    if domains == Reset::Controller {
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_btmac(r::RstBtmac::Asserted);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_btmac(r::RstBtmac::Released);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_btmac_apb(r::RstBtmacApb::Asserted);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_btmac_apb(r::RstBtmacApb::Released);
            });
    }
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_ble_timer(r::RstBleTimer::Asserted);
        });
    s.sar
        .radio
        .modem_syscon_clock_reset
        .modify_modem_rst_conf(|w| {
            w.set_rst_ble_timer(r::RstBleTimer::Released);
        });
    if domains == Reset::Controller {
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_ecb(r::RstModemEcb::Asserted);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_ccm(r::RstModemCcm::Asserted);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_bah(r::RstModemBah::Asserted);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_sec(r::RstModemSec::Asserted);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_ecb(r::RstModemEcb::Released);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_ccm(r::RstModemCcm::Released);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_bah(r::RstModemBah::Released);
            });
        s.sar
            .radio
            .modem_syscon_clock_reset
            .modify_modem_rst_conf(|w| {
                w.set_rst_modem_sec(r::RstModemSec::Released);
            });
    }
    oer_portable_riscv::fence();
}

fn observe_reset(s: &mut Session<'_>, domains: Reset, clocks: Clocks, tag: u32) {
    if clocks == Clocks::Gated {
        lp_clocks_off(s);
    }
    state(s, tag);
    reset(s, domains);
    state(s, tag + 0x10);
    window(4096);
    state(s, tag + 0x20);
    // Clock restoration never sends START, a pending publication or +0x10.
    lp_clocks_on(s);
    state(s, tag + 0x30);
    window(4096);
    state(s, tag + 0x40);
}

fn clear_and_stop(b: &mut Ble) {
    b.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    b.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    b.timer.clear_bluetooth_modem_lp_timer_state_0024();
    b.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_portable_riscv::fence();
}

pub(super) fn run(s: &mut Session<'_>, domains: Reset, clocks: Clocks) {
    emit(0x32f0, domains.code(), clocks.code(), 0);
    // Software-only positive cause: test bit 18 without disabling it first.
    // This diagnostic is distinct from shutdown's compare-disabled admission.
    init_lp(s);
    target(&mut s.ble, HALF);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    command18(&mut s.ble);
    state(s, 0x3200);
    observe_reset(s, domains, clocks, 0x3210);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    state(s, 0x3260);
    // Positive control follows the no-rearm observations, recorded separately.
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    command18(&mut s.ble);
    state(s, 0x3270);
    clear_and_stop(&mut s.ble);

    // One natural rollover, bounded independently of debugger capture time.
    init_lp(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    let start = oer_portable_riscv::cycle32();
    let mut prior = counter(&s.ble);
    let mut wrapped = false;
    while oer_portable_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT {
        let now = counter(&s.ble);
        if now < prior {
            wrapped = true;
            break;
        }
        prior = now;
    }
    emit(
        0x33ff,
        u32::from(wrapped),
        oer_portable_riscv::cycle32().wrapping_sub(start),
        0,
    );
    state(s, 0x3300);
    if !wrapped {
        clear_and_stop(&mut s.ble);
        return;
    }
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    command18(&mut s.ble);
    state(s, 0x3310);
    // Exact production shutdown admission: compare disabled, rollover armed.
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    state(s, 0x3320);
    observe_reset(s, domains, clocks, 0x3330);
    // Distinguish output suppression, counter stop and pending clear.
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    state(s, 0x3380);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    oer_portable_riscv::fence();
    state(s, 0x3390);
    window(4096);
    state(s, 0x33a0);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_portable_riscv::fence();
    state(s, 0x33b0);
    // Publication without rearm, followed by a separate enable control.
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    state(s, 0x33c0);
    command18(&mut s.ble);
    state(s, 0x33d0);
    clear_and_stop(&mut s.ble);
    state(s, 0x33e0);
}
