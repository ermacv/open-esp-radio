//! Route-owned MODEM_LPCON shared-clock capability.
//!
//! The register block travels with the Wi-Fi route. Public callers receive observations and semantic
//! route operations, never a register handle or a droppable clock token.

#![forbid(unsafe_code)]

use oer_chips_esp32s31_pac::dictionaries::IcgState;
use oer_chips_family_radio_power::clocks::{RtcSlowClockSource, WifiLowPowerClockSource};

use crate::{radio::modem::values::ModemLowPowerClockDivider, radio::phy::RadioPhyRegisters};

/// Semantic route-owned observation used by protocol clock checkpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedModemClockObservation {
    pub power_state_map_configured: bool,
}

impl<P: crate::radio::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
    /// The RTC slow-clock source, as `clk_ll_rtc_slow_get_src` reads it.
    #[doc(hidden)]
    pub fn rtc_slow_clock_source(&self) -> RtcSlowClockSource {
        match super::field_byte(
            self.peripherals
                .lp_aon_radio_clock
                .read_root_clk_conf()
                .slow_clk_sel(),
        ) {
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
        let registers = &mut self.peripherals.modem_lpcon_power;
        super::ops::deselect_wifi_low_power_clock_slow_oscillator(registers);
        super::ops::deselect_wifi_low_power_clock_fast_oscillator(registers);
        super::ops::deselect_wifi_low_power_clock_crystal_32khz(registers);
        super::ops::deselect_wifi_low_power_clock_crystal(registers);
        match source {
            WifiLowPowerClockSource::SlowOscillator => {
                super::ops::select_wifi_low_power_clock_slow_oscillator(registers);
            }
            WifiLowPowerClockSource::Crystal32Khz => {
                super::ops::select_wifi_low_power_clock_crystal_32khz(registers);
                super::ops::select_modem_32khz_clock_crystal(registers);
            }
        }
        super::ops::set_wifi_low_power_clock_divider(
            registers,
            const { ModemLowPowerClockDivider::new(0).expect("zero is a valid low-power divider") },
        );
    }

    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        // This is the vendor's monotonic global ICG-map initialization, not
        // lease-owned state. It preserves the existing image and is therefore
        // intentionally not rolled back when an individual route is released.
        super::ops::initialize_shared_modem_power_state_map(
            &mut self.peripherals.modem_lpcon_power,
        );
    }

    #[doc(hidden)]
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        let clk_conf_power_st = self.peripherals.modem_lpcon_power.read_clk_conf_power_st();
        let (
            wifi_power_map_bit_one,
            wifi_power_map_bit_two,
            coexistence_map_bit_one,
            coexistence_map_bit_two,
            phy_i2c_map_bit_one,
            phy_i2c_map_bit_two,
            low_power_apb_map_bit_one,
            low_power_apb_map_bit_two,
        ) = (
            clk_conf_power_st
                .clk_wifipwr_st_map()
                .contains(IcgState::NogatingModem),
            clk_conf_power_st
                .clk_wifipwr_st_map()
                .contains(IcgState::NogatingActive),
            clk_conf_power_st
                .clk_coex_st_map()
                .contains(IcgState::NogatingModem),
            clk_conf_power_st
                .clk_coex_st_map()
                .contains(IcgState::NogatingActive),
            clk_conf_power_st
                .clk_i2c_mst_st_map()
                .contains(IcgState::NogatingModem),
            clk_conf_power_st
                .clk_i2c_mst_st_map()
                .contains(IcgState::NogatingActive),
            clk_conf_power_st
                .clk_lp_apb_st_map()
                .contains(IcgState::NogatingModem),
            clk_conf_power_st
                .clk_lp_apb_st_map()
                .contains(IcgState::NogatingActive),
        );
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
}

impl<P: crate::radio::phy::registers::RadioPlacement>
    oer_chips_family_radio_power::clocks::LowPowerClockPort for RadioPhyRegisters<P>
{
    fn rtc_slow_clock_source(&self) -> RtcSlowClockSource {
        RadioPhyRegisters::rtc_slow_clock_source(self)
    }
    fn select_wifi_low_power_clock(&mut self, source: WifiLowPowerClockSource) {
        RadioPhyRegisters::select_wifi_low_power_clock(self, source);
    }
}
