#![no_std]
use oer_ieee80211_port::ConnectionEpochKind;
use oer_ieee80211_sta::epoch::LinkEpochSource;
use oer_radio_port::{Child, Mint, Root, Slots};

pub fn source<'id>(
    mint: Mint<'id, (Slots<ConnectionEpochKind, 1>, Slots<ConnectionEpochKind, 1>)>,
) -> LinkEpochSource<'id, Child<Root<oer_radio_port::LocalId<'id>>, 0>> {
    let ([first], [second]) = mint.into_pools();
    #[cfg(ui_reject)]
    return LinkEpochSource::new(second); // ui-error: E0308
    #[cfg(not(ui_reject))]
    LinkEpochSource::new(first)
}
