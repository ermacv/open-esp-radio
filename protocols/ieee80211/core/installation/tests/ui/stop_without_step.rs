#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{RadioPort, Running};
use oer_protocols_ieee80211_core_installation::{Input, Machine, Step};
// A pump stops the port only with the right a `Step::Stop` lends: the
// machine lends it to nothing else.
pub fn pump<'id, P: RadioPort<'id>, D>(
    machine: &mut Machine<'id, P::Channel, D>,
    port: &mut P,
    running: Running<'id>,
    now: Instant,
) {
    #[cfg(ui_reject)]
    let _unasked = port.stop(machine.right(), running); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let Running { .. } = running;
    if let Step::Stop(ask, mut resume) = machine.step(Input::Turn, now) {
        let _lent = port.stop(resume.right(), match ask {
            oer_protocols_ieee80211_core_installation::StopAsk::Running(running) => running,
            _ => return,
        });
    }
}
