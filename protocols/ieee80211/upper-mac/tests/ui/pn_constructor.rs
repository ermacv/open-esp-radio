#![no_std]
use oer_ieee80211_upper_mac::ccmp::CcmpTxPacketNumber;
use oer_ieee80211_upper_mac::ccmp::CcmpPacketNumberStep;
pub fn counter(existing: &CcmpTxPacketNumber) {
    #[cfg(ui_reject)]
    let _rewound = CcmpTxPacketNumber::new(CcmpPacketNumberStep::ONE); // ui-error: E0624
    #[cfg(not(ui_reject))]
    let _last = existing.last();
}
