//! The chip's platform clock owner: [`ChipClocks`].
//!
//! The radio's platform clocks ([`PlatformClock`]) live in words other SoC
//! users share, so one owner writes each word and counts references for
//! every user; radio modules hold [`PlatformClockGuard`]s, never fields.
//! The reset root installs the words once ([`ChipClocks::install`], from
//! [`Root::into_runtime`](crate::root::Root::into_runtime)) in one
//! [`ClaimCell`]; every operation borrows them for a few register accesses
//! and never waits. The counts are global, so a guard gives its reference
//! back without a borrow of its provider.
//!
//! What each clock's edge does:
//!
//! - **160 MHz PLL output** (`HP_SYS_CLKRST.REF_160M_CTRL0.REG_REF_160M_CLK_EN`,
//!   a HAL field: the root links no instance to it): the gate opens on the
//!   first reference and closes on the last. Its parent BBPLL stays powered
//!   for the whole image, so the edge powers no PLL.
//! - **Analog-I2C master, coexistence, modem low-power timer**
//!   (`MODEM_LPCON.CLK_CONF`): the root's clock links of `I2C_ANA_MST`
//!   (`master`), `COEX` (`function`) and `MODEM_PWR` (`lp-timer`), through
//!   their generated controls.
//! - **MPLL**: the PSRAM mapping holds it for the whole image
//!   ([`PsramMpll`]), so radio references are counted and never power it
//!   on or off; no code path powers MPLL down.
//!
//! Installation opens the Wi-Fi power gate (the root's `MODEM_PWR`
//! `wifi-power` link) once; nothing closes it.
//!
//! No edge waits: a gate takes effect with its write. Order across clocks
//! (the 160 MHz source before the modem PLL-source gate and after it on
//! release) is the modem clock executor's.

use oer_esp32s31_pac::{
    hp_sys_clkrst::HpSysClkrstPll160, instances, modem_lpcon::ModemLpconChipClocks,
    ownership::ChipClockPeripherals,
};
use oer_memory::ClaimCell;
use oer_mmio::{Fixed, Placement};

use super::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider,
};

#[cfg(test)]
mod tests;

/// The permanent MPLL reference of the PSRAM mapping.
///
/// Stage two runs its code and stacks from PSRAM, which the bootstrap maps
/// over MPLL and never unmaps (docs/architecture.md, "Execution"; the
/// bootstrap's `MemoryClockRegisters::power_memory_pll`). Only the reset
/// root makes one, for stage two, and nothing gives it back: while the
/// image runs, the MPLL count of [`ChipClocks`] has this reference under
/// every radio reference, so it never reaches zero.
///
/// ```compile_fail,E0624
/// let _forged = oer_esp32s31_hal::power::PsramMpll::adopted();
/// ```
#[derive(Debug)]
pub struct PsramMpll {
    _private: (),
}

impl PsramMpll {
    /// The reference the stage-two image adopts from its PSRAM mapping.
    pub(crate) const fn adopted() -> Self {
        Self { _private: () }
    }
}

/// Why the clock owner refused a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChipClockError {
    /// A release without a reference to give back.
    NotHeld(PlatformClock),
    /// The count of `clock` cannot grow.
    CountExhausted(PlatformClock),
}

/// The gate writes of the platform clocks.
pub(crate) trait ClockGates {
    /// Open or close the 160 MHz PLL output.
    fn set_pll_f160m(&mut self, on: bool);
    /// Open or close the analog-I2C master clock.
    fn set_analog_i2c_master(&mut self, on: bool);
    /// Open or close the modem coexistence clock.
    fn set_coexistence(&mut self, on: bool);
    /// Open or close the modem low-power timer clock.
    fn set_low_power_timer(&mut self, on: bool);
    /// Open the Wi-Fi power clock.
    fn open_wifi_power(&mut self);
}

/// The words of the platform clocks: `MODEM_LPCON.CLK_CONF` and
/// `HP_SYS_CLKRST.REF_160M_CTRL0`.
pub(crate) struct ClockWords<
    P: Placement = Fixed<{ instances::modem_lpcon::BASE }>,
    Q: Placement = Fixed<{ instances::hp_sys_clkrst::BASE }>,
> {
    lpcon: ModemLpconChipClocks<P>,
    pll_f160m: HpSysClkrstPll160<Q>,
}

impl ClockWords {
    fn new(parts: ChipClockPeripherals) -> Self {
        let ChipClockPeripherals {
            modem_lpcon_chip_clocks,
            hp_sys_clkrst_pll160,
        } = parts;
        Self {
            lpcon: modem_lpcon_chip_clocks,
            pll_f160m: hp_sys_clkrst_pll160,
        }
    }
}

#[cfg(test)]
impl<'a, 'b> ClockWords<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// The words over host memory from each instance's base.
    pub(crate) fn host(
        modem_lpcon: &'a mut [u32],
        hp_sys_clkrst: &'b mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            lpcon: ModemLpconChipClocks::host(modem_lpcon)?,
            pll_f160m: HpSysClkrstPll160::host(hp_sys_clkrst)?,
        })
    }
}

impl<P: Placement, Q: Placement> ClockGates for ClockWords<P, Q> {
    /// SOURCE: ESP-IDF 4d80c1b2
    /// `components/esp_hal_clock/esp32s31/include/hal/clk_gate_ll.h:321-327`
    /// (`_clk_gate_ll_ref_160m_clk_en`); a HAL field, as the root links no
    /// instance to the 160 MHz output.
    fn set_pll_f160m(&mut self, on: bool) {
        self.pll_f160m.modify_ref_160m_ctrl0(|w| {
            w.set_reg_ref_160m_clk_en(on);
        });
    }

    fn set_analog_i2c_master(&mut self, on: bool) {
        self.lpcon.set_i2c_ana_mst_master_clock(on);
    }

    fn set_coexistence(&mut self, on: bool) {
        self.lpcon.set_coex_function_clock(on);
    }

    fn set_low_power_timer(&mut self, on: bool) {
        self.lpcon.set_modem_pwr_lp_timer_clock(on);
    }

    fn open_wifi_power(&mut self) {
        self.lpcon.set_modem_pwr_wifi_power_clock(true);
    }
}

/// The reference counts of the platform clocks over their gate writes.
pub(crate) struct ClockCounts<G> {
    gates: G,
    /// References per clock; MPLL's count is the radio's, above the PSRAM
    /// mapping's permanent one.
    counts: PlatformClockHolds,
    _psram: PsramMpll,
}

impl<G: ClockGates> ClockCounts<G> {
    /// Adopt the clocks with no radio reference held, and open the Wi-Fi
    /// power gate, which nothing closes.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/rtc_cntl/rtc/esp32s31.rs:670`
    /// and `esp-hal/src/soc/esp32s31/regi2c.rs` (`enable_wifi_power_clock`:
    /// opened at clock initialization, never closed).
    pub(crate) fn adopt(mut gates: G, psram: PsramMpll) -> Self {
        gates.open_wifi_power();
        Self {
            gates,
            counts: PlatformClockHolds::default(),
            _psram: psram,
        }
    }

    /// Take one reference to `clock`: the first opens its gate.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2
    /// `components/esp_hw_support/port/esp32s31/esp_clk_tree.c:510-541`
    /// (`esp_clk_tree_enable_gated_clk`: the gate on the zero-to-one
    /// count), `components/esp_hw_support/include/esp_private/regi2c_ctrl.h:47-63`
    /// (`ANALOG_CLOCK_ENABLE`),
    /// `components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c:209-212`
    /// (the coexistence gate) and esp-hal 1f646c5
    /// `esp-hal/src/soc/esp32s31/regi2c.rs` (`ClkConfRefs`: one count per
    /// `CLK_CONF` gate).
    pub(crate) fn acquire(&mut self, clock: PlatformClock) -> Result<(), ChipClockError> {
        let count = self.counts.get(clock);
        let next = count
            .checked_add(1)
            .ok_or(ChipClockError::CountExhausted(clock))?;
        if count == 0 {
            self.edge(clock, true);
        }
        self.counts.set(clock, next);
        Ok(())
    }

    /// Give back one reference to `clock`: the last closes its gate.
    ///
    /// SOURCE: as [`Self::acquire`], the one-to-zero count.
    pub(crate) fn release(&mut self, clock: PlatformClock) -> Result<(), ChipClockError> {
        let next = self
            .counts
            .get(clock)
            .checked_sub(1)
            .ok_or(ChipClockError::NotHeld(clock))?;
        if next == 0 {
            self.edge(clock, false);
        }
        self.counts.set(clock, next);
        Ok(())
    }

    /// The references held to `clock`, MPLL's above the PSRAM mapping's.
    #[cfg(test)]
    pub(crate) fn count(&self, clock: PlatformClock) -> u16 {
        self.counts.get(clock)
    }

    fn edge(&mut self, clock: PlatformClock, on: bool) {
        match clock {
            PlatformClock::Pll160m => self.gates.set_pll_f160m(on),
            PlatformClock::AnalogI2cMaster => self.gates.set_analog_i2c_master(on),
            PlatformClock::ModemCoexistence => self.gates.set_coexistence(on),
            PlatformClock::ModemLowPowerTimer => self.gates.set_low_power_timer(on),
            // The PSRAM mapping's reference is under every radio one.
            PlatformClock::Mpll => {}
        }
    }
}

/// The installed clock words and their counts.
static CHIP_CLOCKS: ClaimCell<ClockCounts<ClockWords>> = ClaimCell::new();

/// The chip's platform clock owner: the provider and the backend of every
/// [`PlatformClockGuard`] of the radio.
///
/// Only the reset root makes it ([`Root::into_runtime`](crate::root::Root::into_runtime)),
/// once per image. A refused request is the caller's typed error, never a
/// wait: the words are lent to another context now (an operation it
/// interrupted, or another hart), a release has no reference to give back,
/// or a count is exhausted.
///
/// ```compile_fail,E0451
/// let _forged = oer_esp32s31_hal::power::ChipClocks { _private: () };
/// ```
#[derive(Debug)]
#[must_use = "the chip clock owner is the provider of every radio clock reference"]
pub struct ChipClocks {
    _private: (),
}

impl ChipClocks {
    /// Install the clock words with the PSRAM mapping's MPLL reference, once,
    /// from the reset root; this opens the Wi-Fi power gate.
    ///
    /// `parts` is the affine installation authority: only the PAC's one
    /// root can supply these fixed-placement owners, and this call consumes
    /// them without exposing a way to recover them.
    pub(crate) fn install(parts: ChipClockPeripherals, psram: PsramMpll) -> Self {
        // This is the cell's only installer. A second call needs a second
        // `ChipClockPeripherals`, which the PAC cannot issue; lending an
        // empty cell does not change its state. Installation therefore
        // succeeds, without a runtime refusal or a panic in this API.
        let _ = CHIP_CLOCKS.install(ClockCounts::adopt(ClockWords::new(parts), psram));
        Self { _private: () }
    }

    fn with(
        operation: impl FnOnce(&mut ClockCounts<ClockWords>) -> Result<(), ChipClockError>,
    ) -> Result<(), PlatformClockError> {
        match CHIP_CLOCKS.lend(operation) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(PlatformClockError),
        }
    }
}

impl PlatformClockBackend for ChipClocks {
    fn acquire(clock: PlatformClock) -> Result<(), PlatformClockError> {
        Self::with(|counts| counts.acquire(clock))
    }

    fn release(clock: PlatformClock) -> Result<(), PlatformClockError> {
        Self::with(|counts| counts.release(clock))
    }

    /// The reference stays counted, so its clock stays on until a chip
    /// reset: the guard's owner chose that, or dropped the guard, which its
    /// debug build asserts.
    fn abandon(_clock: PlatformClock, _how: PlatformClockAbandonment) {}
}

impl PlatformClockProvider for ChipClocks {
    type Backend = Self;

    fn acquire(
        &self,
        clock: PlatformClock,
    ) -> Result<PlatformClockGuard<Self>, PlatformClockError> {
        <Self as PlatformClockBackend>::acquire(clock)?;
        Ok(PlatformClockGuard::new(clock))
    }
}
