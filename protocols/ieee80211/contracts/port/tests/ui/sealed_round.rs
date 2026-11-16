#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{ReceivePort, Round, data::Sealed};
pub fn round<'id, P: ReceivePort<'id>>(port: &mut P, now: Instant) {
    #[cfg(ui_reject)]
    let _staged = port.round_service(&Sealed(())); // ui-error: E0423
    let _rest = Round::open(port, now).close();
}
