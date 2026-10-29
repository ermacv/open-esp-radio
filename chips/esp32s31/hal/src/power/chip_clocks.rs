//! The chip's platform clock owner: [`ChipClocks`].
//!
//! The radio's platform clocks ([`PlatformClock`]) live in words other SoC
//! users share, so one owner writes each word and counts references for
//! every user; radio modules hold [`PlatformClockGuard`]s, never fields.
//! The family adoption driver installs the root's words once through its HAL
//! adapter ([`ChipClocks::install`]) in one
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

use oer_espressif_radio::clocks::{ClockCounts, ClockError, ClockGates, ClockReferences};

impl ClockReferences for PlatformClockHolds {
    type Clock = PlatformClock;
    fn get(&self, clock: PlatformClock) -> u16 {
        self.get(clock)
    }
    fn set(&mut self, clock: PlatformClock, count: u16) {
        self.set(clock, count);
    }
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
    type References = PlatformClockHolds;
    fn set_clock(&mut self, clock: PlatformClock, on: bool) {
        match clock {
            // SOURCE: ESP-IDF 4d80c1b2 `components/esp_hal_clock/esp32s31/include/hal/clk_gate_ll.h:321-327`.
            PlatformClock::Pll160m => self.pll_f160m.modify_ref_160m_ctrl0(|w| {
                w.set_reg_ref_160m_clk_en(on);
            }),
            PlatformClock::AnalogI2cMaster => self.lpcon.set_i2c_ana_mst_master_clock(on),
            PlatformClock::ModemCoexistence => self.lpcon.set_coex_function_clock(on),
            PlatformClock::ModemLowPowerTimer => self.lpcon.set_modem_pwr_lp_timer_clock(on),
            // The permanent PSRAM reference remains below every radio reference.
            PlatformClock::Mpll => {}
        }
    }
    fn open_wifi_power(&mut self) {
        self.lpcon.set_modem_pwr_wifi_power_clock(true);
    }
}

/// The installed clock words and their counts.
static CHIP_CLOCKS: ClaimCell<ClockCounts<ClockWords, PsramMpll>> = ClaimCell::new();

/// The chip's platform clock owner: the provider and the backend of every
/// [`PlatformClockGuard`] of the radio.
///
/// Only adoption of the reset root's unique clock parts makes it, once per image. A refused request is the caller's typed error, never a
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
        operation: impl FnOnce(
            &mut ClockCounts<ClockWords, PsramMpll>,
        ) -> Result<(), ClockError<PlatformClock>>,
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
