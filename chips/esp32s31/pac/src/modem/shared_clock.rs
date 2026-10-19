//! Route-owned MODEM_LPCON shared-clock capability.
//!
//! The register block travels with the mutually exclusive Wi-Fi, Bluetooth
//! and IEEE 802.15.4 route. Public callers receive observations and semantic
//! route operations, never a register handle or a droppable clock token.

#![forbid(unsafe_code)]

use crate::{RadioPhyRegisters, generated::ModemLowPowerClockDivider};

/// Semantic route-owned observation used by protocol clock checkpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedModemClockObservation {
    pub power_state_map_configured: bool,
}

/// Reviewed selector decoded inside the PAC from `COEX_LP_CLK_CONF`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexistenceLowPowerClockSource {
    Selector1,
    Selector2,
    Selector4,
    Selector8,
}

/// Semantic result of the vendor-required two-read coexistence sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexistenceLowPowerClockObservation {
    pub source: CoexistenceLowPowerClockSource,
    pub divider_minus_one: u16,
}

/// Semantic Bluetooth low-power-clock observation without register authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLowPowerClockObservation {
    pub exclusive_main_xtal_selected: bool,
    pub bluetooth_divider_configured: bool,
}

/// RTC slow-clock source decoded from `LP_AON_CLKRST.ROOT_CLK_CONF`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcSlowClockSource {
    SlowOscillator,
    Crystal32Khz,
    /// A selector value the vendor reports as invalid.
    Invalid(u8),
}

/// Wi-Fi power-domain low-power clock source selected at system start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiLowPowerClockSource {
    SlowOscillator,
    Crystal32Khz,
}

/// Semantic Wi-Fi low-power-clock observation without register authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiLowPowerClockObservation {
    /// The single selected source, or `None` unless exactly one of the two
    /// sources production selects is set.
    pub source: Option<WifiLowPowerClockSource>,
    pub divider: u16,
}

impl RadioPhyRegisters {
    /// The RTC slow-clock source, as `clk_ll_rtc_slow_get_src` reads it.
    #[doc(hidden)]
    pub fn rtc_slow_clock_source(&self) -> RtcSlowClockSource {
        match crate::svd::field_read::read_rtc_slow_clock_source(&self.peripherals.lp_aon_clkrst) {
            0 => RtcSlowClockSource::SlowOscillator,
            1 => RtcSlowClockSource::Crystal32Khz,
            other => RtcSlowClockSource::Invalid(other),
        }
    }

    /// Select the Wi-Fi power-domain low-power clock with divider zero.
    ///
    /// This is the source and divider half of
    /// `modem_clock_select_lp_clock_source(PERIPH_WIFI_MODULE, src, 0)` as run
    /// by `esp_perip_clk_init`: every source is deselected in the
    /// slow-oscillator, fast-oscillator, 32-kHz crystal, main-crystal order,
    /// the one source is selected (the 32-kHz crystal also selects the
    /// crystal as the modem 32-kHz source) and the divider is written, each
    /// as its own field write. The Wi-Fi power clock gate shares its word
    /// with ESP-HAL's reference-counted gates; `esp_hal::init` sets it and
    /// nothing clears it.
    #[doc(hidden)]
    pub fn select_wifi_low_power_clock(&mut self, source: WifiLowPowerClockSource) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::deselect_wifi_low_power_clock_slow_oscillator(registers);
        crate::generated::deselect_wifi_low_power_clock_fast_oscillator(registers);
        crate::generated::deselect_wifi_low_power_clock_crystal_32khz(registers);
        crate::generated::deselect_wifi_low_power_clock_crystal(registers);
        match source {
            WifiLowPowerClockSource::SlowOscillator => {
                crate::generated::select_wifi_low_power_clock_slow_oscillator(registers);
            }
            WifiLowPowerClockSource::Crystal32Khz => {
                crate::generated::select_wifi_low_power_clock_crystal_32khz(registers);
                crate::generated::select_modem_32khz_clock_crystal(registers);
            }
        }
        crate::generated::set_wifi_low_power_clock_divider(
            registers,
            ModemLowPowerClockDivider::new(0).expect("zero is a valid low-power divider"),
        );
    }

    #[doc(hidden)]
    pub fn wifi_low_power_clock_observation(&self) -> WifiLowPowerClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let (slow, fast, crystal, crystal_32khz, divider) =
            crate::svd::field_snapshot_read::observe_wifi_low_power_clock_configuration(registers);
        WifiLowPowerClockObservation {
            source: match (slow, fast, crystal, crystal_32khz) {
                (true, false, false, false) => Some(WifiLowPowerClockSource::SlowOscillator),
                (false, false, false, true) => Some(WifiLowPowerClockSource::Crystal32Khz),
                _ => None,
            },
            divider,
        }
    }

    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        // This is the vendor's monotonic global ICG-map initialization, not
        // lease-owned state. It preserves the existing image and is therefore
        // intentionally not rolled back when an individual route is released.
        crate::generated::initialize_shared_modem_power_state_map(
            &self.peripherals.modem_lpcon_shared_clock,
        );
    }

    #[doc(hidden)]
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let (
            wifi_power_map_bit_one,
            wifi_power_map_bit_two,
            coexistence_map_bit_one,
            coexistence_map_bit_two,
            phy_i2c_map_bit_one,
            phy_i2c_map_bit_two,
            low_power_apb_map_bit_one,
            low_power_apb_map_bit_two,
        ) = crate::svd::field_snapshot_read::observe_shared_modem_power_state_map(registers);
        SharedModemClockObservation {
            power_state_map_configured: wifi_power_map_bit_one
                && wifi_power_map_bit_two
                && coexistence_map_bit_one
                && coexistence_map_bit_two
                && phy_i2c_map_bit_one
                && phy_i2c_map_bit_two
                && low_power_apb_map_bit_one
                && low_power_apb_map_bit_two,
        }
    }

    /// Select the coexistence timer clock and its divider, as complete
    /// `coex_hw_timer_freq_set` does: the one-hot source first, then the
    /// divider, each through its own fresh-read RMW.
    #[doc(hidden)]
    pub fn configure_coexistence_timer_clock(
        &mut self,
        source: crate::CoexTimerClockSource,
        divider_minus_one: crate::CoexTimerClockDividerMinusOne,
    ) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::select_coex_timer_clock_source(registers, source);
        crate::generated::set_coex_timer_clock_divider(registers, divider_minus_one);
    }

    #[doc(hidden)]
    pub fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let source =
            match crate::svd::field_snapshot_read::observe_coexistence_low_power_clock_source(
                registers,
            ) {
                (true, false, false, false) => CoexistenceLowPowerClockSource::Selector1,
                (false, true, false, false) => CoexistenceLowPowerClockSource::Selector2,
                (false, false, true, false) => CoexistenceLowPowerClockSource::Selector4,
                (false, false, false, true) => CoexistenceLowPowerClockSource::Selector8,
                _ => return None,
            };
        Some(CoexistenceLowPowerClockObservation {
            source,
            divider_minus_one: crate::svd::field_read::observe_coexistence_low_power_clock_divider(
                registers,
            ),
        })
    }

    /// Deselect every Bluetooth low-power timer source.
    ///
    /// This is `modem_clock_hal_deselect_all_ble_rtc_timer_lpclk_source`: one
    /// write per selector, in the vendor's slow-oscillator, fast-oscillator,
    /// 32-kHz crystal, main-crystal order. The divider is untouched.
    #[doc(hidden)]
    pub fn deselect_bluetooth_low_power_timer_sources(&mut self) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::deselect_bluetooth_low_power_timer_slow_oscillator(registers);
        crate::generated::deselect_bluetooth_low_power_timer_fast_oscillator(registers);
        crate::generated::deselect_bluetooth_low_power_timer_crystal_32khz(registers);
        crate::generated::deselect_bluetooth_low_power_timer_crystal(registers);
    }

    /// Select the main crystal as the only Bluetooth low-power timer source
    /// and program its divider.
    ///
    /// This is the source and divider half of
    /// `modem_clock_select_lp_clock_source(PERIPH_BT_MODULE, MAIN_XTAL, divider)`:
    /// every selector is cleared, the main crystal is selected and the
    /// divider written, each as its own field write.
    #[doc(hidden)]
    pub fn select_bluetooth_low_power_timer_main_crystal(
        &mut self,
        divider: ModemLowPowerClockDivider,
    ) {
        self.deselect_bluetooth_low_power_timer_sources();
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::select_bluetooth_low_power_timer_crystal(registers);
        crate::generated::set_bluetooth_low_power_timer_divider(registers, divider);
    }

    #[doc(hidden)]
    pub fn bluetooth_low_power_clock_observation(&self) -> BluetoothLowPowerClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let (
            slow_oscillator_selected,
            fast_oscillator_selected,
            crystal_selected,
            crystal_32khz_selected,
            divider_minus_one,
        ) = crate::svd::field_snapshot_read::observe_bluetooth_low_power_timer_configuration(
            registers,
        );
        BluetoothLowPowerClockObservation {
            exclusive_main_xtal_selected: !slow_oscillator_selected
                && !fast_oscillator_selected
                && crystal_selected
                && !crystal_32khz_selected,
            bluetooth_divider_configured: u32::from(divider_minus_one)
                == crate::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER.get(),
        }
    }
}
