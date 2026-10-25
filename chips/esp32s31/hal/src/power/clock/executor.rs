//! Target executor for the shared modem clock planner.
//!
//! The planner decides which dependency edges a module request needs; this
//! executor performs each edge and only then records it complete. Modem gates
//! go to the radio PAC. The upstream 160 MHz source and the analog-I2C master
//! clock belong to the platform clock owner and are reached through
//! [`PlatformClockProvider`], which counts its own references.
//!
//! The coexistence gate shares its register word with the analog-I2C master,
//! so it is a platform-owned half too.
//!
//! For the modem PLL-source dependency the executor acquires the 160 MHz
//! source before opening the modem gate and releases it after closing the
//! gate, in the vendor order. Each platform reference lives in
//! [`ModemPlatformClocks`] while its dependency is enabled. A refused request
//! poisons the transaction: the planner cannot tell a refused request from a
//! partially applied one.

use core::marker::PhantomData;

use crate::{modem::ModemClockDevice, phy::registers::RadioPhyRegisters};

use super::{
    Dependency, ModemClockLease, ModemClockPlanner, PoisonedModemClockAcquire,
    PoisonedModemClockRelease, PreparedModemClockAcquire, PreparedModemClockRelease,
};

/// A platform clock request was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformClockError;

/// A clock whose gate the platform clock owner writes and counts.
///
/// Each lives in a register word that the platform also writes for other
/// SoC users, so the radio requests it instead of writing the gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformClock {
    /// The 160 MHz PLL output that feeds the modem PLL source.
    Pll160m,
    /// The analog-I2C master clock gate in `MODEM_LPCON.CLK_CONF`.
    AnalogI2cMaster,
    /// The MPLL, whose PMU power bits share a word with the front-end
    /// baseband power the PHY writes.
    Mpll,
    /// The modem coexistence clock gate in `MODEM_LPCON.CLK_CONF`.
    ModemCoexistence,
    /// The modem low-power timer clock gate in `MODEM_LPCON.CLK_CONF`.
    ModemLowPowerTimer,
}

/// One platform's reference counts of the radio's platform clocks.
///
/// Its functions take no state: the platform's counts are global, which is
/// what lets a [`PlatformClockGuard`] give a reference back without a
/// borrow of its provider.
pub trait PlatformClockBackend: 'static {
    /// Take one reference to `clock`.
    fn acquire(clock: PlatformClock);
    /// Give back one reference to `clock` that [`Self::acquire`] took.
    fn release(clock: PlatformClock);
    /// Record that a reference to `clock` stays held until a chip reset.
    fn abandon(clock: PlatformClock, how: PlatformClockAbandonment);
}

/// Platform-owned clocks that radio modules depend on.
///
/// The platform clock owner (ESP-HAL) keeps its own reference count of each,
/// because other SoC users share them. The radio holds each reference as a
/// [`PlatformClockGuard`] for as long as it needs the clock and gives it back
/// with [`PlatformClockGuard::release`].
pub trait PlatformClockProvider {
    /// The backend every guard from this provider calls.
    type Backend: PlatformClockBackend;

    /// Acquire one reference to `clock`.
    ///
    /// # Errors
    ///
    /// The platform refused the reference; nothing is held.
    fn acquire(
        &self,
        clock: PlatformClock,
    ) -> Result<PlatformClockGuard<Self::Backend>, PlatformClockError>;
}

/// How a platform clock reference ended without going back to the
/// platform: its clock stays on until a chip reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformClockAbandonment {
    /// An owner kept the clock on deliberately ([`PlatformClockGuard::abandon`]),
    /// such as a power or clock transaction that failed and stays fail-stop.
    Abandoned,
    /// The guard was dropped without [`PlatformClockGuard::release`] or
    /// [`PlatformClockGuard::abandon`]: a defect in its owner.
    Dropped,
}

/// One platform clock reference held by the radio.
///
/// A guard ends in one of two explicit ways: [`Self::release`] gives the
/// reference back through the provider that created it, in its owner's
/// checked release path, and [`Self::abandon`] keeps the clock on
/// deliberately, as a fail-stop owner does. Dropping a guard otherwise is a
/// defect: the clock stays on, which fails safe, the provider records the
/// drop, and a debug build asserts. Destroying the radio's bookkeeping
/// therefore never closes a clock under a user that still runs on it, and
/// never hides that it kept one on. The platform's counts are global, so a
/// guard needs no borrow of its provider and may live in a static radio
/// owner.
#[must_use = "a guard ends with `release` or `abandon`; dropping it keeps the clock on and is a defect"]
pub struct PlatformClockGuard<B: PlatformClockBackend> {
    clock: PlatformClock,
    ended: bool,
    backend: PhantomData<fn() -> B>,
}

impl<B: PlatformClockBackend> PlatformClockGuard<B> {
    /// Wrap one reference to `clock` that the caller just acquired from its
    /// platform: `B::release(clock)` gives back exactly that reference, and
    /// `B::abandon(clock, how)` records that it stays held.
    ///
    /// Only a [`PlatformClockProvider`] implementation creates guards.
    pub const fn new(clock: PlatformClock) -> Self {
        Self {
            clock,
            ended: false,
            backend: PhantomData,
        }
    }

    /// The clock this guard holds.
    pub const fn clock(&self) -> PlatformClock {
        self.clock
    }

    /// Give the reference back to the platform.
    pub fn release(mut self) {
        self.ended = true;
        B::release(self.clock);
    }

    /// Keep the clock on until a chip reset, deliberately.
    pub fn abandon(mut self) {
        self.ended = true;
        B::abandon(self.clock, PlatformClockAbandonment::Abandoned);
    }
}

impl<B: PlatformClockBackend> Drop for PlatformClockGuard<B> {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        B::abandon(self.clock, PlatformClockAbandonment::Dropped);
        #[cfg(test)]
        if std::thread::panicking() {
            return;
        }
        debug_assert!(
            false,
            "a {:?} guard was dropped without release or abandon",
            self.clock
        );
    }
}

impl<B: PlatformClockBackend> core::fmt::Debug for PlatformClockGuard<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("PlatformClockGuard")
            .field(&self.clock)
            .finish()
    }
}

/// How many references the radio's guards hold, per platform clock.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlatformClockHolds {
    counts: [u16; PlatformClock::COUNT],
}

impl PlatformClockHolds {
    /// The references held to `clock`.
    pub const fn get(&self, clock: PlatformClock) -> u16 {
        self.counts[clock.index()]
    }

    /// Record `count` references to `clock`.
    pub const fn set(&mut self, clock: PlatformClock, count: u16) {
        self.counts[clock.index()] = count;
    }

    /// Whether no reference is held.
    pub fn is_empty(&self) -> bool {
        self.counts.iter().all(|&count| count == 0)
    }

    /// Count the reference `guard` holds.
    pub(crate) fn count<B: PlatformClockBackend>(&mut self, guard: Option<&PlatformClockGuard<B>>) {
        if let Some(guard) = guard {
            let index = guard.clock().index();
            self.counts[index] = self.counts[index].saturating_add(1);
        }
    }
}

impl PlatformClock {
    /// Every platform clock, in index order.
    pub const ALL: [Self; 5] = [
        Self::Pll160m,
        Self::AnalogI2cMaster,
        Self::Mpll,
        Self::ModemCoexistence,
        Self::ModemLowPowerTimer,
    ];

    /// The number of platform clocks.
    pub const COUNT: usize = Self::ALL.len();

    /// This clock's position in [`Self::ALL`].
    pub const fn index(self) -> usize {
        match self {
            Self::Pll160m => 0,
            Self::AnalogI2cMaster => 1,
            Self::Mpll => 2,
            Self::ModemCoexistence => 3,
            Self::ModemLowPowerTimer => 4,
        }
    }
}

/// Platform clock references the modem clock planner's dependency edges
/// hold, one per platform-owned dependency.
#[derive(Debug)]
pub(crate) struct ModemPlatformClocks<B: PlatformClockBackend> {
    pll_f160m: Option<PlatformClockGuard<B>>,
    analog_i2c: Option<PlatformClockGuard<B>>,
    coexistence: Option<PlatformClockGuard<B>>,
}

impl<B: PlatformClockBackend> Default for ModemPlatformClocks<B> {
    fn default() -> Self {
        Self {
            pll_f160m: None,
            analog_i2c: None,
            coexistence: None,
        }
    }
}

impl<B: PlatformClockBackend> ModemPlatformClocks<B> {
    fn slot(&mut self, clock: PlatformClock) -> &mut Option<PlatformClockGuard<B>> {
        match clock {
            PlatformClock::Pll160m => &mut self.pll_f160m,
            PlatformClock::AnalogI2cMaster => &mut self.analog_i2c,
            PlatformClock::ModemCoexistence => &mut self.coexistence,
            PlatformClock::Mpll | PlatformClock::ModemLowPowerTimer => {
                unreachable!("no modem clock dependency holds this platform clock")
            }
        }
    }

    fn acquire(
        &mut self,
        clock: PlatformClock,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), PlatformClockError> {
        let slot = self.slot(clock);
        debug_assert!(slot.is_none(), "the planner enables a dependency once");
        if slot.is_none() {
            *slot = Some(platform.acquire(clock)?);
        }
        Ok(())
    }

    fn release(&mut self, clock: PlatformClock) {
        if let Some(guard) = self.slot(clock).take() {
            guard.release();
        }
    }

    /// Keep every reference still held on until a chip reset, and forget
    /// it.
    #[cfg(test)]
    pub(crate) fn abandon(self) {
        for guard in [self.pll_f160m, self.analog_i2c, self.coexistence]
            .into_iter()
            .flatten()
        {
            guard.abandon();
        }
    }

    /// Count the references held into `holds`.
    pub(crate) fn count_into(&self, holds: &mut PlatformClockHolds) {
        for guard in [&self.pll_f160m, &self.analog_i2c, &self.coexistence] {
            holds.count(guard.as_ref());
        }
    }
}

/// Modem clock gates driven by the radio PAC.
pub(crate) trait ModemClockPort {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool);
}

impl ModemClockPort for RadioPhyRegisters {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.configure_modem_clock_device(device, enable);
    }
}

const fn modem_device(dependency: Dependency) -> Option<ModemClockDevice> {
    Some(match dependency {
        Dependency::ModemAdcCommonFe => ModemClockDevice::ModemAdcCommonFe,
        Dependency::ModemPrivateFe => ModemClockDevice::ModemPrivateFe,
        Dependency::WifiApb => ModemClockDevice::WifiApb,
        Dependency::WifiBb44m => ModemClockDevice::WifiBaseband44m,
        Dependency::WifiMac => ModemClockDevice::WifiMac,
        Dependency::WifiBb => ModemClockDevice::WifiBaseband,
        Dependency::WifiBb80x1 => ModemClockDevice::WifiBaseband80x1,
        Dependency::Etm => ModemClockDevice::Etm,
        Dependency::BtMac => ModemClockDevice::BluetoothMac,
        Dependency::BtPeripheral => ModemClockDevice::BluetoothPeripheral,
        Dependency::BtApbAndSecurity => ModemClockDevice::BluetoothApb,
        Dependency::BtIeee802154CommonBaseband => {
            ModemClockDevice::BluetoothIeee802154CommonBaseband
        }
        Dependency::Ieee802154ApbAndMac => ModemClockDevice::Ieee802154Mac,
        // Platform-owned halves are handled by the caller.
        Dependency::Pll160AndModemSource
        | Dependency::AnalogI2cMaster
        | Dependency::Coexistence => return None,
    })
}

fn perform_acquire<B: PlatformClockBackend>(
    dependency: Dependency,
    port: &mut impl ModemClockPort,
    held: &mut ModemPlatformClocks<B>,
    platform: &impl PlatformClockProvider<Backend = B>,
) -> Result<(), PlatformClockError> {
    match dependency {
        Dependency::Pll160AndModemSource => {
            held.acquire(PlatformClock::Pll160m, platform)?;
            port.configure_device(ModemClockDevice::PllSourceGate, true);
        }
        Dependency::AnalogI2cMaster => held.acquire(PlatformClock::AnalogI2cMaster, platform)?,
        Dependency::Coexistence => held.acquire(PlatformClock::ModemCoexistence, platform)?,
        other => {
            if let Some(device) = modem_device(other) {
                port.configure_device(device, true);
            }
        }
    }
    Ok(())
}

fn perform_release<B: PlatformClockBackend>(
    dependency: Dependency,
    port: &mut impl ModemClockPort,
    held: &mut ModemPlatformClocks<B>,
) -> Result<(), PlatformClockError> {
    match dependency {
        Dependency::Pll160AndModemSource => {
            port.configure_device(ModemClockDevice::PllSourceGate, false);
            held.release(PlatformClock::Pll160m);
        }
        Dependency::AnalogI2cMaster => held.release(PlatformClock::AnalogI2cMaster),
        Dependency::Coexistence => held.release(PlatformClock::ModemCoexistence),
        other => {
            if let Some(device) = modem_device(other) {
                port.configure_device(device, false);
            }
        }
    }
    Ok(())
}

/// Perform and commit one prepared acquisition.
///
/// # Errors
///
/// A refused platform request poisons the transaction at that edge.
#[allow(
    clippy::result_large_err,
    reason = "the poisoned transaction retains the planner without allocation"
)]
pub(crate) fn execute_acquire<B: PlatformClockBackend>(
    prepared: PreparedModemClockAcquire,
    port: &mut impl ModemClockPort,
    held: &mut ModemPlatformClocks<B>,
    platform: &impl PlatformClockProvider<Backend = B>,
) -> Result<(ModemClockPlanner, ModemClockLease), PoisonedModemClockAcquire> {
    oer_espressif_modem_clock::execute_acquire(prepared, |dependency| {
        perform_acquire(dependency, port, held, platform)
    })
}

/// Perform and commit one prepared release.
///
/// # Errors
///
/// Releasing never fails; the planner's release contract still reports a
/// poisoned transaction.
#[allow(
    clippy::result_large_err,
    reason = "the poisoned transaction retains the planner and lease without allocation"
)]
pub(crate) fn execute_release<B: PlatformClockBackend>(
    prepared: PreparedModemClockRelease,
    port: &mut impl ModemClockPort,
    held: &mut ModemPlatformClocks<B>,
) -> Result<ModemClockPlanner, PoisonedModemClockRelease> {
    oer_espressif_modem_clock::execute_release(prepared, |dependency| {
        perform_release(dependency, port, held)
    })
}

#[cfg(test)]
mod tests;
