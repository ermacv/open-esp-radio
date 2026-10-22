#![no_std]
use oer_esp32s31_ieee80211_driver::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_time::Timer;

pub fn submit<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, H: LowerMacHardware>(
    mut enabled: EnabledView<'_, LowerMacCore<'core, 'slot, oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile, P, E, T, C, 512>>,
    hardware: &mut H,
    queue: FreeQueue<'core>,
    admission: TxAdmission<'core>,
    values: MpduValues<'slot, 512>,
) {
    let mut closing = enabled.hold_new_attempts();
    #[cfg(ui_reject)]
    let _result = enabled.submit_checked(hardware, queue, admission, values); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let _result = closing.submit_checked(hardware, queue, admission, values);
}
