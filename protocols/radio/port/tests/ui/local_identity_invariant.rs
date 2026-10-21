#![no_std]
use oer_radio_port::LocalId;

pub fn select<'long: 'short, 'short>(
    own: LocalId<'short>,
    longer: LocalId<'long>,
) -> LocalId<'short> {
    #[cfg(ui_reject)]
    return longer; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    own
}
