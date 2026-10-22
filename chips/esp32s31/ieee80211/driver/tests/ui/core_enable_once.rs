#![no_std]
use oer_esp32s31_ieee80211_driver::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_time::Timer;
pub fn enable<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware, K: LowerMacSink<'core, 'slot, 512>>(core: DisabledView<'_, LowerMacCore<'core, 'slot, oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile, P, E, T, C, 512>>, hardware: &mut H, sink: &mut K) {
    let first = core.enable_checked();
    #[cfg(ui_reject)]
    let _second = core.enable_checked(); // ui-error: E0382
    #[cfg(not(ui_reject))]
    if let LifecycleStart::Enabled(core) = first {
        if let Ok(disabled) = core.disable_checked(hardware, sink).finish(sink) {
            let _second = disabled.enable_checked();
        }
    }
}
