#![no_std]
use oer_ieee80211_upper_mac::ccmp::CcmpTxPacketNumber;
use oer_ieee80211_wire::ccmp::{CcmpPacketNumber, CcmpPacketNumberStep};
pub fn counter(existing: &CcmpTxPacketNumber) {
    #[cfg(ui_reject)]
    let _rewound = CcmpTxPacketNumber::continuing_after(CcmpPacketNumberStep::ONE, CcmpPacketNumber::ZERO); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _last = existing.last();
}
