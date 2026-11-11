//! The connected station's data.
//!
//! An MSDU comes in the network's memory, its Ethernet frame where the
//! station reads it ([`MSDU_ETHERNET_OFFSET`]), and becomes one MPDU in
//! place, written once as a data arena of the port takes it: [`plane`] is
//! the station's data transmission over the port's two DMA arenas. It
//! keeps the link's header template, Block Ack windows and retry policy,
//! and draws each MPDU's sequence and packet numbers from the station's
//! link, whose spaces the station's own frames share.
//!
//! A received data MPDU of the station's BSS gives its MSDUs only once the
//! replay check of the association's keys passed it
//! ([`CcmpRxReplayEpoch`](oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayEpoch)); the reorder, when it comes, runs before it.

use oer_protocols_ieee80211_contracts_port::{RxCryptoStatus, RxEvidence, RxMeta};
/// The access category of a data frame.
pub use oer_protocols_ieee80211_wire::data::MsduParts;
pub use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
/// The TID of a data frame.
pub use oer_protocols_ieee80211_wire::qos::WmmUserPriority;

/// The length of an Ethernet header: destination, source and EtherType.
const ETHERNET_HEADER_LEN: usize = 14;

/// Where an MSDU's payload starts in a frame region: where the longest
/// data MPDU header ends, QoS Data with the CCMP header and LLC/SNAP. The
/// plane writes the header in front of the payload in place.
pub const MSDU_PAYLOAD_OFFSET: usize =
    oer_protocols_ieee80211_wire::data::IEEE80211_QOS_DATA_HEADER_LEN
        + oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN
        + oer_protocols_ieee80211_wire::data::LLC_SNAP_HEADER_LEN;

/// Where a network places an MSDU's Ethernet frame in a frame region: its
/// header right before [`MSDU_PAYLOAD_OFFSET`].
pub const MSDU_ETHERNET_OFFSET: usize = MSDU_PAYLOAD_OFFSET - ETHERNET_HEADER_LEN;

/// The protection of a link's data MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataProtection<K> {
    /// An ordinary open MPDU, without a CCMP header or key.
    Open,
    /// A CCMP MPDU using the link's pairwise key.
    Ccmp(K),
}

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

/// Move the payload at `MSDU_PAYLOAD_OFFSET..end` down to start at `to`,
/// one shift-long run at a time: each run's source lies right behind its
/// destination, so the two never overlap.
fn move_down(output: &mut [u8], to: usize, end: usize) -> Option<()> {
    let shift = MSDU_PAYLOAD_OFFSET.checked_sub(to)?;
    output.get(..end)?;
    let mut at = to;
    while let Some(from) = at.checked_add(shift).filter(|&from| from < end) {
        let (front, back) = output.split_at_mut_checked(from)?;
        let run = shift.min(end.checked_sub(from)?);
        let destination = front.get_mut(at..)?.get_mut(..run)?;
        for (octet, source) in destination.iter_mut().zip(back.get(..run)?) {
            *octet = *source;
        }
        at = from;
    }
    Some(())
}

mod aggregate;
pub use aggregate::{AggregateRetry, MAX_AGGREGATE, PpduLimit};
mod ethernet;
pub mod plane;
pub use ethernet::{
    EthernetFrame, FrameTooLong, MAX_ETHERNET_PAYLOAD_LEN, MsduDelivery, MsduPlan, PlannedCopyError,
};

#[cfg(test)]
mod tests;
