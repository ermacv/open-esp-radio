#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_controller::dtm::Reset;
use oer_bluetooth_le_port::{Idle, Session};
use oer_radio_port::{Root, SlotIdentity};
pub fn complete<A: SlotIdentity, B: SlotIdentity>(reset: Reset<Root<A>>, own: &Session<'_, Root<A>, Idle>, foreign: &Session<'_, Root<B>, Idle>) {
    #[cfg(ui_reject)]
    let _ = reset.after_install(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = reset.after_install(own);
}
