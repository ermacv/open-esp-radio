#![no_std]
use oer_radio_port::{Disabled, Owner};
use oer_ieee80211_runtime::station_service::{
    PortAck, PortRxMeta, ServiceIo, StationPort, StationService,
};
use oer_ieee80211_upper_mac::rate_selection::RateSelection;
use oer_time::Timer;

pub fn run_once<'p, 'id, B: StationPort<'id>, S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>>(
    service: StationService<'id, B, S>,
    owner: Owner<'p, 'id, B, Disabled>,
    io: &mut ServiceIo<'_, B::Caps, 1, 1, 1>,
    timer: &impl Timer,
) {
    let run = service.run(owner, io, timer);
    #[cfg(ui_reject)]
    drop(service); // ui-error: E0382
    drop(run);
}
