#![no_std]
use oer_esp32s31_ieee80211::lower_mac::{KeyUse, TxAdmission, TxTicket};
pub fn protected<'core>(key: KeyUse<'core>, ticket: TxTicket<'core>) -> TxAdmission<'core> {
    #[cfg(ui_reject)]
    { TxAdmission::Protected(ticket) } // ui-error: E0308
    #[cfg(not(ui_reject))]
    { TxAdmission::Protected(key) }
}
