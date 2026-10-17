#![no_std]
use oer_esp32s31_ieee80211::{
    lower_mac::{KeyHandle, ReceiveKeyHandle, LowerMacCore, LowerMacHardware, LowerMacSink, TxTicket},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn receive_only<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware, S: LowerMacSink<'core, 'slot, 512>>(
    core: &mut LowerMacCore<'core, 'slot, P, E, T, 512>,
    hardware: &mut H,
    receive: ReceiveKeyHandle<'core>,
    transmit: KeyHandle<'core>,
    ticket: TxTicket<'core>,
    sink: &mut S,
) {
    #[cfg(ui_reject)]
    let _admission = core.admit_key(&receive, ticket); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _admission = core.admit_key(&transmit, ticket);
    core.remove_checked_key(hardware, receive, sink);
}
