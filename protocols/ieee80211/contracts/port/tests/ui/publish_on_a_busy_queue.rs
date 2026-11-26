#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{Chain, DataPort, GateOpen, OnAir, Ppdu, QueueRight};
// A queue's right is the proof it is free: a publication takes it and the
// chain on the air holds it until its settlement gives it back, so a second
// publication on a queue that carries a chain cannot be written.
pub fn twice<'id, P: DataPort<'id>, const S: usize>(
    port: &mut P,
    first: Chain<'id, S>,
    second: Chain<'id, S>,
    queue: QueueRight<'id>,
    how: Ppdu<'id, P::Caps>,
    gate: &GateOpen<'id>,
    now: Instant,
) -> OnAir<'id, S> {
    let on_air = port.publish(first, queue, how, gate, now);
    #[cfg(ui_reject)]
    let _second = port.publish(second, queue, how, gate, now); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let _ = second;
    on_air
}
