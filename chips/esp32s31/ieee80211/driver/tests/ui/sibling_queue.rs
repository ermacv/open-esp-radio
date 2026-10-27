#![no_std]
use oer_esp32s31_ieee80211_driver::{
    lower_mac::{EnabledView, FreeQueue, LowerMacCore, LowerMacHardware, MpduValues, TxAdmission},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

use oer_radio_port::{Child, Root};
pub fn submit<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware>(
    core: &mut EnabledView<'_, LowerMacCore<'core, 'slot, P, E, T, C, 512, Child<Root<oer_radio_port::LocalId<'core>>, 0>>>,
    hardware: &mut H,
    own: FreeQueue<'core, Child<Root<oer_radio_port::LocalId<'core>>, 0>>,
    sibling: FreeQueue<'core, Child<Root<oer_radio_port::LocalId<'core>>, 1>>,
    admission: TxAdmission<'core, Child<Root<oer_radio_port::LocalId<'core>>, 0>>,
    values: MpduValues<'slot, 512>,
) {
    #[cfg(ui_reject)]
    let _result = core.submit_checked(hardware, sibling, admission, values); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _result = core.submit_checked(hardware, own, admission, values);
}
