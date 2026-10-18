//! Typed ownership of one AP GTK and the bounded pairwise CCMP slot table.

use oer_esp32s31_ieee80211_mac::crypto::{
    AP_PAIRWISE_SLOT_COUNT, ApGroupCcmpSlot, ApPairwiseCcmpSlot, CcmpTxPacketNumberError,
    CryptoKeyError, install_ap_group_ccmp, install_ap_pairwise_ccmp,
};

use oer_ieee80211_mac::ccmp::{
    CcmpPacketNumber, CcmpReplayError, CcmpReplayLane, CcmpRxReplayCandidate, CcmpRxReplayState,
};

use oer_ieee80211_rsn::{Ptk, frames::RsnGtk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSecurityError {
    Crypto(CryptoKeyError),
    PacketNumber(CcmpTxPacketNumberError),
    SecurityModeMismatch,
    PairwiseStorageNotEmpty,
    PairwiseAlreadyInstalled,
    AssociationIdAlreadyInstalled,
    WrongPeer,
    Replay(CcmpReplayError),
}

impl From<CryptoKeyError> for ApSecurityError {
    fn from(error: CryptoKeyError) -> Self {
        Self::Crypto(error)
    }
}

impl From<CcmpTxPacketNumberError> for ApSecurityError {
    fn from(error: CcmpTxPacketNumberError) -> Self {
        Self::PacketNumber(error)
    }
}

impl From<CcmpReplayError> for ApSecurityError {
    fn from(error: CcmpReplayError) -> Self {
        Self::Replay(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSecurityStopReport {
    OpenNoKeys,
    Wpa2Personal {
        pairwise_slots_cleared: u8,
        group_hardware_index: u8,
    },
}

/// O(1) identity of one installed pairwise hardware-key slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPairwiseBinding {
    index: u8,
    peer: [u8; 6],
    hardware_index: u8,
    generation: u32,
}

impl ApPairwiseBinding {
    pub const fn hardware_index(self) -> u8 {
        self.hardware_index
    }

    pub(crate) const fn generation(self) -> u32 {
        self.generation
    }
}

/// Two-phase replay admission tied to one exact pairwise-key generation.
///
/// The binding is revalidated at commit, so a candidate prepared before a
/// peer clear or PTK replacement cannot mutate the replacement key's replay
/// frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPairwiseRxCandidate {
    binding: ApPairwiseBinding,
    replay: CcmpRxReplayCandidate,
}

pub struct ApSecurityStartFailure<'storage> {
    pub error: ApSecurityError,
    pub storage: &'storage mut ApPairwiseKeyStorage,
}

impl core::fmt::Debug for ApSecurityStartFailure<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ApSecurityStartFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Stable caller-owned table of installed AP pairwise-key capabilities.
pub struct ApPairwiseKeyStorage {
    pairwise: [Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize],
    generations: [u32; AP_PAIRWISE_SLOT_COUNT as usize],
    rx_replay: [CcmpRxReplayState; AP_PAIRWISE_SLOT_COUNT as usize],
}

impl ApPairwiseKeyStorage {
    pub const fn new() -> Self {
        Self {
            pairwise: [const { None }; AP_PAIRWISE_SLOT_COUNT as usize],
            generations: [0; AP_PAIRWISE_SLOT_COUNT as usize],
            rx_replay: [const { CcmpRxReplayState::new(CcmpPacketNumber::ZERO) };
                AP_PAIRWISE_SLOT_COUNT as usize],
        }
    }
}

impl Default for ApPairwiseKeyStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub enum ApSecurity<'storage> {
    Open {
        storage: Option<&'storage mut ApPairwiseKeyStorage>,
    },
    Wpa2Personal {
        group: ApGroupCcmpSlot,
        storage: Option<&'storage mut ApPairwiseKeyStorage>,
    },
}

impl<'storage> ApSecurity<'storage> {
    pub fn install_group<H>(
        hardware: &mut H,
        gtk: &RsnGtk,
        storage: &'storage mut ApPairwiseKeyStorage,
    ) -> Result<Self, ApSecurityStartFailure<'storage>>
    where
        H: oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware,
    {
        if storage.pairwise.iter().any(Option::is_some) {
            return Err(ApSecurityStartFailure {
                error: ApSecurityError::PairwiseStorageNotEmpty,
                storage,
            });
        }
        let group = match install_ap_group_ccmp(hardware, gtk.key_id(), gtk.key()) {
            Ok(group) => group,
            Err(error) => {
                return Err(ApSecurityStartFailure {
                    error: ApSecurityError::Crypto(error),
                    storage,
                });
            }
        };
        Ok(Self::Wpa2Personal {
            group,
            storage: Some(storage),
        })
    }

    /// Bind an Open AP epoch without touching any hardware key entry.
    pub fn open(
        storage: &'storage mut ApPairwiseKeyStorage,
    ) -> Result<Self, ApSecurityStartFailure<'storage>> {
        if storage.pairwise.iter().any(Option::is_some) {
            return Err(ApSecurityStartFailure {
                error: ApSecurityError::PairwiseStorageNotEmpty,
                storage,
            });
        }
        Ok(Self::Open {
            storage: Some(storage),
        })
    }

    pub fn install_pairwise<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        association_id: u16,
        ptk: &Ptk,
    ) -> Result<(), ApSecurityError>
    where
        H: oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware,
    {
        if matches!(self, Self::Open { .. }) {
            return Err(ApSecurityError::SecurityModeMismatch);
        }
        if self
            .slots()
            .iter()
            .flatten()
            .any(|slot| slot.peer() == &peer)
        {
            return Err(ApSecurityError::PairwiseAlreadyInstalled);
        }
        let index = usize::from(
            u8::try_from(
                association_id
                    .checked_sub(1)
                    .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?,
            )
            .map_err(|_| CryptoKeyError::InvalidAccessPointAssociationId)?,
        );
        let destination = self
            .slots()
            .get(index)
            .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?;
        if destination.is_some() {
            return Err(ApSecurityError::AssociationIdAlreadyInstalled);
        }
        let next_generation = self.storage().generations[index]
            .checked_add(1)
            .expect("AP pairwise-key generation space is not reusable");
        let slot = install_ap_pairwise_ccmp(hardware, peer, association_id, ptk.temporal_key())?;
        let storage = self.storage_mut();
        storage.pairwise[index] = Some(slot);
        storage.generations[index] = next_generation;
        storage.rx_replay[index] = CcmpRxReplayState::default();
        Ok(())
    }

    pub fn clear_peer<H>(&mut self, hardware: &mut H, peer: [u8; 6]) -> Result<(), ApSecurityError>
    where
        H: oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware,
    {
        let Some(index) = self
            .slots()
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|slot| slot.peer() == &peer))
        else {
            return Ok(());
        };
        let slot = self.slots_mut()[index]
            .take()
            .expect("matching pairwise slot is occupied");
        slot.clear(hardware);
        let storage = self.storage_mut();
        storage.generations[index] = storage.generations[index]
            .checked_add(1)
            .expect("AP pairwise-key generation space is not reusable");
        storage.rx_replay[index] = CcmpRxReplayState::default();
        Ok(())
    }

    /// Allocate the next pairwise TX packet number for the owned peer.
    ///
    /// A failed later frame build may leave a gap, which is valid for CCMP;
    /// the packet number is never rolled back or reused.
    pub fn next_pairwise_tx_ccmp_header(
        &mut self,
        peer: [u8; 6],
    ) -> Result<[u8; 8], ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(ApSecurityError::SecurityModeMismatch);
        }
        let slot = self
            .slots_mut()
            .iter_mut()
            .flatten()
            .find(|slot| slot.peer() == &peer)
            .ok_or(ApSecurityError::WrongPeer)?;
        Ok(slot.next_tx_ccmp_header()?)
    }

    pub fn bind_pairwise(
        &self,
        peer: [u8; 6],
        association_id: u16,
    ) -> Result<ApPairwiseBinding, ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(ApSecurityError::SecurityModeMismatch);
        }
        let index = usize::from(
            u8::try_from(
                association_id
                    .checked_sub(1)
                    .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?,
            )
            .map_err(|_| CryptoKeyError::InvalidAccessPointAssociationId)?,
        );
        let slot = self
            .slots()
            .get(index)
            .and_then(Option::as_ref)
            .filter(|slot| slot.peer() == &peer)
            .ok_or(ApSecurityError::WrongPeer)?;
        Ok(ApPairwiseBinding {
            index: u8::try_from(index)
                .map_err(|_| CryptoKeyError::InvalidAccessPointAssociationId)?,
            peer,
            hardware_index: slot.hardware_index(),
            generation: self.storage().generations[index],
        })
    }

    /// Prepare one post-reorder, hardware-authenticated pairwise CCMP MPDU.
    ///
    /// Callers must commit the returned token before publishing any Ethernet
    /// view. The token remains fenced to this exact installed PTK generation.
    pub fn prepare_bound_pairwise_rx(
        &self,
        binding: ApPairwiseBinding,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<ApPairwiseRxCandidate, ApSecurityError> {
        let index = self.validate_pairwise_binding(binding)?;
        let replay = self.storage().rx_replay[index].prepare(lane, packet_number)?;
        Ok(ApPairwiseRxCandidate { binding, replay })
    }

    /// Commit a prepared PN only while its pairwise key generation is still
    /// current. A clear/reinstall edge invalidates the candidate first.
    pub fn commit_bound_pairwise_rx(
        &mut self,
        candidate: ApPairwiseRxCandidate,
    ) -> Result<(), ApSecurityError> {
        let index = self.validate_pairwise_binding(candidate.binding)?;
        self.storage_mut().rx_replay[index].commit(candidate.replay)?;
        Ok(())
    }

    /// Admit and commit one ordinary pairwise MPDU in the replay owner's
    /// synchronous transaction.
    ///
    /// Fragment reassembly keeps the split prepare/commit API above because
    /// its candidate survives an ownership boundary. Ordinary post-reorder
    /// delivery has no such boundary, so validating the same binding twice
    /// only adds work to every AP data frame.
    #[inline(always)]
    pub fn commit_bound_pairwise_rx_immediate(
        &mut self,
        binding: ApPairwiseBinding,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<(), ApSecurityError> {
        let index = self.validate_pairwise_binding(binding)?;
        self.storage_mut().rx_replay[index].commit_immediate(lane, packet_number)?;
        Ok(())
    }

    pub fn next_bound_pairwise_tx_ccmp_header(
        &mut self,
        binding: ApPairwiseBinding,
    ) -> Result<[u8; 8], ApSecurityError> {
        let index = self.validate_pairwise_binding(binding)?;
        let slot = self
            .slots_mut()
            .get_mut(index)
            .and_then(Option::as_mut)
            .expect("validated pairwise binding owns an occupied slot");
        Ok(slot.next_tx_ccmp_header()?)
    }

    pub fn pairwise_hardware_index(&self, peer: [u8; 6]) -> Result<u8, ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(ApSecurityError::SecurityModeMismatch);
        }
        let slot = self
            .slots()
            .iter()
            .flatten()
            .find(|slot| slot.peer() == &peer)
            .ok_or(ApSecurityError::WrongPeer)?;
        Ok(slot.hardware_index())
    }

    pub fn next_group_tx_ccmp_header(&mut self) -> Result<[u8; 8], ApSecurityError> {
        match self {
            Self::Open { .. } => Err(ApSecurityError::SecurityModeMismatch),
            Self::Wpa2Personal { group, .. } => Ok(group.next_tx_ccmp_header()?),
        }
    }

    pub const fn group_hardware_index(&self) -> Result<u8, ApSecurityError> {
        match self {
            Self::Open { .. } => Err(ApSecurityError::SecurityModeMismatch),
            Self::Wpa2Personal { group, .. } => Ok(group.hardware_index()),
        }
    }

    /// Clear every installed AP key before the radio owner may become stopped.
    pub fn stop<H>(
        self,
        hardware: &mut H,
    ) -> (ApSecurityStopReport, &'storage mut ApPairwiseKeyStorage)
    where
        H: oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware,
    {
        let (storage, group) = match self {
            Self::Open { mut storage } => (
                storage
                    .take()
                    .expect("active Open AP security owns pairwise-key storage"),
                None,
            ),
            Self::Wpa2Personal { group, mut storage } => (
                storage
                    .take()
                    .expect("active WPA2 AP security owns pairwise-key storage"),
                Some(group),
            ),
        };
        let mut pairwise_slots_cleared = 0_u8;
        for index in 0..storage.pairwise.len() {
            if let Some(pairwise) = storage.pairwise[index].take() {
                pairwise.clear(hardware);
                storage.generations[index] = storage.generations[index]
                    .checked_add(1)
                    .expect("AP pairwise-key generation space is not reusable");
                storage.rx_replay[index] = CcmpRxReplayState::default();
                pairwise_slots_cleared = pairwise_slots_cleared.saturating_add(1);
            }
        }
        let report = if let Some(group) = group {
            let group_hardware_index = group.hardware_index();
            group.clear(hardware);
            ApSecurityStopReport::Wpa2Personal {
                pairwise_slots_cleared,
                group_hardware_index,
            }
        } else {
            debug_assert_eq!(pairwise_slots_cleared, 0);
            ApSecurityStopReport::OpenNoKeys
        };
        (report, storage)
    }

    fn slots(&self) -> &[Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize] {
        &match self {
            Self::Open { storage } | Self::Wpa2Personal { storage, .. } => storage,
        }
        .as_deref()
        .expect("active AP security owns pairwise-key storage")
        .pairwise
    }

    fn slots_mut(&mut self) -> &mut [Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize] {
        &mut self.storage_mut().pairwise
    }

    fn storage(&self) -> &ApPairwiseKeyStorage {
        match self {
            Self::Open { storage } | Self::Wpa2Personal { storage, .. } => storage,
        }
        .as_deref()
        .expect("active AP security owns pairwise-key storage")
    }

    fn storage_mut(&mut self) -> &mut ApPairwiseKeyStorage {
        match self {
            Self::Open { storage } | Self::Wpa2Personal { storage, .. } => storage,
        }
        .as_deref_mut()
        .expect("active AP security owns pairwise-key storage")
    }

    fn validate_pairwise_binding(
        &self,
        binding: ApPairwiseBinding,
    ) -> Result<usize, ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(ApSecurityError::SecurityModeMismatch);
        }
        let index = usize::from(binding.index);
        let storage = self.storage();
        let slot = storage
            .pairwise
            .get(index)
            .and_then(Option::as_ref)
            .filter(|slot| {
                slot.peer() == &binding.peer
                    && slot.hardware_index() == binding.hardware_index
                    && storage.generations[index] == binding.generation
            })
            .ok_or(ApSecurityError::WrongPeer)?;
        debug_assert_eq!(slot.hardware_index(), binding.hardware_index);
        Ok(index)
    }
}

#[cfg(test)]
mod tests;
