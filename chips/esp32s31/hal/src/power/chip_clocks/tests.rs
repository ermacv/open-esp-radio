//! HAL word atoms and their installed-owner authority.
use super::{ClockCounts, ClockWords, PsramMpll};
use crate::power::{PlatformClock, PlatformClockBackend, PlatformClockError};
use oer_esp32s31_pac::{hp_sys_clkrst::HpSysClkrstPll160, modem_lpcon::ModemLpconChipClocks};
use std::{vec, vec::Vec};

/// `MODEM_LPCON` up to `CLK_CONF` and `HP_SYS_CLKRST` up to `REF_160M_CTRL0`.
fn memory() -> (Vec<u32>, Vec<u32>) {
    (vec![0xa5a5_a5a0; 0x20 / 4], vec![0xa5a5_a400; 0x184 / 4])
}

#[test]
fn the_words_keep_every_other_bit_and_set_each_gate() {
    let (mut lpcon, mut clkrst) = memory();
    {
        let words = ClockWords::host(&mut lpcon, &mut clkrst).expect("memory holds the words");
        let mut counts = ClockCounts::adopt(words, PsramMpll::adopted());
        for clock in PlatformClock::ALL {
            counts.acquire(clock).expect("granted");
        }
    }
    {
        let lpcon = ModemLpconChipClocks::host(&mut lpcon).expect("memory holds the word");
        let gates = lpcon.read_clk_conf();
        assert!(gates.clk_wifipwr_en());
        assert!(gates.clk_coex_en());
        assert!(gates.clk_i2c_mst_en());
        assert!(gates.clk_lp_timer_en());
        let clkrst = HpSysClkrstPll160::host(&mut clkrst).expect("memory holds the word");
        let reference = clkrst.read_ref_160m_ctrl0();
        assert!(reference.reg_ref_160m_clk_en());
        assert_eq!(
            reference.reg_ref_160m_clk_div_num(),
            0,
            "the divider keeps its value"
        );
    }
    {
        let words = ClockWords::host(&mut lpcon, &mut clkrst).expect("memory holds the words");
        let mut counts = ClockCounts::adopt(words, PsramMpll::adopted());
        for clock in PlatformClock::ALL {
            counts.acquire(clock).expect("granted");
            counts.release(clock).expect("held");
        }
    }
    let lpcon_view = ModemLpconChipClocks::host(&mut lpcon).expect("memory holds the word");
    let gates = lpcon_view.read_clk_conf();
    assert!(
        gates.clk_wifipwr_en(),
        "nothing closes the Wi-Fi power gate"
    );
    assert!(!gates.clk_coex_en() && !gates.clk_i2c_mst_en() && !gates.clk_lp_timer_en());
    let clkrst_view = HpSysClkrstPll160::host(&mut clkrst).expect("memory holds the word");
    assert!(!clkrst_view.read_ref_160m_ctrl0().reg_ref_160m_clk_en());
}

#[test]
fn an_uninstalled_owner_refuses_every_request() {
    // No host test installs the chip's words.
    for clock in PlatformClock::ALL {
        assert_eq!(
            <super::ChipClocks as PlatformClockBackend>::acquire(clock),
            Err(PlatformClockError)
        );
        assert_eq!(
            <super::ChipClocks as PlatformClockBackend>::release(clock),
            Err(PlatformClockError)
        );
    }
}
