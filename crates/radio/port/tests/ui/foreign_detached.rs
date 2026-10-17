#![no_std]
use oer_radio_port::{Detached, Retired};
fn finish<'id, S>(_: Retired<'id, S>, _: Detached<'id>) {}
#[cfg(ui_reject)]
pub fn foreign<'first: 'second, 'second, S>(
    retired: Retired<'first, S>,
    detached: Detached<'second>,
) {
    finish(retired, detached); // ui-error: null lifetime may not live long enough
}
#[cfg(not(ui_reject))]
pub fn same<'id, S>(retired: Retired<'id, S>, detached: Detached<'id>) {
    finish(retired, detached);
}
