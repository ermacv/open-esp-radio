#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, DataPort, GateOpen, QueueRight, SinglePlace, Station, TxFrame,
};
// A single's place names its interface by its type: a station's place is
// not published as an access point's.
pub fn publish<'id, P: DataPort<'id>>(
    port: &mut P,
    station: SinglePlace<'id, Station>,
    access_point: SinglePlace<'id, AccessPoint>,
    queue: QueueRight<'id>,
    gate: &GateOpen<'id>,
    frame: &TxFrame<'id, P::Caps>,
    now: Instant,
) {
    #[cfg(ui_reject)]
    port.publish_single::<AccessPoint>(station, queue, gate, frame, now); // ui-error: E0308
    #[cfg(not(ui_reject))]
    port.publish_single::<AccessPoint>(access_point, queue, gate, frame, now);
    let _ = station;
}
