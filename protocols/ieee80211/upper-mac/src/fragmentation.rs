//! Bounded data-fragment reassembly above the port.

#[cfg(test)]
use oer_ieee80211_wire::data::DataInterfaceRole;

use oer_ieee80211_wire::{ccmp::CcmpPacketNumber, data::LLC_SNAP_HEADER_LEN, fragmentation::*};

mod reassembly;
pub use reassembly::{
    OpenDataDefragmentation, OpenDataDefragmenter, OpenDataFragmentAdmission,
    OpenDataFragmentPreflight, OpenDataUnfragmentedAdmission, OpenReassembledData,
};

/// The local lifetime admitted with a fragment train. CCMP callers must use
/// the installation generation of the authenticated key, including across a
/// same-KeyID rekey; an association generation or wire KeyID is insufficient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FragmentAdmissionEpoch {
    OpenAssociation(u64),
    CcmpKeyInstallation(u64),
}

impl FragmentAdmissionEpoch {
    fn validate(self, protection: DataFragmentProtection) -> Result<(), OpenDataFragmentError> {
        match (self, protection) {
            (Self::OpenAssociation(_), DataFragmentProtection::Open)
            | (Self::CcmpKeyInstallation(_), DataFragmentProtection::Ccmp) => Ok(()),
            _ => Err(OpenDataFragmentError::KeyEpochRequired),
        }
    }
}

/// Payload bytes retained for one ordinary 1,500-byte Ethernet payload plus
/// its LLC/SNAP header. The Ethernet header is reconstructed from the exact
/// admitted 802.11 address tuple and therefore consumes no retained bytes.
pub const OPEN_DATA_REASSEMBLY_CAPACITY: usize = 1_500 + LLC_SNAP_HEADER_LEN;

/// Software resource lifetime for an incomplete Open MSDU.
///
/// This is deliberately not presented as an ESP32-S31 hardware value. It is
/// a finite host-owned eviction policy used only when a runtime clock sample
/// accompanies the received fragment.
pub const OPEN_DATA_FRAGMENT_TIMEOUT: oer_time::Duration = oer_time::Duration::from_secs(1);

#[cfg(test)]
const DATA: u16 = 0x0008;
#[cfg(test)]
const QOS_DATA: u16 = 0x0088;
#[cfg(test)]
const TO_DS: u16 = 0x0100;
#[cfg(test)]
const FROM_DS: u16 = 0x0200;
#[cfg(test)]
const MORE_FRAGMENTS: u16 = 0x0400;
#[cfg(test)]
const RETRY: u16 = 0x0800;
#[cfg(test)]
const QOS_AMSDU_PRESENT: u8 = 0x80;

#[cfg(test)]
mod tests;
