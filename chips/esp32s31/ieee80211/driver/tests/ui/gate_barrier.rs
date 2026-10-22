#![no_std]
use oer_esp32s31_ieee80211_driver::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_time::Timer;
pub fn reopen<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware, K: LowerMacSink<'core, 'slot, 512>>(core: EnabledView<'_, LowerMacCore<'core, 'slot, oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile, P, E, T, C, 512>, ClosingGate>, hardware: &mut H, sink: &mut K) {
    #[cfg(ui_reject)]
    let _result = core.open_gate(hardware, sink); // ui-error: E0599
    #[cfg(not(ui_reject))]
    if let GateClosure::Closed(core) = core.close_gate(hardware) {
        let _result = core.open_gate(hardware, sink);
    }
}
