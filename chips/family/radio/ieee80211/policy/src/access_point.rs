//! The Espressif access point's local advertisement: the capabilities,
//! rates and WMM parameters its beacons, probe responses and association
//! responses announce.
//!
//! SOURCE(esp32s31): the archive access point's reviewed profile
//! (`open-esp-radio-rs` da26f64c3,
//! `crates/roles/esp32s31/ieee80211/ap/src/profile.rs`), on which WPA2
//! stations associated on hardware: the HT capability base `0x100c`
//! (SMPS disabled, DSSS/CCK in 40 MHz) the vendor's
//! `libnet80211[ieee80211_ht.o]::ieee80211_ht_attach` initializes, A-MPDU
//! parameters `0x03`, receive MCS 0-7 and TX MCS parameters `0x01`; the
//! legacy rates 1, 2, 5.5 and 11 Mbit/s basic with the OFDM set; the
//! vendor's WMM access-category parameters; ESS, Short Preamble and Short
//! Slot Time capability information.

use oer_portable_wire::U8;
use oer_protocols_ieee80211_wire::{
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
};

/// The HT capabilities the access point announces.
pub const HT_CAPABILITIES: HtLocalCapabilities = HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

/// One access category's parameters.
const fn ac(aifsn: U8<4>, ecw_min: U8<4>, ecw_max: U8<4>, txop: u16) -> WmmAcParameters {
    WmmAcParameters {
        admission_control_mandatory: false,
        aifsn,
        ecw_min,
        ecw_max,
        txop_limit_units_32_us: txop,
    }
}

/// The access point's advertisement.
pub const ADVERTISEMENT: Advertisement = {
    let Some(wmm) = WmmParameters::new(
        4,
        false,
        [
            ac(
                U8::constant::<3>(),
                U8::constant::<4>(),
                U8::constant::<10>(),
                0,
            ),
            ac(
                U8::constant::<7>(),
                U8::constant::<4>(),
                U8::constant::<10>(),
                0,
            ),
            ac(
                U8::constant::<2>(),
                U8::constant::<3>(),
                U8::constant::<4>(),
                94,
            ),
            ac(
                U8::constant::<2>(),
                U8::constant::<2>(),
                U8::constant::<3>(),
                47,
            ),
        ],
    ) else {
        panic!("the WMM parameters fit their fields");
    };
    let Some(advertisement) = Advertisement::new(
        LegacyRates::new(
            [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
            [0x6c, 0x12, 0x24, 0x48],
        ),
        HT_CAPABILITIES,
        wmm,
        // ESS, Short Preamble and Short Slot Time. Privacy follows the
        // access point's security.
        0x0421,
    ) else {
        panic!("the advertisement leaves Privacy to the security mode");
    };
    advertisement
};
