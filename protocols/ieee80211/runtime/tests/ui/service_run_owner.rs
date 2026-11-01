#![no_std]
use oer_ieee80211_runtime::station::{PortAck, PortBlockAck, PortRxMeta, Runner, ServiceIo, StationPort};
use oer_ieee80211_core_upper_mac::rate_selection::RateSelection;
use oer_radio_port::{Disabled, Owner};
use oer_time::Timer;

pub fn run_once<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: oer_ieee80211_core_roles_sta::machine::StaCallbacks,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>, PortBlockAck<'id, B>>,
>(
    service: Runner<'id, B, C, S>,
    owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
    io: &mut ServiceIo<
        '_,
        B::Caps,
        impl oer_ieee80211_runtime::station::ServiceOutputSink<B::Caps>,
        B::LentRx,
        impl oer_ieee80211_runtime::station::NetworkStop,
        1,
        1,
        1,
    >,
    timer: &impl Timer,
    unwrap: &mut oer_ieee80211_runtime::station::KeyUnwrapSlot<
        impl oer_crypto::KeyUnwrap<{ oer_ieee80211_core_roles_sta::machine::KEY_DATA_CAPACITY }, Operation: Unpin>,
    >,
) {
    let run = service.run(owner, io, timer, unwrap);
    #[cfg(ui_reject)]
    drop(service); // ui-error: E0382
    drop(run);
}
