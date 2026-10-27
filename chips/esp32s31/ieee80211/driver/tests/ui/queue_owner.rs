#![no_std]
use oer_esp32s31_ieee80211_driver::{
    lower_mac::{EnabledView, FreeQueue, LowerMacCore, LowerMacHardware, MpduValues, TxAdmission},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn submit<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware>(
    core: &mut EnabledView<'_, LowerMacCore<'core, 'slot, P, E, T, C, 512>>,
    hardware: &mut H,
    queue: FreeQueue<'core>,
    first_admission: TxAdmission<'core>,
    second_admission: TxAdmission<'core>,
    first: MpduValues<'slot, 512>,
    second: MpduValues<'slot, 512>,
) {
    let returned = core.submit_checked(hardware, queue, first_admission, first);
    #[cfg(ui_reject)]
    let _second = core.submit_checked(hardware, queue, second_admission, second); // ui-error: E0382
    #[cfg(not(ui_reject))]
    if let Err(refused) = returned {
        let _second = core.submit_checked(hardware, refused.queue, second_admission, second);
    }
}
