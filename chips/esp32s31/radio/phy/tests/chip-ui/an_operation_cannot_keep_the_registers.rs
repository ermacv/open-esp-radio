#![no_std]
use oer_chips_esp32s31_hal::{
    radio::domain::{ChipPhy, RadioDomain},
};
use oer_chips_esp32s31_radio_phy::{
    NoopPhyTargetObserver, RomShortDelay,
    executor::wait::WaitTally,
    owner::{PhyHandle, RfOpen},
};

pub fn hold<T>(
    timer: &T,
    radio: &mut RadioDomain,
    phy: &mut PhyHandle<'_, ChipPhy>,
    open: RfOpen<ChipPhy>,
) where
    T: oer_portable_cooperate::CooperativeTimer
        + oer_portable_settle::SettleCounter
        + oer_portable_clock::Clock
        + WaitTally,
{
    let mut platform = ();
    let pass = oer_chips_esp32s31_radio_phy::track::<(), RomShortDelay, _>(
        timer,
        radio,
        phy,
        open,
        &mut platform,
        NoopPhyTargetObserver,
    );
    #[cfg(ui_reject)]
    let _registers = radio.phy_hardware(); // ui-error: E0499
    drop(pass);
}
