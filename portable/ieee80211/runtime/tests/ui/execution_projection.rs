#![no_std]
use oer_ieee80211_lower_mac::InFlight;
#[cfg(ui_reject)]
use oer_ieee80211_lower_mac::TxMpdu;
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelMpdu, ModelSubframes};

pub fn lend<'id, C, T>(
    single: &mut InFlight<C, T, ModelMpdu<'id>>,
    aggregate: &mut InFlight<C, T, ModelSubframes<'id>>,
) {
    let mut single = single.execute();
    let mut aggregate = aggregate.execute();
    #[cfg(ui_reject)]
    {
        let _: &mut ModelMpdu<'id> = &mut single; // ui-error: E0308
        let _: &mut ModelSubframes<'id> = &mut aggregate; // ui-error: E0308
        single.into_parts(); // ui-error: E0599
        aggregate.into_iter(); // ui-error: E0599
    }
    #[cfg(not(ui_reject))]
    core::hint::black_box((&mut single, &mut aggregate));
}
