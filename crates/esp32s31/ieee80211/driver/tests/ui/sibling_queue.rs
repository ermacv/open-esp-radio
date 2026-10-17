#![no_std]
use oer_esp32s31_ieee80211::{
    lower_mac::{EnabledCore, FreeQueue, LowerMacCore, LowerMacHardware, MpduValues, TxAdmission},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

use oer_radio_port::{Child, Root};
pub fn submit<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware>(
    core: &mut EnabledCore<LowerMacCore<'core, 'slot, P, E, T, 512, Child<Root, 0>>>,
    hardware: &mut H,
    own: FreeQueue<'core, Child<Root, 0>>,
    sibling: FreeQueue<'core, Child<Root, 1>>,
    admission: TxAdmission<'core, Child<Root, 0>>,
    values: MpduValues<'slot, 512>,
) {
    #[cfg(ui_reject)]
    let _result = core.submit_checked(hardware, sibling, admission, values); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _result = core.submit_checked(hardware, own, admission, values);
}
