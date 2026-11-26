#![no_std]
use oer_protocols_ieee80211_contracts_port::PublishedSingle;
// A published single is kept once, from its publication to its end, which
// takes it: it is neither copied nor cloned.
pub fn twice(published: PublishedSingle) -> PublishedSingle {
    #[cfg(ui_reject)]
    let _another = published.clone(); // ui-error: E0599
    published
}
