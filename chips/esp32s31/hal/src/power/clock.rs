//! ESP32-S31 table of the shared modem clock planner.
//!
//! The chip-neutral reference-count planner is [`oer_espressif_modem_clock`]; this
//! module supplies the ESP32-S31 vendor device order and module dependency
//! sets, and its executor performs each edge. The analog-I2C master clock is
//! not refcounted here, because the vendor keeps that refcount in the platform
//! analog-I2C owner, and the Wi-Fi clock dependencies are retained while
//! Wi-Fi is initialized.
//!
//! Source: reviewed evidence `ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK`, ESP-IDF
//! revision `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe` (Apache-2.0),
//! `components/esp_hw_support/modem/port/esp32s31/`
//! `modem_clock_impl.c` (`*_CLOCK_DEPS`, `modem_clock_get_module_deps`, the
//! per-device `modem_clock_*_configure` actions and the I2C-master refcount
//! exception in `modem_clock_device_context`), the device order of
//! `modem/include/modem/modem_clock_impl.h` and the S31 `soc_caps.h`
//! selections (`SOC_MODEM_CLOCK_SOC_PLL_SOURCE_CG_SUPPORTED`,
//! `SOC_PHY_CALIBRATION_CLOCK_IS_INDEPENDENT`,
//! `SOC_MODEM_CLOCK_WIFI_BB_80X1_AS_APB`). The vendor `DATADUMP` device has no
//! module dependency and is omitted. Applies to ESP32-S31 only.
//!
//! The monotonic modem ICG map that the vendor ORs in on every module enable
//! belongs to platform initialization. A concurrently split radio runs a
//! managed planner inside its arbiter: every client goes through the arbiter
//! and the upstream PLL and analog-I2C halves through the platform clock
//! provider.

#![forbid(unsafe_code)]

use oer_espressif_modem_clock::{DependencySet, ModemClockDependency};

pub(crate) use oer_espressif_modem_clock::ModemClockPlannerIdentity;

/// Exact low-bit-first dependency identities, in the vendor device order.
///
/// Discriminants are planner indices, not public register masks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Dependency {
    ModemAdcCommonFe,
    ModemPrivateFe,
    /// First acquires the upstream 160 MHz source, then opens the modem
    /// PLL-source gate; release closes the gate first.
    Pll160AndModemSource,
    Coexistence,
    /// Acquires the platform analog-I2C clock reference.
    AnalogI2cMaster,
    WifiApb,
    WifiBb44m,
    WifiMac,
    /// Enabling pulses the Wi-Fi baseband reset before the clock.
    WifiBb,
    WifiBb80x1,
    Etm,
    BtMac,
    BtPeripheral,
    BtApbAndSecurity,
    BtIeee802154CommonBaseband,
    Ieee802154ApbAndMac,
}

impl ModemClockDependency for Dependency {
    const LOW_BIT_FIRST: &'static [Self] = &[
        Self::ModemAdcCommonFe,
        Self::ModemPrivateFe,
        Self::Pll160AndModemSource,
        Self::Coexistence,
        Self::AnalogI2cMaster,
        Self::WifiApb,
        Self::WifiBb44m,
        Self::WifiMac,
        Self::WifiBb,
        Self::WifiBb80x1,
        Self::Etm,
        Self::BtMac,
        Self::BtPeripheral,
        Self::BtApbAndSecurity,
        Self::BtIeee802154CommonBaseband,
        Self::Ieee802154ApbAndMac,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// The vendor keeps this refcount in the platform analog-I2C owner.
    fn refcounted(self) -> bool {
        !matches!(self, Self::AnalogI2cMaster)
    }

    /// Hardware stays enabled while Wi-Fi is initialized.
    fn retained_while_wifi_initialized(self) -> bool {
        matches!(
            self,
            Self::WifiApb | Self::WifiBb44m | Self::WifiMac | Self::WifiBb | Self::WifiBb80x1
        )
    }
}

pub(crate) type ModemClockPlanner<'identity> =
    oer_espressif_modem_clock::ModemClockPlanner<'identity, Dependency>;
pub(crate) type ModemClockLease<'identity> =
    oer_espressif_modem_clock::ModemClockLease<'identity, Dependency>;
pub(crate) type PreparedModemClockAcquire<'identity> =
    oer_espressif_modem_clock::PreparedModemClockAcquire<'identity, Dependency>;
pub(crate) type PreparedModemClockRelease<'planner, 'lease> =
    oer_espressif_modem_clock::PreparedModemClockRelease<'planner, 'lease, Dependency>;
pub(crate) type PoisonedModemClockAcquire<'identity> =
    oer_espressif_modem_clock::PoisonedModemClockAcquire<'identity, Dependency>;
pub(crate) type PoisonedModemClockRelease<'planner, 'lease> =
    oer_espressif_modem_clock::PoisonedModemClockRelease<'planner, 'lease, Dependency>;

/// Radio modules that request modem clocks, with their vendor dependency sets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the complete vendor module list; not every module has a client yet"
)]
pub(crate) enum ModemClockModule {
    AnalogI2cMaster,
    Phy,
    ModemAdcCommonFe,
    Coexistence,
    Wifi,
    Bluetooth,
    PhyCalibration,
    Ieee802154,
    ModemEtm,
    BluetoothApb,
}

impl ModemClockModule {
    /// The module's exact vendor dependency set.
    pub(crate) fn dependencies(self) -> DependencySet<Dependency> {
        use Dependency::*;
        let set = DependencySet::of;
        match self {
            Self::AnalogI2cMaster => set(&[AnalogI2cMaster]),
            Self::Phy => set(&[
                ModemAdcCommonFe,
                ModemPrivateFe,
                Pll160AndModemSource,
                WifiBb80x1,
                AnalogI2cMaster,
            ]),
            Self::ModemAdcCommonFe => set(&[ModemAdcCommonFe, Pll160AndModemSource]),
            Self::Coexistence => set(&[Coexistence, Pll160AndModemSource]),
            Self::Wifi => set(&[
                WifiMac,
                WifiApb,
                WifiBb,
                WifiBb44m,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
            ]),
            Self::Bluetooth => set(&[
                BtMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
                BtPeripheral,
            ]),
            Self::PhyCalibration => set(&[
                WifiApb,
                WifiBb,
                WifiBb44m,
                WifiBb80x1,
                Pll160AndModemSource,
                BtIeee802154CommonBaseband,
                BtApbAndSecurity,
            ]),
            Self::Ieee802154 => set(&[
                Ieee802154ApbAndMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
            ]),
            Self::ModemEtm => set(&[Etm]),
            Self::BluetoothApb => set(&[BtApbAndSecurity, Etm, Pll160AndModemSource, BtMac]),
        }
    }
}

mod executor;
pub(crate) use executor::ModemPlatformClocks;
pub use executor::{
    PlatformClock, PlatformClockAbandonment, PlatformClockError, PlatformClockGuard,
    PlatformClockHolds, PlatformClockProvider,
};
pub(crate) use executor::{execute_acquire, execute_release};

#[cfg(test)]
mod tests;
