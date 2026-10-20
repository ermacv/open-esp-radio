//! The CCMP replay check of one association's keys, shared by the station
//! and the access point: the receive frontiers of the pairwise key and of
//! each installed group key by its index, each with one lane for non-QoS
//! frames and one per TID ([`CcmpRxReplayState`]). A group key's frontiers
//! start from its receive sequence when it is installed and leave with its
//! removal. Only one installation of each wire ID is active; the station
//! retains other installations' frontiers and exchanges them at a backend
//! RX terminal without reinitializing their counters.
//!
//! The receive path checks a frame here once it is decrypted and verified,
//! and after the reorder of its TID, when the receiver has one: within a
//! lane the packet numbers then strictly increase.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use super::{
    CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane, CcmpRxReplayCandidate,
    CcmpRxReplayState, GroupKeyId,
};

/// Which key a received frame's destination selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpRxProtection {
    /// An individually addressed frame: the pairwise key.
    Pairwise,
    /// A group addressed frame: the group key.
    Group,
    /// A frame for neither.
    Other,
}

/// Why the replay check refused a frame or a key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpRxReplayError {
    /// The pairwise key's receive sequence is not a packet number.
    InvalidPairwiseReceiveSequence,
    /// The group key's receive sequence is not a packet number.
    InvalidGroupReceiveSequence,
    /// The frame is for neither key.
    ForeignDestination,
    /// A pairwise frame's CCMP header names a group key's index.
    UnexpectedKeyId {
        /// The index the header names.
        observed: CcmpKeyId,
    },
    /// A group frame's CCMP header names an index no installed group key
    /// has.
    UninstalledGroupKey(CcmpKeyId),
    /// The frame repeats or precedes a packet number of its lane.
    Replay(CcmpReplayError),
}

/// The receive frontiers of one association's pairwise key and installed
/// group keys.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpRxReplayEpoch {
    pairwise: CcmpRxReplayState,
    /// The frontiers of the group key of each index 0 to 3, while it is
    /// installed.
    groups: [Option<CcmpRxReplayState>; 4],
}

impl CcmpRxReplayEpoch {
    /// The frontiers of a new pairwise key and of the group key
    /// `group_key_id`, from their receive sequences.
    ///
    /// # Errors
    ///
    /// A receive sequence that is no packet number.
    pub fn new(
        pairwise_receive_sequence: [u8; 8],
        group_key_id: GroupKeyId,
        group_receive_sequence: [u8; 8],
    ) -> Result<Self, CcmpRxReplayError> {
        let mut epoch = Self::pairwise(pairwise_receive_sequence)?;
        epoch.install_group(group_key_id, group_receive_sequence)?;
        Ok(epoch)
    }

    /// Start an association's pairwise frontier before the first GTK terminal.
    /// No group frame is admitted until its actual installation succeeds.
    pub fn pairwise(receive_sequence: [u8; 8]) -> Result<Self, CcmpRxReplayError> {
        let pairwise = CcmpRxReplayState::from_receive_sequence(receive_sequence)
            .map_err(|_| CcmpRxReplayError::InvalidPairwiseReceiveSequence)?;
        Ok(Self {
            pairwise,
            groups: [None, None, None, None],
        })
    }

    /// Exchange the active installation of this wire ID at the backend's RX
    /// boundary. The caller retains the returned frontier with its old key;
    /// reselecting it exchanges that same owner back, never reinitializing RSC.
    #[must_use = "retain the old frontier if its installed key is retained"]
    pub fn exchange_group(
        &mut self,
        key_id: GroupKeyId,
        state: CcmpRxReplayState,
    ) -> Option<CcmpRxReplayState> {
        key_id.select_mut(&mut self.groups).replace(state)
    }

    /// Whether the group key of `key_id` is installed.
    pub const fn has_group(&self, key_id: GroupKeyId) -> bool {
        key_id.select(&self.groups).is_some()
    }

    /// The group key `key_id` is installed with its receive sequence: its
    /// frontiers start there. The key owner must compare material before
    /// installation: identical material keeps its existing epoch unchanged;
    /// new material starts from KeyRSC even if it reuses an ID with higher PN.
    ///
    /// # Errors
    ///
    /// A receive sequence that is no packet number; nothing changed.
    pub fn install_group(
        &mut self,
        key_id: GroupKeyId,
        receive_sequence: [u8; 8],
    ) -> Result<(), CcmpRxReplayError> {
        let group = CcmpRxReplayState::from_receive_sequence(receive_sequence)
            .map_err(|_| CcmpRxReplayError::InvalidGroupReceiveSequence)?;
        self.install_group_epoch(key_id, group);
        Ok(())
    }

    /// Publish a successor epoch previously checked from its KeyRSC. The
    /// key owner calls this at the installation boundary in the RX stream.
    pub fn install_group_epoch(&mut self, key_id: GroupKeyId, epoch: CcmpRxReplayState) {
        *key_id.select_mut(&mut self.groups) = Some(epoch);
    }

    /// The group key `key_id` is removed: its frames are refused from now
    /// on.
    pub fn remove_group(&mut self, key_id: GroupKeyId) {
        *key_id.select_mut(&mut self.groups) = None;
    }

    /// Prepare the check of a frame of `protection`, of the TID `tid` (none
    /// for a non-QoS frame), with the CCMP header `header`. The returned
    /// token borrows this exact key frontier exclusively through publication;
    /// it cannot be applied to another epoch or survive key removal.
    ///
    /// # Errors
    ///
    /// A foreign destination, a key index that does not match it or names
    /// no installed group key, or a replayed packet number.
    pub fn prepare(
        &mut self,
        protection: CcmpRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<CcmpRxReplayCandidate<'_>, CcmpRxReplayError> {
        let lane = match tid {
            Some(tid) => CcmpReplayLane::Tid(tid),
            None => CcmpReplayLane::NonQos,
        };
        match protection {
            CcmpRxProtection::Pairwise => {
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return Err(CcmpRxReplayError::UnexpectedKeyId {
                        observed: header.key_id(),
                    });
                }
                self.pairwise
                    .prepare(lane, header.packet_number())
                    .map_err(CcmpRxReplayError::Replay)
            }
            CcmpRxProtection::Group => {
                let observed = header.key_id();
                let Some(key_id) = GroupKeyId::new(observed.value()) else {
                    return Err(CcmpRxReplayError::UninstalledGroupKey(observed));
                };
                let Some(group) = key_id.select_mut(&mut self.groups) else {
                    return Err(CcmpRxReplayError::UninstalledGroupKey(observed));
                };
                group
                    .prepare(lane, header.packet_number())
                    .map_err(CcmpRxReplayError::Replay)
            }
            CcmpRxProtection::Other => Err(CcmpRxReplayError::ForeignDestination),
        }
    }

    /// Check a frame that is published at once: prepare and commit.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare`].
    pub fn check(
        &mut self,
        protection: CcmpRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<(), CcmpRxReplayError> {
        let candidate = self.prepare(protection, tid, header)?;
        candidate.commit();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
