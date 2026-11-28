#![no_std]
use oer_chips_esp32s31_hal::{
    radio::domain::{ChipPhy, RadioDomain},
};
use oer_chips_esp32s31_radio_phy::{
    NoopPhyTargetObserver, RomShortDelay,
    executor::wait::WaitTally,
    owner::{PhyHandle, RfOpen},
};

pub fn other<T, Q>(
    timer: &T,
    radio: &mut RadioDomain,
    phy: &mut PhyHandle<'_, ChipPhy>,
    open: RfOpen<Q>,
) where
    T: oer_portable_cooperate::CooperativeTimer
        + oer_portable_settle::SettleCounter
        + oer_portable_clock::Clock
        + WaitTally,
{
    let mut platform = ();
    #[cfg(ui_reject)]
    let _pass = oer_chips_esp32s31_radio_phy::track::<(), RomShortDelay, _>(
        timer,
        radio,
        phy,
        open, // ui-error: E0308
        &mut platform,
        NoopPhyTargetObserver,
    );
    let _ = (timer, radio, phy, open, &mut platform);
}
