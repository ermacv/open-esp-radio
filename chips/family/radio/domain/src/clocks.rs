//! Reference-counted platform clock edges, retaining the permanent memory owner.

/// Software reference storage for the chip's finite clock vocabulary.
pub trait ClockReferences: Default {
    type Clock: Copy;
    fn get(&self, clock: Self::Clock) -> u16;
    fn set(&mut self, clock: Self::Clock, count: u16);
}

/// Gate atoms. A clock backed by a permanent memory reference has a no-op edge.
pub trait ClockGates {
    type References: ClockReferences;
    fn set_clock(&mut self, clock: <Self::References as ClockReferences>::Clock, on: bool);
    fn open_wifi_power(&mut self);
}

/// A refused count transition; no gate write has been issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockError<C> {
    NotHeld(C),
    CountExhausted(C),
}

/// Counts above the permanent memory reference. Only the chip owner installs it.
pub struct ClockCounts<G: ClockGates, P> {
    gates: G,
    counts: G::References,
    _permanent: P,
}

impl<G: ClockGates, P> ClockCounts<G, P> {
    /// Adopt existing clocks and open the Wi-Fi power gate once.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/rtc_cntl/rtc/esp32s31.rs:670`
    /// and `esp-hal/src/soc/esp32s31/regi2c.rs` (`enable_wifi_power_clock`).
    pub fn adopt(mut gates: G, permanent: P) -> Self {
        gates.open_wifi_power();
        Self {
            gates,
            counts: G::References::default(),
            _permanent: permanent,
        }
    }

    /// First reference opens the gate, then commits the count.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `components/esp_hw_support/port/esp32s31/esp_clk_tree.c:510-541`
    /// (`esp_clk_tree_enable_gated_clk`), `components/esp_hw_support/include/esp_private/regi2c_ctrl.h:47-63`
    /// (`ANALOG_CLOCK_ENABLE`) and `components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c:209-212`.
    pub fn acquire(
        &mut self,
        clock: <G::References as ClockReferences>::Clock,
    ) -> Result<(), ClockError<<G::References as ClockReferences>::Clock>> {
        let count = self.counts.get(clock);
        let next = count
            .checked_add(1)
            .ok_or(ClockError::CountExhausted(clock))?;
        if count == 0 {
            self.gates.set_clock(clock, true);
        }
        self.counts.set(clock, next);
        Ok(())
    }

    /// Last reference closes the gate before committing the count.
    /// SOURCE: the one-to-zero transition in the same sources as `acquire`.
    pub fn release(
        &mut self,
        clock: <G::References as ClockReferences>::Clock,
    ) -> Result<(), ClockError<<G::References as ClockReferences>::Clock>> {
        let next = self
            .counts
            .get(clock)
            .checked_sub(1)
            .ok_or(ClockError::NotHeld(clock))?;
        if next == 0 {
            self.gates.set_clock(clock, false);
        }
        self.counts.set(clock, next);
        Ok(())
    }
}

#[cfg(test)]
mod tests;

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
