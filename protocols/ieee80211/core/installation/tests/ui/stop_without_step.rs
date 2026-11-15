#![no_std]
use core::task::Context;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::DataPort;
use oer_protocols_ieee80211_core_installation::{Input, Machine, Step};
// A pump stops the port only with the right a `Step::Stop` lends: the
// machine lends it to nothing else.
pub fn pump<'id, P: DataPort<'id>, C: Copy>(
    machine: &mut Machine<'id, C, P::Fault>,
    port: &mut P,
    cx: &mut Context<'_>,
    now: Instant,
) {
    #[cfg(ui_reject)]
    let _unasked = port.poll_stop(machine.right(), cx, now); // ui-error: E0599
    if let Step::Stop(mut resume) = machine.step(Input::Turn, now) {
        let _next = match port.poll_stop(resume.right(), cx, now) {
            core::task::Poll::Ready(answer) => resume.answer(answer),
            core::task::Poll::Pending => resume.pending(),
        };
    }
}
