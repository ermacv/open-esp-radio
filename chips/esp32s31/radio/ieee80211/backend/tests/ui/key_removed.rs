#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::{
    lower_mac::{association::StationPairwiseSlot, KeyHandle, LowerMacCore, LowerMacHardware, LowerMacSink, TxTicket},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_portable_time::Timer;

pub fn remove<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::RadioCoexPriorities, H: LowerMacHardware, S: LowerMacSink<'core, 'slot, 512>>(
    core: &mut LowerMacCore<'core, 'slot, P, E, T, C, 512>,
    hardware: &mut H,
    key: KeyHandle<'core>,
    ticket: TxTicket<'core>,
    sink: &mut S,
) {
    #[cfg(not(ui_reject))]
    let _admitted_before_removal = core.admit_key(&key, ticket);
    let _place = key
        .into_removing::<StationPairwiseSlot<'core>>()
        .map(|removing| core.remove_checked_key(hardware, removing, sink));
    #[cfg(ui_reject)]
    let _admitted_after_removal = core.admit_key(&key, ticket); // ui-error: E0382
}
