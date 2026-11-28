//! The radio's platform clocks: the gate of the 160 MHz reference
//! (`HP_SYS_CLKRST.REF_160M_CTRL0`) and the analog-I2C master's,
//! coexistence's and Wi-Fi power's gates (`MODEM_LPCON.CLK_CONF`).
//!
//! At run time the radio's owner is these words' one writer: the build's
//! only writers of them are this module's, on the radio's level
//! (`research/449-report-2026-11-28.md`, §2, found by the compiler). So
//! the split hands them to the radio domain's shared registers, and the
//! domain counts each clock's holders itself. Its holders are common power
//! and the modem clock dependency that needs the clock, at most
//! [`HOLDERS`] of them, so a count can neither wrap nor be refused. A
//! hold is a value: [`PlatformClocks::acquire`] gives a [`PlatformHold`]
//! and only [`PlatformClocks::release`] takes it, so no release lacks its
//! acquire.
//!
//! What each gate's edge does:
//!
//! - **160 MHz reference**: the gate opens on the first hold and closes on
//!   the last. Its parent BBPLL is the bootstrap's and stays powered.
//! - **Analog-I2C master, coexistence**: the root's clock links of
//!   `I2C_ANA_MST` (`master`) and `COEX` (`function`), through their
//!   generated controls.
//! - **Wi-Fi power**: opened first by Wi-Fi's power-up and closed last by
//!   its power-down ([`Self::open_wifi_power`], [`Self::close_wifi_power`]).
//!
//! No edge waits: a gate takes effect with its write.

use crate::radio::phy::power_words::ModemBus;
use oer_chips_esp32s31_pac::{
    hp_sys_clkrst::HpSysClkrstPll160, instances, modem_lpcon::ModemLpconChipClocks,
    ownership::ChipClockPeripherals,
};
use oer_portable_mmio::{Fixed, Placement};

#[cfg(test)]
mod tests;

/// A clock whose gate lies in a word the radio's platform clocks hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformClock {
    /// The 160 MHz reference that feeds the modem PLL source.
    Pll160m,
    /// The analog-I2C master clock gate in `MODEM_LPCON.CLK_CONF`.
    AnalogI2cMaster,
    /// The modem coexistence clock gate in `MODEM_LPCON.CLK_CONF`.
    ModemCoexistence,
}

/// The most holders a platform clock has: common power and the one modem
/// clock dependency that needs it (`Pll160AndModemSource`,
/// `AnalogI2cMaster`, `Coexistence`).
pub(crate) const HOLDERS: u8 = 2;

const _: () = if HOLDERS == u8::MAX {
    panic!("a platform clock's count must not reach its type's end")
};

/// One hold of a platform clock: made only by [`PlatformClocks::acquire`]
/// and taken only by [`PlatformClocks::release`].
#[derive(Debug)]
#[must_use = "a hold keeps its clock on until it is released"]
pub(crate) struct PlatformHold {
    clock: PlatformClock,
}

/// The radio's platform clocks as its holders reach them: the domain's
/// [`PlatformClocks`], or a test's record of the edges.
pub(crate) trait PlatformGates {
    /// Take a hold of `clock`.
    fn acquire(&mut self, clock: PlatformClock) -> PlatformHold;
    /// Give `hold` back.
    fn release(&mut self, hold: PlatformHold);
}

impl<P: Placement, Q: Placement> PlatformGates for PlatformClocks<P, Q> {
    fn acquire(&mut self, clock: PlatformClock) -> PlatformHold {
        PlatformClocks::acquire(self, clock)
    }
    fn release(&mut self, hold: PlatformHold) {
        PlatformClocks::release(self, hold);
    }
}

#[cfg(test)]
impl PlatformHold {
    /// A hold a test's gates make for `clock`.
    pub(crate) const fn for_test(clock: PlatformClock) -> Self {
        Self { clock }
    }

    /// The clock this hold keeps.
    pub(crate) const fn clock(&self) -> PlatformClock {
        self.clock
    }
}

/// Wi-Fi's power gate, opened by its power-up and closed by its power-down.
pub(crate) trait WifiPowerGate {
    fn open_wifi_power(&mut self);
    fn close_wifi_power(&mut self);
}

impl<P: Placement, Q: Placement> WifiPowerGate for PlatformClocks<P, Q> {
    fn open_wifi_power(&mut self) {
        PlatformClocks::open_wifi_power(self);
    }
    fn close_wifi_power(&mut self) {
        PlatformClocks::close_wifi_power(self);
    }
}

/// Each platform clock's holders.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Counts {
    pll160m: u8,
    analog_i2c: u8,
    coexistence: u8,
}

impl Counts {
    fn of(&mut self, clock: PlatformClock) -> &mut u8 {
        match clock {
            PlatformClock::Pll160m => &mut self.pll160m,
            PlatformClock::AnalogI2cMaster => &mut self.analog_i2c,
            PlatformClock::ModemCoexistence => &mut self.coexistence,
        }
    }
}

/// The words of the platform clocks and each clock's holders.
pub(crate) struct PlatformClocks<
    P: Placement = Fixed<{ instances::modem_lpcon::BASE }>,
    Q: Placement = Fixed<{ instances::hp_sys_clkrst::BASE }>,
> {
    lpcon: ModemLpconChipClocks<P>,
    pll_f160m: HpSysClkrstPll160<Q>,
    counts: Counts,
}

impl PlatformClocks {
    /// The words with no holder. This performs no MMIO.
    pub(crate) fn new(parts: ChipClockPeripherals) -> Self {
        let ChipClockPeripherals {
            modem_lpcon_chip_clocks,
            hp_sys_clkrst_pll160,
        } = parts;
        Self {
            lpcon: modem_lpcon_chip_clocks,
            pll_f160m: hp_sys_clkrst_pll160,
            counts: Counts::default(),
        }
    }
}

#[cfg(test)]
impl<'a, 'b> PlatformClocks<oer_portable_mmio::Host<'a>, oer_portable_mmio::Host<'b>> {
    /// The words over host memory from each instance's base.
    pub(crate) fn host(
        modem_lpcon: &'a mut [u32],
        hp_sys_clkrst: &'b mut [u32],
    ) -> Result<Self, oer_portable_mmio::HostTooSmall> {
        Ok(Self {
            lpcon: ModemLpconChipClocks::host(modem_lpcon)?,
            pll_f160m: HpSysClkrstPll160::host(hp_sys_clkrst)?,
            counts: Counts::default(),
        })
    }
}

#[cfg(test)]
impl<P: Placement, Q: Placement> PlatformClocks<P, Q> {
    /// Whether `clock`'s gate reads open.
    pub(crate) fn gate_open(&self, clock: PlatformClock) -> bool {
        match clock {
            PlatformClock::Pll160m => self.pll_f160m.read_ref_160m_ctrl0().reg_ref_160m_clk_en(),
            PlatformClock::AnalogI2cMaster => self.lpcon.read_clk_conf().clk_i2c_mst_en(),
            PlatformClock::ModemCoexistence => self.lpcon.read_clk_conf().clk_coex_en(),
        }
    }

    /// Whether the Wi-Fi power gate reads open.
    pub(crate) fn wifi_power_open(&self) -> bool {
        self.lpcon.read_clk_conf().clk_wifipwr_en()
    }
}

impl<P: Placement, Q: Placement> PlatformClocks<P, Q> {
    /// Take a hold of `clock`; its first hold opens its gate.
    pub(crate) fn acquire(&mut self, clock: PlatformClock) -> PlatformHold {
        let count = *self.counts.of(clock);
        if count == 0 {
            self.set_gate(clock, true);
        }
        // At most `HOLDERS` holds exist (the const above): no wrap.
        *self.counts.of(clock) = count_after(count, 1);
        PlatformHold { clock }
    }

    /// Give `hold` back; the last hold of its clock closes the gate.
    pub(crate) fn release(&mut self, hold: PlatformHold) {
        let PlatformHold { clock } = hold;
        // A hold exists only after its acquire's increment: no wrap.
        let count = count_after(*self.counts.of(clock), -1);
        *self.counts.of(clock) = count;
        if count == 0 {
            self.set_gate(clock, false);
        }
    }

    /// Whether any clock has a holder.
    pub(crate) fn any_held(&self) -> bool {
        self.counts != Counts::default()
    }

    /// Open the Wi-Fi power gate: the first write of Wi-Fi's power-up.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/rtc_cntl/rtc/esp32s31.rs:670`
    /// and `esp-hal/src/soc/esp32s31/regi2c.rs` (`enable_wifi_power_clock`).
    pub(crate) fn open_wifi_power(&mut self) {
        self.lpcon.set_modem_pwr_wifi_power_clock(true);
    }

    /// Close the Wi-Fi power gate: the last write of Wi-Fi's power-down.
    pub(crate) fn close_wifi_power(&mut self) {
        self.lpcon.set_modem_pwr_wifi_power_clock(false);
    }

    fn set_gate(&mut self, clock: PlatformClock, on: bool) {
        match clock {
            // SOURCE: ESP-IDF 4d80c1b2 `components/esp_hal_clock/esp32s31/include/hal/clk_gate_ll.h:321-327`.
            PlatformClock::Pll160m => self.pll_f160m.modify_ref_160m_ctrl0(|w| {
                w.set_reg_ref_160m_clk_en(on);
            }),
            PlatformClock::AnalogI2cMaster => self.lpcon.set_i2c_ana_mst_master_clock(on),
            PlatformClock::ModemCoexistence => self.lpcon.set_coex_function_clock(on),
        }
    }

    /// The two words as the hardware has them, `MODEM_LPCON.CLK_CONF` only
    /// behind the modem register bus `bus` proves open. Writes nothing.
    pub(crate) fn observe(&self, bus: &ModemBus<'_>) -> ChipClockWords {
        ChipClockWords {
            hp_sys_clkrst_ref_160m_ctrl0: self.pll_f160m.read_ref_160m_ctrl0().record_word(),
            modem_lpcon_clk_conf: match bus {
                ModemBus::Open { proof: _, .. } => Some(self.lpcon.read_clk_conf().record_word()),
                ModemBus::Closed => None,
            },
        }
    }
}

/// `count` moved by `step`, in a count the holders bound (`HOLDERS`).
const fn count_after(count: u8, step: i8) -> u8 {
    count.wrapping_add_signed(step)
}

/// The platform clock words for a diagnostic record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChipClockWords {
    /// The 160 MHz reference's gate.
    pub hp_sys_clkrst_ref_160m_ctrl0: u32,
    /// The modem low-power clock gates; absent while the modem register bus
    /// is closed: not read.
    pub modem_lpcon_clk_conf: Option<u32>,
}
