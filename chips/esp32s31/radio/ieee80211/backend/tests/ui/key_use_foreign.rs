#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::{CorePools, KeyUse, TxAdmission};
use oer_protocols_radio_port::branded;
fn accept<'id>(_: &oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::CoreAuthority<'id>, _: TxAdmission<'id>) {}
pub async fn installations() {
    branded::<CorePools, _>(async |outer| {
        let outer = outer.into_pools();
        let [ticket, _, _, _] = outer.tickets;
        let admission = TxAdmission::Open(ticket);
        branded::<CorePools, _>(async |inner| {
            let inner = inner.into_pools();
            #[cfg(ui_reject)]
            accept(&inner.authority, admission); // ui-error@-3: lifetime may not live long enough
            #[cfg(not(ui_reject))]
            accept(&outer.authority, admission);
        }).await;
    }).await;
}
pub fn move_right<'id>(key: KeyUse<'id>) -> TxAdmission<'id> { TxAdmission::Protected(key) }
