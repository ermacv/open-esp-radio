//! The clock owner's counts over recorded gate writes, and its words over
//! host memory read back through typed views of the same words.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{hp_sys_clkrst::HpSysClkrstPll160, modem_lpcon::ModemLpconChipClocks};

use super::{ChipClockError, ClockCounts, ClockGates, ClockWords, PsramMpll};
use crate::power::{PlatformClock, PlatformClockBackend, PlatformClockError};

/// One gate write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Write {
    Pll160m(bool),
    AnalogI2cMaster(bool),
    Coexistence(bool),
    LowPowerTimer(bool),
    WifiPowerOpen,
}

#[derive(Default)]
struct Recorded(Vec<Write>);

impl ClockGates for Recorded {
    fn set_pll_f160m(&mut self, on: bool) {
        self.0.push(Write::Pll160m(on));
    }
    fn set_analog_i2c_master(&mut self, on: bool) {
        self.0.push(Write::AnalogI2cMaster(on));
    }
    fn set_coexistence(&mut self, on: bool) {
        self.0.push(Write::Coexistence(on));
    }
    fn set_low_power_timer(&mut self, on: bool) {
        self.0.push(Write::LowPowerTimer(on));
    }
    fn open_wifi_power(&mut self) {
        self.0.push(Write::WifiPowerOpen);
    }
}

fn adopted() -> ClockCounts<Recorded> {
    ClockCounts::adopt(Recorded::default(), PsramMpll::adopted())
}

#[test]
fn adoption_opens_only_the_wifi_power_gate() {
    let counts = adopted();
    assert_eq!(counts.gates.0, [Write::WifiPowerOpen]);
    for clock in PlatformClock::ALL {
        assert_eq!(counts.count(clock), 0, "{clock:?} has no radio reference");
    }
}

#[test]
fn only_the_first_and_last_reference_write_a_gate() {
    let mut counts = adopted();
    for clock in [
        PlatformClock::Pll160m,
        PlatformClock::AnalogI2cMaster,
        PlatformClock::Pll160m,
        PlatformClock::ModemCoexistence,
        PlatformClock::ModemLowPowerTimer,
    ] {
        counts.acquire(clock).expect("granted");
    }
    for clock in [
        PlatformClock::Pll160m,
        PlatformClock::ModemLowPowerTimer,
        PlatformClock::AnalogI2cMaster,
        PlatformClock::ModemCoexistence,
        PlatformClock::Pll160m,
    ] {
        counts.release(clock).expect("held");
    }
    assert_eq!(
        counts.gates.0,
        [
            Write::WifiPowerOpen,
            Write::Pll160m(true),
            Write::AnalogI2cMaster(true),
            Write::Coexistence(true),
            Write::LowPowerTimer(true),
            Write::LowPowerTimer(false),
            Write::AnalogI2cMaster(false),
            Write::Coexistence(false),
            Write::Pll160m(false),
        ]
    );
}

#[test]
fn mpll_references_never_write_and_never_cross_the_psram_reference() {
    let mut counts = adopted();
    for _ in 0..3 {
        counts.acquire(PlatformClock::Mpll).expect("granted");
    }
    assert_eq!(counts.count(PlatformClock::Mpll), 3);
    for _ in 0..3 {
        counts.release(PlatformClock::Mpll).expect("held");
    }
    assert_eq!(counts.count(PlatformClock::Mpll), 0);
    assert_eq!(
        counts.release(PlatformClock::Mpll),
        Err(ChipClockError::NotHeld(PlatformClock::Mpll)),
        "the PSRAM mapping's reference is not the radio's to give back"
    );
    assert_eq!(
        counts.gates.0,
        [Write::WifiPowerOpen],
        "MPLL powers nothing"
    );
}

#[test]
fn a_release_before_its_acquire_is_refused_and_writes_nothing() {
    let mut counts = adopted();
    for clock in PlatformClock::ALL {
        assert_eq!(counts.release(clock), Err(ChipClockError::NotHeld(clock)));
    }
    counts
        .acquire(PlatformClock::ModemCoexistence)
        .expect("granted");
    counts
        .release(PlatformClock::ModemCoexistence)
        .expect("held");
    assert_eq!(
        counts.release(PlatformClock::ModemCoexistence),
        Err(ChipClockError::NotHeld(PlatformClock::ModemCoexistence)),
        "a second release has nothing to give back"
    );
    assert_eq!(
        counts.gates.0,
        [
            Write::WifiPowerOpen,
            Write::Coexistence(true),
            Write::Coexistence(false),
        ]
    );
}

#[test]
fn an_exhausted_count_refuses_without_a_write() {
    let mut counts = adopted();
    counts.counts.set(PlatformClock::AnalogI2cMaster, u16::MAX);
    assert_eq!(
        counts.acquire(PlatformClock::AnalogI2cMaster),
        Err(ChipClockError::CountExhausted(
            PlatformClock::AnalogI2cMaster
        ))
    );
    assert_eq!(counts.count(PlatformClock::AnalogI2cMaster), u16::MAX);
    assert_eq!(counts.gates.0, [Write::WifiPowerOpen]);
}

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
