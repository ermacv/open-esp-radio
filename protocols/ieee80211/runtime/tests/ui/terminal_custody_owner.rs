#![no_std]
use oer_ieee80211_runtime::station_service::{QuarantinedTerminals, StationPort};

pub fn return_once<'id, B: StationPort<'id>>(bank: QuarantinedTerminals<'id, B>) {
    let returned = bank.into_parts();
    #[cfg(ui_reject)]
    drop(bank); // ui-error: E0382
    drop(returned);
}
