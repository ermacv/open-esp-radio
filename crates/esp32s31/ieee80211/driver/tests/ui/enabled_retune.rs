#![no_std]
use oer_esp32s31_ieee80211::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_time::Timer;
pub fn retune<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware, K: LowerMacSink<'core, 'slot, 512>>(mut core: EnabledCore<LowerMacCore<'core, 'slot, P, E, T, 512>>, channel: oer_ieee80211_mac::channel::WifiChannel, hardware: &mut H, sink: &mut K) {
    #[cfg(ui_reject)]
    core.set_checked_channel(channel); // ui-error: E0624
    #[cfg(not(ui_reject))]
    if let Ok(mut disabled) = core.disable_checked(hardware, sink).finish(sink) {
        disabled.set_checked_channel(channel);
        let _next = disabled.enable_checked();
    }
}
