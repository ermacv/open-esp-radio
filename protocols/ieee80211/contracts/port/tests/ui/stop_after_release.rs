#![no_std]
use oer_protocols_ieee80211_contracts_port::{RadioPort, Retired, Running, StopRight};
pub fn stop<'id, P: RadioPort<'id>>(
    port: &mut P,
    retired: Retired<'id>,
    running: Running<'id>,
    right: StopRight<'id>,
) {
    drop(port.release(retired, right));
    #[cfg(ui_reject)]
    let _after = port.stop(&mut right, running); // ui-error: E0382
    #[cfg(not(ui_reject))]
    drop(running);
}
