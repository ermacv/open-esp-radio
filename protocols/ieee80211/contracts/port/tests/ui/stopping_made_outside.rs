#![no_std]
use oer_protocols_ieee80211_contracts_port::{PortKey, Running, StopFrom, Stopping};
// A stop is made only by the port's key, from where it begins: a running
// radio cannot be turned into a stop by a core.
pub fn stop<'id>(key: &PortKey<'id>, running: Running<'id>) -> Stopping<'id> {
    #[cfg(ui_reject)]
    let stopping: Stopping<'id> = running.into(); // ui-error: E0277
    #[cfg(not(ui_reject))]
    let stopping = key.stop(StopFrom::Running(running));
    stopping
}
