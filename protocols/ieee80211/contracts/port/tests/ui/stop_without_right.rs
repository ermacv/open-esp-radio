#![no_std]
use core::task::Context;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{DataPort, StopRight};
pub fn stop<'id, P: DataPort<'id>>(
    port: &mut P,
    right: &mut StopRight<'id>,
    cx: &mut Context<'_>,
    now: Instant,
) {
    #[cfg(ui_reject)]
    let _unasked = port.poll_stop(cx, now); // ui-error: E0061
    #[cfg(ui_reject)]
    let _unrecovered = port.recover_for_stop(); // ui-error: E0061
    let _stopped = port.poll_stop(right, cx, now);
    let _recovered = port.recover_for_stop(right);
}
