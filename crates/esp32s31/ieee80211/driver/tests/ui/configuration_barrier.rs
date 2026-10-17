#![no_std]
use oer_esp32s31_ieee80211::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_time::Timer;
pub fn submit<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware, K: LowerMacSink<'core, 'slot, 512>>(mut core: Configuring<EnabledCore<LowerMacCore<'core, 'slot, P, E, T, 512>>>, hardware: &mut H, sink: &mut K, queue: FreeQueue<'core>, admission: TxAdmission<'core>, values: MpduValues<'slot,512>) {
    #[cfg(ui_reject)]
    let _result = core.submit_checked(hardware, queue, admission, values); // ui-error: E0599
    #[cfg(not(ui_reject))]
    if let ConfigurationProgress::Configured(mut core) = core.advance(hardware, sink) {
        let _result = core.submit_checked(hardware, queue, admission, values);
    }
}
