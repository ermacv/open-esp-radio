//! Wi-Fi's low-power clock selection, decoded from the RTC slow clock's source.

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

/// Source observation and fixed selector-write atom.
pub trait LowPowerClockPort {
    fn rtc_slow_clock_source(&self) -> RtcSlowClockSource;
    fn select_wifi_low_power_clock(&mut self, source: WifiLowPowerClockSource);
}

/// Select XTAL32K only for that RTC source; all other selectors use RC_SLOW.
/// SOURCE: ESP-IDF 4d59230d `esp_perip_clk_init`.
pub fn select_wifi_low_power_clock(port: &mut impl LowPowerClockPort) {
    let source = match port.rtc_slow_clock_source() {
        RtcSlowClockSource::Crystal32Khz => WifiLowPowerClockSource::Crystal32Khz,
        RtcSlowClockSource::SlowOscillator | RtcSlowClockSource::Invalid(_) => {
            WifiLowPowerClockSource::SlowOscillator
        }
    };
    port.select_wifi_low_power_clock(source);
}

#[cfg(test)]
mod tests;
