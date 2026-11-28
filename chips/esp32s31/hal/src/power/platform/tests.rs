//! The platform words' gates and their holders.
use super::{PlatformClock, PlatformClocks};
use oer_chips_esp32s31_pac::{hp_sys_clkrst::HpSysClkrstPll160, modem_lpcon::ModemLpconChipClocks};
use std::{vec, vec::Vec};

/// `MODEM_LPCON` up to `CLK_CONF` and `HP_SYS_CLKRST` up to `REF_160M_CTRL0`.
fn memory() -> (Vec<u32>, Vec<u32>) {
    (vec![0xa5a5_a5a0; 0x20 / 4], vec![0xa5a5_a400; 0x184 / 4])
}

const ALL: [PlatformClock; 3] = [
    PlatformClock::Pll160m,
    PlatformClock::AnalogI2cMaster,
    PlatformClock::ModemCoexistence,
];

#[test]
fn the_first_hold_opens_a_gate_and_the_last_closes_it_keeping_every_other_bit() {
    let (mut lpcon, mut clkrst) = memory();
    let mut clocks = PlatformClocks::host(&mut lpcon, &mut clkrst).expect("memory holds the words");
    for clock in ALL {
        assert!(!clocks.gate_open(clock));
        let first = clocks.acquire(clock);
        assert!(clocks.gate_open(clock), "{clock:?}: the first hold opens");
        let second = clocks.acquire(clock);
        clocks.release(first);
        assert!(clocks.gate_open(clock), "{clock:?}: a hold is left");
        assert!(clocks.any_held());
        clocks.release(second);
        assert!(!clocks.gate_open(clock), "{clock:?}: the last hold closes");
    }
    assert!(!clocks.any_held());
    drop(clocks);
    let clkrst = HpSysClkrstPll160::host(&mut clkrst).expect("memory holds the word");
    assert_eq!(
        clkrst.read_ref_160m_ctrl0().record_word(),
        0xa5a5_a400,
        "every other bit of the word keeps its value"
    );
    let lpcon = ModemLpconChipClocks::host(&mut lpcon).expect("memory holds the word");
    assert_eq!(lpcon.read_clk_conf().record_word(), 0xa5a5_a5a0);
}

#[test]
fn the_wifi_power_gate_is_opened_and_closed_by_its_own_edges() {
    let (mut lpcon, mut clkrst) = memory();
    let mut clocks = PlatformClocks::host(&mut lpcon, &mut clkrst).expect("memory holds the words");
    clocks.open_wifi_power();
    assert!(clocks.wifi_power_open());
    assert!(!clocks.any_held(), "the Wi-Fi power gate is no count");
    assert!(ALL.iter().all(|clock| !clocks.gate_open(*clock)));
    clocks.close_wifi_power();
    assert!(!clocks.wifi_power_open());
}

#[test]
fn the_modem_low_power_word_is_read_only_with_the_radio_s_open_bus() {
    use crate::radio::phy::registers::host::with_registers_memory;
    use oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn;
    let (mut lpcon, mut clkrst) = memory();
    let clocks = PlatformClocks::host(&mut lpcon, &mut clkrst).expect("memory holds the words");
    let closed = clocks.observe(&crate::radio::phy::power_words::ModemBus::Closed);
    assert_eq!(closed.modem_lpcon_clk_conf, None);
    assert_eq!(closed.hp_sys_clkrst_ref_160m_ctrl0, 0xa5a5_a400);
    let (open, _) = with_registers_memory(0, |registers| {
        let parts = &mut registers.peripherals;
        parts.hp_sys_clkrst_radio.modify_modem_ctrl0(|w| {
            w.set_reg_modem_clk_en(RegModemClkEn::Enabled);
        });
        parts.hp_sys_clkrst_radio.modify_modem_conf(|w| {
            w.set_modem_apb_clk_en(true).set_modem_rst_en(false);
        });
        let (_, bus) = registers.observe_power();
        assert!(
            matches!(bus, crate::radio::phy::power_words::ModemBus::Open { .. }),
            "the gate words read open"
        );
        clocks.observe(&bus)
    });
    assert_eq!(open.modem_lpcon_clk_conf, Some(0xa5a5_a5a0));
    assert_eq!(open.hp_sys_clkrst_ref_160m_ctrl0, 0xa5a5_a400);
}
