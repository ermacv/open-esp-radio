//! The connected station's received data.
//!
//! The station's data transmission is `oer-protocols-ieee80211-core-datapath`'s
//! plane. A received data MPDU of the station's BSS gives its MSDUs only once the
//! replay check of the association's keys passed it
//! ([`CcmpRxReplayEpoch`](oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayEpoch)); the reorder, when it comes, runs before it.

use oer_protocols_ieee80211_contracts_port::{RxCryptoStatus, RxEvidence, RxMeta};

pub(crate) fn integrity(
    meta: &RxMeta,
) -> oer_protocols_ieee80211_core_upper_mac::receive::Integrity {
    use oer_protocols_ieee80211_core_upper_mac::receive::Integrity;
    match meta.crypto {
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
        | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified) => {
            Integrity::DecryptedAndVerified
        }
        _ => Integrity::Unverified,
    }
}

mod ethernet;
pub use ethernet::{
    EthernetFrame, FrameTooLong, MAX_ETHERNET_PAYLOAD_LEN, MsduDelivery, MsduPlan, PlannedCopyError,
};
