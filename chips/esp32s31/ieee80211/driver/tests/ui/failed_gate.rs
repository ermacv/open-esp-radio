#![no_std]
use oer_esp32s31_ieee80211_driver::{
    lower_mac::{FreeQueue, FailedGateView, LowerMacCore, LowerMacHardware, MpduValues, TxAdmission},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn submit<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware>(
    failed: &mut FailedGateView<'_, LowerMacCore<'core, 'slot, P, E, T, C, 512>>,
    hardware: &mut H,
    queue: FreeQueue<'core>,
    admission: TxAdmission<'core>,
    values: MpduValues<'slot, 512>,
) {
    #[cfg(ui_reject)]
    let _result = failed.submit_checked(hardware, queue, admission, values); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _next_terminal_deadline = failed.next_deadline();
}
