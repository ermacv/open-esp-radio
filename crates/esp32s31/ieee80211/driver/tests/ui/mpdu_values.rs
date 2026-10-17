#![no_std]
use oer_esp32s31_ieee80211::lower_mac::MpduValues;
pub fn change_length<'slot>(mut values: MpduValues<'slot, 512>) -> MpduValues<'slot, 512> {
    #[cfg(ui_reject)]
    { values.len = usize::MAX; } // ui-error: E0616
    values
}
