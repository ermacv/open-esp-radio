#![no_std]
use oer_esp32s31_ieee80211::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_time::Timer;
pub fn enable<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware, K: LowerMacSink<'core, 'slot, 512>>(core: DisabledCore<LowerMacCore<'core, 'slot, P, E, T, 512>>, hardware: &mut H, sink: &mut K) {
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
