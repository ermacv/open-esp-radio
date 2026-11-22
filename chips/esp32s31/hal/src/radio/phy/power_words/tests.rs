use super::*;
use crate::radio::phy::registers::host::with_registers_memory;

/// A word no gate field of the host memory decides: every lane the same.
const FILL: u32 = 0x5a5a_5a5a;

/// The modem register bus set as `open` says: clock, APB clock and reset
/// through their own fields, every other bit as the fill left it.
fn gate<P: RadioPlacement>(
    registers: &mut RadioPhyRegisters<P>,
    clock: bool,
    apb: bool,
    reset: bool,
) {
    let parts = &mut registers.peripherals;
    parts.hp_sys_clkrst_radio.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(if clock {
            RegModemClkEn::Enabled
        } else {
            RegModemClkEn::Disabled
        });
    });
    parts.hp_sys_clkrst_radio.modify_modem_conf(|w| {
        w.set_modem_apb_clk_en(apb).set_modem_rst_en(reset);
    });
}

#[test]
fn the_modem_words_are_read_only_behind_an_open_register_bus() {
    for (clock, apb, reset, open) in [
        (true, true, false, true),
        (false, true, false, false),
        (true, false, false, false),
        (true, true, true, false),
    ] {
        let ((words, modem), _) = with_registers_memory(FILL, |registers| {
            gate(registers, clock, apb, reset);
            let (words, bus) = registers.observe_power();
            let modem = match bus {
                ModemBus::Open { words, .. } => Some(words),
                ModemBus::Closed => None,
            };
            (words, modem)
        });
        assert_eq!(
            modem.is_some(),
            open,
            "clock {clock}, apb {apb}, reset {reset}"
        );
        // The PMU's words answer whatever the gate: as the memory holds them.
        assert_eq!(
            [
                words.pmu_hp_active_icg_modem,
                words.pmu_hp_active_hp_ck_power,
                words.pmu_rf_pwc,
                words.pmu_ana_peri_pwr_ctrl,
            ],
            [FILL; 4]
        );
        if let Some(modem) = modem {
            assert_eq!(modem.syscon_clk_conf, FILL);
            assert_eq!(modem.lpcon_tick_conf, FILL);
        }
    }
}

#[test]
fn a_snapshot_writes_nothing() {
    let (before, after) = {
        let (_, before) =
            with_registers_memory(FILL, |registers| gate(registers, true, true, false));
        let (_, after) = with_registers_memory(FILL, |registers| {
            gate(registers, true, true, false);
            let _ = registers.observe_power();
        });
        (before, after)
    };
    assert_eq!(before, after, "every part's memory as the gate left it");
}
