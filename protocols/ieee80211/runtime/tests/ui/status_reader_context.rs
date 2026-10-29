#![no_std]
use oer_ieee80211_runtime::station::{Status, StatusReader};
#[cfg(ui_reject)]
fn require_send<T: Send>(_: T) {}
pub fn read_here(reader: StatusReader<'_>) -> Status {
    #[cfg(ui_reject)]
    require_send(reader); // ui-error: E0277
    reader.status()
}
