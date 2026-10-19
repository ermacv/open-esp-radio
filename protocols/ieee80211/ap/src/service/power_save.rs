//! Power-save admission, TIM counts and affine release completion.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;

impl<'peers, S: SppScope> AccessPointService<'peers, S> {
    /// Decide ownership of a newly arrived downlink unicast frame.
    ///
    /// The service never stores the frame itself. A `Buffer` result requires
    /// the caller to retain the frame first and only then call
    /// [`Self::commit_buffered_unicast`].
    pub fn admit_downlink(&self, peer: [u8; 6]) -> Result<ApDownlinkAdmission, ApServiceError> {
        let peer = self.checked_peer(peer)?;
        if peer.phase != ApPeerPhase::Authorized {
            return Err(ApServiceError::WrongPeerPhase);
        }
        let disposition = match peer.power_state {
            ApPeerPowerState::Active => ApDownlinkDisposition::TransmitNow,
            ApPeerPowerState::Sleeping => ApDownlinkDisposition::Buffer,
        };
        Ok(ApDownlinkAdmission {
            identity: peer.association_identity(),
            disposition,
        })
    }

    /// Commit one frame already retained by the caller's per-peer queue.
    pub fn commit_buffered_unicast(
        &mut self,
        identity: ApAssociationIdentity,
    ) -> Result<u16, ApServiceError> {
        let buffered = {
            let peer = self
                .bound_association_mut(identity)
                .ok_or(ApServiceError::UnknownPeer)?;
            if peer.phase != ApPeerPhase::Authorized
                || peer.power_state != ApPeerPowerState::Sleeping
            {
                return Err(ApServiceError::WrongPeerPhase);
            }
            peer.buffered_unicast_frames = peer
                .buffered_unicast_frames
                .checked_add(1)
                .ok_or(ApServiceError::BufferedTrafficOverflow)?;
            peer.buffered_unicast_frames
        };
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(buffered)
    }

    /// Reserve one buffered unicast frame for an awake peer or a PS-Poll.
    ///
    /// Forgetting the returned token intentionally leaves the peer blocked:
    /// a second dequeue cannot overtake an unaccounted first frame.
    pub fn begin_buffered_unicast_release(
        &mut self,
        identity: ApAssociationIdentity,
    ) -> Result<Option<ApBufferedUnicastRelease>, ApServiceError> {
        let token = {
            let peer = self
                .bound_association_mut(identity)
                .ok_or(ApServiceError::UnknownPeer)?;
            if peer.phase != ApPeerPhase::Authorized {
                return Err(ApServiceError::WrongPeerPhase);
            }
            if peer.buffered_release_in_flight {
                return Err(ApServiceError::BufferedReleaseInFlight);
            }
            if peer.buffered_unicast_frames == 0 {
                return Ok(None);
            }
            peer.buffered_release_in_flight = true;
            ApBufferedUnicastRelease {
                identity,
                more_data: peer.buffered_unicast_frames > 1,
            }
        };
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(Some(token))
    }

    /// Resolve one reserved queue release after the caller either transmitted
    /// the exact retained frame or returned it to the same queue.
    pub fn complete_buffered_unicast_release(
        &mut self,
        release: ApBufferedUnicastRelease,
        delivered: bool,
    ) -> Result<u16, ApServiceError> {
        let remaining = {
            let peer = self
                .bound_association_mut(release.identity)
                .ok_or(ApServiceError::AssociationIdMismatch)?;
            // The release is affine and this peer permits only one release
            // in flight. Association identity fences slot reuse, so a second
            // serial number would duplicate those two ownership invariants.
            if !peer.buffered_release_in_flight {
                return Err(ApServiceError::StaleBufferedRelease);
            }
            if delivered {
                peer.buffered_unicast_frames = peer
                    .buffered_unicast_frames
                    .checked_sub(1)
                    .ok_or(ApServiceError::NoBufferedTraffic)?;
            }
            peer.buffered_release_in_flight = false;
            peer.buffered_unicast_frames
        };
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(remaining)
    }

    /// Apply one parsed peer PM edge after the caller has validated that the
    /// frame belongs to this AP. PS-Poll reserves, but does not consume, one
    /// caller-owned buffered frame.
    pub fn observe_power_save(
        &mut self,
        observation: ApPowerSaveObservation,
        now: oer_time::Instant,
    ) -> Result<ApPowerSaveAction, ApServiceError> {
        match observation {
            ApPowerSaveObservation::Sleeping { peer } | ApPowerSaveObservation::Active { peer } => {
                let requested = if matches!(observation, ApPowerSaveObservation::Sleeping { .. }) {
                    ApPeerPowerState::Sleeping
                } else {
                    ApPeerPowerState::Active
                };
                let binding = self.bind_peer(peer).ok_or(ApServiceError::UnknownPeer)?;
                self.observe_bound_power_state(binding, requested, now)
            }
            ApPowerSaveObservation::PsPoll {
                peer,
                association_id,
            } => {
                let inactive_timeout = self.inactive_timeout.duration();
                let release_already_pending = {
                    let existing = self.checked_peer_mut(peer)?;
                    if existing.phase != ApPeerPhase::Authorized
                        || existing.power_state != ApPeerPowerState::Sleeping
                    {
                        return Err(ApServiceError::WrongPeerPhase);
                    }
                    if existing.association_id != association_id {
                        return Err(ApServiceError::AssociationIdMismatch);
                    }
                    existing.last_activity = now;
                    existing.deadline = now.saturating_add(inactive_timeout);
                    existing.buffered_release_in_flight
                };
                // A retried PS-Poll may arrive while the exact oldest frame is
                // already reserved or crossing TX. It is idempotent: never
                // reserve a second frame and never turn a valid control retry
                // into a terminal protocol error.
                if release_already_pending {
                    return Ok(ApPowerSaveAction::None);
                }
                let identity = self.checked_peer(peer)?.association_identity();
                Ok(match self.begin_buffered_unicast_release(identity)? {
                    Some(release) => ApPowerSaveAction::ReleaseOne(release),
                    None => ApPowerSaveAction::None,
                })
            }
        }
    }

    /// Apply one admitted PM state through a generation-bound O(1) peer
    /// identity.
    ///
    /// The data dispatcher has already resolved this binding for controlled
    /// port and key admission. Reusing it avoids a second scan of the AP peer
    /// table for every received data MPDU while preserving slot-reuse fencing.
    pub fn observe_bound_power_state(
        &mut self,
        binding: ApPeerBinding,
        requested: ApPeerPowerState,
        now: oer_time::Instant,
    ) -> Result<ApPowerSaveAction, ApServiceError> {
        let inactive_timeout = self.inactive_timeout.duration();
        let (peer, changed, buffered_frames) = {
            let existing = self
                .bound_peer_mut(binding)
                .ok_or(ApServiceError::UnknownPeer)?;
            if existing.phase != ApPeerPhase::Authorized {
                return Err(ApServiceError::WrongPeerPhase);
            }
            let changed = existing.power_state != requested;
            existing.power_state = requested;
            existing.last_activity = now;
            existing.deadline = now.saturating_add(inactive_timeout);
            (existing.address, changed, existing.buffered_unicast_frames)
        };
        if !changed {
            return Ok(ApPowerSaveAction::None);
        }
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(ApPowerSaveAction::StateChanged {
            peer,
            state: requested,
            buffered_frames,
        })
    }

    /// Apply activity from the saturated admitted-data path without rewriting
    /// the peer deadline for every MPDU.
    ///
    /// A PM transition remains an immediate control-plane edge. When the PM
    /// state is unchanged, refreshing at half of the inactivity interval keeps
    /// the deadline at least half an interval in the future while avoiding
    /// shared peer-state writes on every received packet.
    pub fn observe_bound_data_power_state(
        &mut self,
        binding: ApPeerBinding,
        requested: ApPeerPowerState,
        now: oer_time::Instant,
    ) -> Result<ApPowerSaveAction, ApServiceError> {
        let inactive_timeout = self.inactive_timeout.duration();
        let refresh_margin = oer_time::Duration::from_micros(inactive_timeout.as_micros() / 2);
        let (peer, changed, buffered_frames) = {
            let existing = self
                .bound_peer_mut(binding)
                .ok_or(ApServiceError::UnknownPeer)?;
            if existing.phase != ApPeerPhase::Authorized {
                return Err(ApServiceError::WrongPeerPhase);
            }
            let changed = existing.power_state != requested;
            let refresh_due = existing.deadline <= now.saturating_add(refresh_margin);
            if changed || refresh_due {
                existing.power_state = requested;
                existing.last_activity = now;
                existing.deadline = now.saturating_add(inactive_timeout);
            }
            (existing.address, changed, existing.buffered_unicast_frames)
        };
        if !changed {
            return Ok(ApPowerSaveAction::None);
        }
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(ApPowerSaveAction::StateChanged {
            peer,
            state: requested,
            buffered_frames,
        })
    }

    /// Complete typed TIM bitmap for the public AP AID range 1..=15.
    /// Canonical Partial Virtual Bitmap compression is derived by the beacon
    /// owner only after every peer AID has passed capacity validation.
    pub fn unicast_tim_bitmap(
        &self,
    ) -> Result<TimVirtualBitmap<AP_TIM_VIRTUAL_BITMAP_OCTETS>, TimBitmapError> {
        let mut bitmap = TimVirtualBitmap::try_new()?;
        for peer in self.storage().peers.iter().flatten().filter(|peer| {
            peer.power_state == ApPeerPowerState::Sleeping && peer.buffered_unicast_frames != 0
        }) {
            let association_id = TimAssociationId::new(peer.association_id)?;
            bitmap.set(association_id, true)?;
        }
        Ok(bitmap)
    }

    pub const fn buffered_group_frames(&self) -> u16 {
        self.buffered_group_frames
    }

    pub const fn group_traffic_pending(&self) -> bool {
        self.buffered_group_frames != 0
    }

    /// Decide ownership of a newly arrived multicast/broadcast frame.
    ///
    /// Group traffic is retained whenever at least one authorized station has
    /// announced PM=1. The caller must retain the payload first and call
    /// [`Self::commit_buffered_group`] only after that ownership transfer
    /// succeeds.
    pub fn group_downlink_disposition(&self) -> ApDownlinkDisposition {
        // Once a DTIM queue exists, retain later group frames behind it even
        // if the last sleeping peer wakes before the advertised release. This
        // preserves caller-owned FIFO order and prevents a fresh multicast
        // frame from overtaking the DTIM-bound prefix.
        if self.buffered_group_frames != 0
            || self.storage().peers.iter().flatten().any(|peer| {
                peer.phase == ApPeerPhase::Authorized
                    && peer.power_state == ApPeerPowerState::Sleeping
            })
        {
            ApDownlinkDisposition::Buffer
        } else {
            ApDownlinkDisposition::TransmitNow
        }
    }

    /// Commit one multicast/broadcast frame already retained by the caller.
    pub fn commit_buffered_group(&mut self) -> Result<u16, ApServiceError> {
        self.buffered_group_frames = self
            .buffered_group_frames
            .checked_add(1)
            .ok_or(ApServiceError::BufferedTrafficOverflow)?;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(self.buffered_group_frames)
    }

    /// Reserve the oldest caller-owned group frame after a successful DTIM
    /// beacon publication advertised group traffic.
    ///
    /// The DTIM publication edge is intentionally owned by the caller. This
    /// service cannot infer it from a timer or from the current TIM phase.
    pub fn begin_buffered_group_release(
        &mut self,
    ) -> Result<Option<ApBufferedGroupRelease>, ApServiceError> {
        if self.buffered_group_release_in_flight {
            return Err(ApServiceError::BufferedReleaseInFlight);
        }
        if self.buffered_group_frames == 0 {
            return Ok(None);
        }
        self.buffered_group_release_generation = self
            .buffered_group_release_generation
            .checked_add(1)
            .ok_or(ApServiceError::BufferedTrafficOverflow)?;
        self.buffered_group_release_in_flight = true;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(Some(ApBufferedGroupRelease {
            generation: self.buffered_group_release_generation,
            more_data: self.buffered_group_frames > 1,
        }))
    }

    /// Resolve one affine group release after its exact retained payload
    /// reached terminal hardware publication or was restored to the queue.
    ///
    /// `delivered` means terminal publication success. Group-addressed MPDUs
    /// have no acknowledgement, so this API never manufactures ACK evidence.
    pub fn complete_buffered_group_release(
        &mut self,
        release: ApBufferedGroupRelease,
        delivered: bool,
    ) -> Result<u16, ApServiceError> {
        if !self.buffered_group_release_in_flight
            || release.generation != self.buffered_group_release_generation
        {
            return Err(ApServiceError::StaleBufferedRelease);
        }
        if delivered {
            self.buffered_group_frames = self
                .buffered_group_frames
                .checked_sub(1)
                .ok_or(ApServiceError::NoBufferedTraffic)?;
        }
        self.buffered_group_release_in_flight = false;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(self.buffered_group_frames)
    }

    /// Account one group frame only after its DTIM-scoped publication has
    /// reached a terminal success. Failed frames remain advertised.
    pub fn complete_buffered_group(&mut self, delivered: bool) -> Result<u16, ApServiceError> {
        if self.buffered_group_release_in_flight {
            return Err(ApServiceError::BufferedReleaseInFlight);
        }
        if delivered {
            self.buffered_group_frames = self
                .buffered_group_frames
                .checked_sub(1)
                .ok_or(ApServiceError::NoBufferedTraffic)?;
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        Ok(self.buffered_group_frames)
    }

    /// Clear the portable advertisement count at a caller-owned queue-drop
    /// boundary such as AP stop.
    ///
    /// The returned count tells the caller exactly how many retained payload
    /// owners it must drop. An in-flight affine release must be rolled back
    /// before this operation is legal.
    pub fn discard_buffered_groups(&mut self) -> Result<u16, ApServiceError> {
        if self.buffered_group_release_in_flight {
            return Err(ApServiceError::BufferedReleaseInFlight);
        }
        let discarded = self.buffered_group_frames;
        if discarded != 0 {
            self.buffered_group_frames = 0;
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        Ok(discarded)
    }
}
