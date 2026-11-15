#![no_std]
use core::task::Context;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{DataPort, Retired, StopRight};
pub fn stop<'id, P: DataPort<'id>>(
    port: &mut P,
    retired: Retired<'id>,
    mut right: StopRight<'id>,
    cx: &mut Context<'_>,
    now: Instant,
) {
    let _stopped = port.poll_stop(&mut right, cx, now);
    drop(port.release(retired, right));
    #[cfg(ui_reject)]
    let _after = port.poll_stop(&mut right, cx, now); // ui-error: E0382
}
pub fn recover<'id, P: DataPort<'id>>(port: &mut P, retired: Retired<'id>, mut right: StopRight<'id>) {
    let _recovered = port.recover_for_stop(&mut right);
    drop(port.release(retired, right));
    #[cfg(ui_reject)]
    let _after = port.recover_for_stop(&mut right); // ui-error: E0382
}
