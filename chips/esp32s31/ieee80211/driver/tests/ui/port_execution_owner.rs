#![no_std]
use oer_esp32s31_ieee80211_driver::port::{Device, Mpdu};
use oer_ieee80211_port::InFlight;
#[cfg(ui_reject)]
use oer_ieee80211_port::TxMpdu;

pub fn lend<'id, Id: oer_radio_port::SlotIdentity + 'id, D: Device<B>, const B: usize, C, T>(operation: &mut InFlight<C, T, Mpdu<'id, Id, D, B>>) {
    let mut view = operation.execute();
    #[cfg(ui_reject)]
    {
        let _: &mut Mpdu<'id, Id, D, B> = &mut view; // ui-error: E0308
        view.into_parts(); // ui-error: E0599
    }
    #[cfg(not(ui_reject))]
    core::hint::black_box(&mut view);
}
