#![no_std]
use oer_radio_port::{Disabled, Owner};
use oer_ieee80211_runtime::station_service::{
    PortAck, PortRxMeta, ServiceIo, StationPort, StationService,
};
use oer_ieee80211_upper_mac::rate_selection::RateSelection;
use oer_time::Timer;

pub fn run_once<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id, C: oer_ieee80211_sta::machine::StaCallbacks, S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>>(
    service: StationService<'id, B, C, S>,
    owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
    io: &mut ServiceIo<'_, 'id, B::Caps, impl oer_ieee80211_runtime::station_service::ServiceOutputSink<'id, B::Caps, <B::Types<'id> as oer_ieee80211_port::PhaseTypes<'id>>::StationCommandPath>, 1, 1, 1, <B::Types<'id> as oer_ieee80211_port::PhaseTypes<'id>>::StationCommandPath>,
    timer: &impl Timer,
) {
    let run = service.run(owner, io, timer);
    #[cfg(ui_reject)]
    drop(service); // ui-error: E0382
    drop(run);
}
