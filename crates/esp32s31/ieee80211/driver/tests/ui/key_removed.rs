#![no_std]
use oer_esp32s31_ieee80211::{
    lower_mac::{KeyHandle, LowerMacCore, LowerMacHardware, LowerMacSink, TxTicket},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn remove<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware, S: LowerMacSink<'core, 'slot, 512>>(
    core: &mut LowerMacCore<'core, 'slot, P, E, T, 512>,
    hardware: &mut H,
    key: KeyHandle<'core>,
    ticket: TxTicket<'core>,
    sink: &mut S,
) {
    #[cfg(not(ui_reject))]
    let _admitted_before_removal = core.admit_key(&key, ticket);
    core.remove_checked_key(hardware, key, sink);
    #[cfg(ui_reject)]
    let _admitted_after_removal = core.admit_key(&key, ticket); // ui-error: E0382
}
