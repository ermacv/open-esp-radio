#![no_std]
use oer_esp32s31_ieee80211::{
    lower_mac::{EnabledCore, FreeQueue, LowerMacCore, LowerMacHardware, MpduValues, TxAdmission},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn submit<'core, 'foreign, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware>(
    core: &mut EnabledCore<LowerMacCore<'core, 'slot, P, E, T, 512>>,
    hardware: &mut H,
    own: FreeQueue<'core>,
    foreign: FreeQueue<'foreign>,
    admission: TxAdmission<'core>,
    values: MpduValues<'slot, 512>,
) {
    #[cfg(ui_reject)]
    let _result = core.submit_checked(hardware, foreign, admission, values); // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let _result = core.submit_checked(hardware, own, admission, values);
}
