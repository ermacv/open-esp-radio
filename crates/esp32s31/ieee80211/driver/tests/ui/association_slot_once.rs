#![no_std]
use oer_esp32s31_ieee80211::{
    lower_mac::{
        LowerMacCore, LowerMacHardware,
        association::{StationAssociation, StationPairwiseSlot, TransmitKeyInstallation},
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_time::Timer;

pub fn install<'id, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: LowerMacHardware>(
    core: &mut LowerMacCore<'id, 'slot, P, E, T, 512>,
    hardware: &mut H,
    association: &StationAssociation<'id>,
    slot: StationPairwiseSlot<'id>,
) {
    let _first = core.install_checked_key(hardware, TransmitKeyInstallation::StationPairwise {
        association, slot, peer: [2; 6],
    }, &[1; 16]);
    #[cfg(ui_reject)]
    let _second = core.install_checked_key(hardware, TransmitKeyInstallation::StationPairwise {
        association, slot, peer: [2; 6], // ui-error: E0382
    }, &[1; 16]);
}
