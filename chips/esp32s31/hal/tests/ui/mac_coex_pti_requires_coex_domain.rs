#![no_std]
use oer_esp32s31_hal::{coex::CoexPti, ieee80211::init::MacCoexPti, types::MacPti};

pub fn project(pti: CoexPti) {
    let _ = MacCoexPti::from(pti);
    let _ = MacPti::from_coex(pti);
    #[cfg(ui_reject)]
    let _ = MacCoexPti::from(16_u8); // ui-error: E0277
    #[cfg(ui_reject)]
    let _ = CoexPti(16); // ui-error: E0423
}
