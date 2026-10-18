#![no_std]
#![cfg(feature = "esp32s31")]
#![forbid(unsafe_code)]

//! ESP-HAL Wi-Fi platform adapter for the open ESP32-S31 Wi-Fi driver.
//!
//! The open driver owns the recovered Wi-Fi bring-up on the shared radio.
//! This adapter supplies Wi-Fi's own MAC platform: the `WIFI` singleton, its
//! CPU interrupt binding and the MAC initializer's platform sources.

use esp_hal::{
    interrupt::Priority,
    peripherals::{Interrupt, WIFI},
    rng::Rng,
    system::Cpu,
};
use oer_espressif_interrupt_table_esp_hal::{self as interrupt_table, Route};
use oer_interrupt_table::Entry;

use oer_esp32s31_hal::coex::CoexPtiTable;
use oer_esp32s31_phy::PhyTxTargetPowerProfile;

use oer_esp32s31_ieee80211::mac_start::WifiMacPlatform;

use oer_esp32s31_ieee80211_mac::init::{
    MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacDelayEntropy, MacSlowClockCalibration,
    MacSlowClockCalibrationSource, MacTxPowerPair, MacTxPowerSource,
};

pub mod mac_interrupt_epoch;

/// Wi-Fi's own MAC platform on the shared radio.
///
/// It retains the virtual `WIFI` singleton, the routes of its two interrupt
/// sources in the image's interrupt table and the calibrated TX power profile
/// consumed by cold MAC initialization. The shared radio-platform singletons
/// belong to the `EspHalRadioPlatform` of the shared radio's owner.
pub struct EspHalWifiPlatform {
    _wifi: WIFI<'static>,
    mac: Route,
    power: Route,
    phy_tx_power: Option<PhyTxTargetPowerProfile>,
}

/// A Wi-Fi interrupt source enabled on another core than the image's
/// interrupt table routes it to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiInterruptError(pub interrupt_table::Error);

impl EspHalWifiPlatform {
    /// `mac` and `power` are the tokens of `MODEM_WIFI_MAC` and
    /// `MODEM_WIFI_PWR` in the image's interrupt table, whose entries name the
    /// Wi-Fi system's `mac_interrupt` and `power_interrupt`; another source's
    /// token does not compile.
    pub fn new<M, P>(wifi: WIFI<'static>, mac: M, power: P) -> Self
    where
        M: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
        P: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    {
        const {
            assert!(
                M::SOURCE as u16 == Interrupt::MODEM_WIFI_MAC as u16,
                "the MAC token is not MODEM_WIFI_MAC's"
            );
            assert!(
                P::SOURCE as u16 == Interrupt::MODEM_WIFI_PWR as u16,
                "the power token is not MODEM_WIFI_PWR's"
            );
        };
        Self {
            _wifi: wifi,
            mac: Route::new(mac),
            power: Route::new(power),
            phy_tx_power: None,
        }
    }

    /// Transfer the calibrated Rust-owned PHY target-power snapshot into the
    /// platform capability consumed by cold MAC initialization.
    pub fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.phy_tx_power = Some(profile);
    }

    /// Route both Wi-Fi interrupt sources to their table level on their
    /// table core.
    ///
    /// # Errors
    ///
    /// On another core than the table's; nothing is routed then.
    pub fn enable_interrupts(&self) -> Result<(), WifiInterruptError> {
        interrupt_table::enable_route(&self.mac).map_err(WifiInterruptError)?;
        if let Err(error) = interrupt_table::enable_route(&self.power) {
            interrupt_table::disable_route(&self.mac);
            return Err(WifiInterruptError(error));
        }
        Ok(())
    }

    /// Silence both Wi-Fi interrupt sources on their table core.
    ///
    /// This closes only the platform routing edge. The caller may retain both
    /// ISR capabilities unchanged for same-epoch resume, or mask/acknowledge
    /// their banks for terminal teardown. It does not establish MAC/DMA or RF
    /// quiescence.
    pub fn disable_interrupts(&self) {
        interrupt_table::disable_route(&self.mac);
        interrupt_table::disable_route(&self.power);
    }
}

impl WifiMacPlatform for EspHalWifiPlatform {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        EspHalWifiPlatform::install_phy_tx_power_profile(self, profile);
    }
}

impl MacDelayEntropy for EspHalWifiPlatform {
    fn mac_delay_random(&mut self) -> u32 {
        // SOURCE: complete libpp hal_he_set_mac_delay on-chip branch obtains
        // `_random()` from g_wifi_osi_funcs. The esp-hal adapter implements
        // that callback with this same safe RNG facade.
        Rng::new().random()
    }
}

impl MacSlowClockCalibrationSource for EspHalWifiPlatform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        // SOURCE: the S31 esp-hal radio adapter installs slowclk_cal_get in
        // its OSI table and currently returns an unimplemented zero placeholder.
        // Keep that absence visible here; a future real calibration belongs behind
        // this platform trait and must return `Calibrated` with provenance.
        MacSlowClockCalibration::Unavailable
    }
}

impl MacTxPowerSource for EspHalWifiPlatform {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let Some(profile) = &self.phy_tx_power else {
            // Cold MAC init is ordered after the open PHY profile transfer.
            // Keep accidental misuse fail-closed without consulting vendor
            // global state or panicking in the hardware bring-up path.
            return MacTxPowerPair::ZERO;
        };
        let pair = profile.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

impl MacCoexPtiSource for EspHalWifiPlatform {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        // These cold values configure the MAC's own scheduler even though this
        // integration starts no Bluetooth/802.15.4 coexistence runtime. In
        // particular, complete `hal_init` publishes event three as RX_ACK PTI
        // seven. The former deterministic-zero substitution let pending BE
        // TX (PTI one) outrank an immediate response: RX-only HIL passed, but
        // concurrent TX produced thousands of WDEVRX_ABORT_FCS_PASS events.
        //
        // The values are the arbiter's cold table (`coex_pti_tab`).
        MacCoexPti::from_osi_value(CoexPtiTable::VENDOR.pti(event.coex_event()).value())
    }
}
