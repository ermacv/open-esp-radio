//! Power-save admission and release bridges to the portable AP owner.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> ApEngine<'storage> {
    pub fn admit_downlink(&self, peer: [u8; 6]) -> Result<ApDownlinkAdmission, ApEngineError> {
        Ok(self.service.admit_downlink(peer)?)
    }

    pub fn group_downlink_disposition(&self) -> ApDownlinkDisposition {
        self.service.group_downlink_disposition()
    }

    pub fn commit_buffered_unicast(
        &mut self,
        identity: ApAssociationIdentity,
    ) -> Result<u16, ApEngineError> {
        Ok(self.service.commit_buffered_unicast(identity)?)
    }

    pub fn begin_buffered_unicast_release(
        &mut self,
        identity: ApAssociationIdentity,
    ) -> Result<Option<ApBufferedUnicastRelease>, ApEngineError> {
        Ok(self.service.begin_buffered_unicast_release(identity)?)
    }

    pub fn association_is_current(&self, identity: ApAssociationIdentity) -> bool {
        self.service.association_is_current(identity)
    }

    pub fn association_status(&self, identity: ApAssociationIdentity) -> Option<ApPeerStatus> {
        self.service.bound_authorized_peer_status(identity)
    }

    pub fn complete_buffered_unicast_release(
        &mut self,
        release: ApBufferedUnicastRelease,
        delivered: bool,
    ) -> Result<u16, ApEngineError> {
        Ok(self
            .service
            .complete_buffered_unicast_release(release, delivered)?)
    }

    pub fn observe_power_save(
        &mut self,
        observation: ApPowerSaveObservation,
        now: oer_time::Instant,
    ) -> Result<ApPowerSaveAction, ApEngineError> {
        Ok(self.service.observe_power_save(observation, now)?)
    }

    /// Refresh the PM state of the peer admitted by the current RX binding.
    /// A control-plane revision or a different transmitter falls back to the
    /// general address-resolving path.
    pub fn observe_rx_peer_power_state(
        &mut self,
        peer: [u8; 6],
        state: ApPeerPowerState,
        now: oer_time::Instant,
    ) -> Result<ApPowerSaveAction, ApEngineError> {
        if let Some(binding) = self.rx_peer
            && binding.peer.address() == peer
            && binding.status_revision == self.service.status_revision()
        {
            return Ok(self
                .service
                .observe_bound_data_power_state(binding.peer, state, now)?);
        }
        let observation = match state {
            ApPeerPowerState::Active => ApPowerSaveObservation::Active { peer },
            ApPeerPowerState::Sleeping => ApPowerSaveObservation::Sleeping { peer },
        };
        self.observe_power_save(observation, now)
    }

    /// Parse and apply an AP power-save edge from one complete 802.11 MPDU.
    /// Non-PM frames are left to the ordinary receive classifier.
    pub fn observe_power_save_frame(
        &mut self,
        frame: &[u8],
        now: oer_time::Instant,
    ) -> Result<Option<ApPowerSaveAction>, ApEngineError> {
        let Some(observation) =
            observe_ap_power_save_for_access_point(frame, self.service.address())
        else {
            return Ok(None);
        };
        self.observe_power_save(observation, now).map(Some)
    }

    pub fn commit_buffered_group(&mut self) -> Result<u16, ApEngineError> {
        Ok(self.service.commit_buffered_group()?)
    }

    pub fn begin_buffered_group_release(
        &mut self,
    ) -> Result<Option<ApBufferedGroupRelease>, ApEngineError> {
        Ok(self.service.begin_buffered_group_release()?)
    }

    pub fn complete_buffered_group_release(
        &mut self,
        release: ApBufferedGroupRelease,
        delivered: bool,
    ) -> Result<u16, ApEngineError> {
        Ok(self
            .service
            .complete_buffered_group_release(release, delivered)?)
    }

    pub fn complete_buffered_group(&mut self, delivered: bool) -> Result<u16, ApEngineError> {
        Ok(self.service.complete_buffered_group(delivered)?)
    }

    pub fn discard_buffered_groups(&mut self) -> Result<u16, ApEngineError> {
        Ok(self.service.discard_buffered_groups()?)
    }
}
