#![no_std]
use oer_protocols_ieee80211_contracts_port::{RadioPort, Running, StopRight};
pub fn stop<'id, P: RadioPort<'id>>(port: &mut P, running: Running<'id>, right: &mut StopRight<'id>) {
    #[cfg(ui_reject)]
    let _unasked = port.stop(running); // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _stopping = port.stop(right, running);
}
