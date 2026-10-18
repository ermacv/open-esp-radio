#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{CorePath, Submission, TicketKind, TicketPath};
use oer_esp32s31_ieee80211::lower_mac::{KeyUse, TxAdmission};
use oer_ieee80211_lower_mac::InFlight;
use oer_radio_port::Slot;
#[cfg(ui_reject)]
type Permission<'id> = Slot<'id, TicketKind, TicketPath>;
#[cfg(not(ui_reject))]
type Permission<'id> = KeyUse<'id, CorePath>;
pub fn submit<'id, C, M>(operation: InFlight<C, Slot<'id, TicketKind, TicketPath>, M>, permission: Permission<'id>) -> Submission<'id, C, M> {
    Submission { operation, admission: TxAdmission::Protected(permission) } // ui-error: E0308
}
