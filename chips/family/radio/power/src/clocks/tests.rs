use super::*;

#[test]
fn only_the_rtc_crystal_selector_chooses_wifi_xtal32k() {
    use std::cell::Cell;
    struct Port {
        rtc: RtcSlowClockSource,
        selected: Option<WifiLowPowerClockSource>,
        reads: Cell<u8>,
    }
    impl LowPowerClockPort for Port {
        fn rtc_slow_clock_source(&self) -> RtcSlowClockSource {
            self.reads.set(self.reads.get() + 1);
            self.rtc
        }
        fn select_wifi_low_power_clock(&mut self, source: WifiLowPowerClockSource) {
            self.selected = Some(source);
        }
    }
    for (rtc, expected) in [
        (
            RtcSlowClockSource::Crystal32Khz,
            WifiLowPowerClockSource::Crystal32Khz,
        ),
        (
            RtcSlowClockSource::SlowOscillator,
            WifiLowPowerClockSource::SlowOscillator,
        ),
        (
            RtcSlowClockSource::Invalid(2),
            WifiLowPowerClockSource::SlowOscillator,
        ),
        (
            RtcSlowClockSource::Invalid(u8::MAX),
            WifiLowPowerClockSource::SlowOscillator,
        ),
    ] {
        let mut port = Port {
            rtc,
            selected: None,
            reads: Cell::new(0),
        };
        select_wifi_low_power_clock(&mut port);
        assert_eq!(port.selected, Some(expected));
        assert_eq!(port.reads.get(), 1);
    }
}
