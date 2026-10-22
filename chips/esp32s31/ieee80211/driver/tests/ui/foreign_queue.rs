#![no_std]
use oer_esp32s31_ieee80211_driver::{
    lower_mac::{EnabledView, FreeQueue, LowerMacCore, LowerMacHardware, MpduValues, TxAdmission},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn submit<'core, 'foreign, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware>(
    core: &mut EnabledView<'_, LowerMacCore<'core, 'slot, oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile, P, E, T, C, 512>>,
    hardware: &mut H,
    own: FreeQueue<'core>,
    foreign: FreeQueue<'foreign>,
    admission: TxAdmission<'core>,
    values: MpduValues<'slot, 512>,
) {
    #[cfg(ui_reject)]
    let _result = core.submit_checked(hardware, foreign, admission, values); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let _result = core.submit_checked(hardware, own, admission, values);
}
