use super::{PlatformPllSourceBaseline, WifiPowerBaseline};
use crate::modem::syscon::ModemSysconPowerBaseline;

const BASELINE: PlatformPllSourceBaseline = PlatformPllSourceBaseline {
    modem_apb_clock_enabled: false,
    modem_source_clock_enabled: true,
    modem_pll_selected: false,
    modem_pll_clock_enabled: false,
    modem_xtal_clock_enabled: true,
};

#[test]
fn packed_power_baseline_preserves_every_decoded_field() {
    for bus_clock in [false, true] {
        let baseline =
            WifiPowerBaseline::new(bus_clock, BASELINE, ModemSysconPowerBaseline::default());
        assert_eq!(baseline.modem_register_bus_clock_enabled(), bus_clock);
        assert_eq!(baseline.pll_source(), BASELINE);
        assert_eq!(baseline.modem_syscon(), ModemSysconPowerBaseline::default());
    }
}
