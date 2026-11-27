#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    DataPort, GateOpen, Layout, LongIn, LongNow, LongPlace, LongPpdu, QueueNow, QueuePlaces,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
// A queue's place answers a publication's proof that it is free; the proof
// borrows the place until its transition, after which the place is carried.
// Two publications on one queue at once cannot be written.
pub fn twice<'id, P: DataPort<'id>, M: LongIn<<P::Layout as Layout>::Longs>, const S: usize>(
    port: &mut P,
    first: &mut LongPlace<'id, M, S>,
    second: &mut LongPlace<'id, M, S>,
    queues: &mut QueuePlaces<'id>,
    how: LongPpdu<'id, P::Caps>,
    gate: &GateOpen<'id>,
    now: Instant,
) {
    let place = queues.at_mut(WmmAccessCategory::BestEffort);
    let (LongNow::Open(first), QueueNow::Free(free)) = (first.state(), place.state()) else {
        return;
    };
    #[cfg(ui_reject)]
    let again = place.state(); // ui-error: E0499
    let _air = first.publish(free, port, how, gate, now);
    #[cfg(ui_reject)]
    let _ = again;
    let _ = second;
}
