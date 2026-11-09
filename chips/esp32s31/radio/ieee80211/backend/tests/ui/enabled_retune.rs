#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_portable_time::Timer;
pub fn retune<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::RadioCoexPriorities, H: LowerMacHardware, K: LowerMacSink<'core, 'slot, 512>>(mut core: EnabledView<'_, LowerMacCore<'core, 'slot, P, E, T, C, 512>>, channel: oer_protocols_ieee80211_wire::channel::Ghz2_4Channel, hardware: &mut H, sink: &mut K) {
    #[cfg(ui_reject)]
    core.set_checked_channel(channel); // ui-error: E0624
    #[cfg(not(ui_reject))]
    if let Ok(mut disabled) = core.disable_checked(hardware, sink).finish(sink) {
        disabled.set_checked_channel(channel);
        let _next = disabled.enable_checked();
    }
}
