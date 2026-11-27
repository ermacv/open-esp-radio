#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, DataPort, Layout, LongIn, LongNow, LongPlace, Station,
};
use oer_protocols_ieee80211_wire::qos::Tid;
// A chain is opened for one interface's link, which names the interface
// its frames go as: a station's link does not open an access point's chain.
pub fn open<'id, P: DataPort<'id>, M: LongIn<<P::Layout as Layout>::Longs>, const S: usize>(
    port: &mut P,
    place: &mut LongPlace<'id, M, S>,
    station: &P::Link<Station>,
    access_point: &P::Link<AccessPoint>,
    tid: Tid,
) {
    let _ = (station, access_point);
    let LongNow::Idle(idle) = place.state() else {
        return;
    };
    #[cfg(ui_reject)]
    let _open = idle.open::<AccessPoint, P>(port, station, tid); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _open = idle.open::<AccessPoint, P>(port, access_point, tid);
}
