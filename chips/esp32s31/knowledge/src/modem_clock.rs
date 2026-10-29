//! S31 modem clock device order and module dependency sets.
//! SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, ESP-IDF
//! `4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`,
//! `components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`;
//! `modem/include/modem/modem_clock_impl.h` and S31 `soc_caps.h`.
//! DATADUMP has no module dependency. Analog I2C counts live with its platform owner.

/// Exact low-bit-first dependency identities, in the vendor device order.
///
/// Discriminants are planner indices, not public register masks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Dependency {
    /// Common front-end APB and ADC clocks.
    ModemAdcCommonFe,
    /// Private front-end clocks.
    ModemPrivateFe,
    /// First acquires the upstream 160 MHz source, then opens the modem
    /// PLL-source gate; release closes the gate first.
    Pll160AndModemSource,
    /// Coexistence.
    Coexistence,
    /// Acquires the platform analog-I2C clock reference.
    AnalogI2cMaster,
    /// Wi-Fi APB clock.
    WifiApb,
    /// Wi-Fi 44 MHz baseband clock.
    WifiBb44m,
    /// Wi-Fi MAC clock.
    WifiMac,
    /// Enabling pulses the Wi-Fi baseband reset before the clock.
    WifiBb,
    /// Shared Wi-Fi/Bluetooth 80x1 baseband clock.
    WifiBb80x1,
    /// Event Task Matrix clock.
    Etm,
    /// Bluetooth MAC clock.
    BtMac,
    /// Bluetooth security and timer clocks.
    BtPeripheral,
    /// Bluetooth and security APB clocks.
    BtApbAndSecurity,
    /// Shared Bluetooth/IEEE 802.15.4 baseband clock.
    BtIeee802154CommonBaseband,
    /// IEEE 802.15.4 APB and MAC clocks.
    Ieee802154ApbAndMac,
}

impl Dependency {
    /// Dependencies in vendor device order.
    pub const LOW_BIT_FIRST: [Self; 16] = [
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

    /// The vendor keeps this refcount in the platform analog-I2C owner.
    pub fn refcounted(self) -> bool {
        !matches!(self, Self::AnalogI2cMaster)
    }

    /// Hardware stays enabled while Wi-Fi is initialized.
    pub fn retained_while_wifi_initialized(self) -> bool {
        matches!(
            self,
            Self::WifiApb | Self::WifiBb44m | Self::WifiMac | Self::WifiBb | Self::WifiBb80x1
        )
    }
}

/// Radio modules that request modem clocks, with their vendor dependency sets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockModule {
    /// Analog I2C master module.
    AnalogI2cMaster,
    /// PHY module.
    Phy,
    /// Common front-end APB and ADC clocks.
    ModemAdcCommonFe,
    /// Coexistence.
    Coexistence,
    /// Wi-Fi module.
    Wifi,
    /// Bluetooth module.
    Bluetooth,
    /// PHY calibration module.
    PhyCalibration,
    /// IEEE 802.15.4 module.
    Ieee802154,
    /// Modem Event Task Matrix module.
    ModemEtm,
    /// Bluetooth APB module.
    BluetoothApb,
}

impl ModemClockModule {
    /// The module's exact vendor dependency set.
    pub const fn dependencies(self) -> &'static [Dependency] {
        use Dependency::*;
        match self {
            Self::AnalogI2cMaster => &[AnalogI2cMaster],
            Self::Phy => &[
                ModemAdcCommonFe,
                ModemPrivateFe,
                Pll160AndModemSource,
                WifiBb80x1,
                AnalogI2cMaster,
            ],
            Self::ModemAdcCommonFe => &[ModemAdcCommonFe, Pll160AndModemSource],
            Self::Coexistence => &[Coexistence, Pll160AndModemSource],
            Self::Wifi => &[
                WifiMac,
                WifiApb,
                WifiBb,
                WifiBb44m,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
            ],
            Self::Bluetooth => &[
                BtMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
                BtPeripheral,
            ],
            Self::PhyCalibration => &[
                WifiApb,
                WifiBb,
                WifiBb44m,
                WifiBb80x1,
                Pll160AndModemSource,
                BtIeee802154CommonBaseband,
                BtApbAndSecurity,
            ],
            Self::Ieee802154 => &[
                Ieee802154ApbAndMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
            ],
            Self::ModemEtm => &[Etm],
            Self::BluetoothApb => &[BtApbAndSecurity, Etm, Pll160AndModemSource, BtMac],
        }
    }
}
