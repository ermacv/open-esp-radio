#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_core_installation::{Input, Machine, Step};
// The borrow of the machine is the ticket: while a step is out, the
// machine cannot be stepped.
pub fn twice<'id, F>(machine: &mut Machine<'id, u8, F>, now: Instant) {
    let first = machine.step(Input::Turn, now);
    #[cfg(ui_reject)]
    let _second = machine.step(Input::Turn, now); // ui-error: E0499
    if let Step::Rest(_) = first {}
}
