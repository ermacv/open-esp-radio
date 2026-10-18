//! Power-save admission and affine unicast/group release transactions.
//! The network TX owner retains every lease until commit, rollback or discard.

use crate::diagnostics::aggregate_tx::NetworkTxRetentionDropReason;

use super::*;

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    pub(super) fn retain_power_save(
        &mut self,
        engine: &mut ApEngine<'_>,
        frame: N,
    ) -> Result<Option<(ApTxFlowKey, N)>, AccessPointDatapathError> {
        let unbound_key = ApTxFlowKey::unbound_from_ethernet(frame.as_slice());
        let Some(peer) = frame
            .as_slice()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Ok(Some((unbound_key, frame)));
        };
        if peer[0] & 1 != 0 {
            if engine.group_downlink_disposition() == ApDownlinkDisposition::TransmitNow {
                return Ok(Some((unbound_key, frame)));
            }
            let index = match self.buffered_group.push(frame, &mut self.frame_arena) {
                Ok(index) => index,
                Err(frame) => {
                    // Do not advertise a TIM entry for a dropped owner.
                    self.discard_retention(NetworkTxRetentionDropReason::GroupPowerSaveFull, frame);
                    return Ok(None);
                }
            };
            if let Err(error) = engine.commit_buffered_group() {
                self.buffered_group
                    .take_at(index)
                    .expect("the just-inserted AP group lease is still owned")
                    .complete(&mut self.frame_arena);
                return Err(AccessPointDatapathError::Control(
                    AccessPointControlError::from(error),
                ));
            }
            return Ok(None);
        }
        let admission = match engine.admit_downlink(peer) {
            Ok(admission) => admission,
            // Preserve the ordinary admission path for an unknown or
            // unauthorized destination so its existing rejection accounting
            // remains authoritative.
            Err(_) => return Ok(Some((unbound_key, frame))),
        };
        let identity = admission.identity();
        let key = ApTxFlowKey::associated(identity);
        if admission.disposition() == ApDownlinkDisposition::TransmitNow {
            return Ok(Some((key, frame)));
        }

        let index = match self
            .buffered_unicast
            .push(identity, frame, &mut self.frame_arena)
        {
            Ok(index) => index,
            Err(frame) => {
                self.discard_retention(NetworkTxRetentionDropReason::UnicastPowerSaveFull, frame);
                return Ok(None);
            }
        };
        if let Err(error) = engine.commit_buffered_unicast(identity) {
            self.buffered_unicast
                .take_at(index)
                .expect("the just-inserted AP power-save lease is still owned")
                .complete(&mut self.frame_arena);
            return Err(AccessPointDatapathError::Control(
                AccessPointControlError::from(error),
            ));
        }
        Ok(None)
    }

    /// Preserve a requested PS-Poll release and refresh awake readiness.
    /// An ordinary wake edge does not reserve a peer or remove a queue entry;
    /// its affine release is acquired only after common destination selection.
    pub(in super::super) fn refresh_power_save_demand<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        if self.prepared_buffered_release.is_some() || self.active_buffered_release.is_some() {
            return Ok(false);
        }

        if let Some(release) = control.take_pending_buffered_release() {
            let identity = release.identity();
            if let Some(index) = self.buffered_unicast.oldest_index_for(identity) {
                let buffered = self
                    .buffered_unicast
                    .take_at(index)
                    .expect("the PS-Poll release names one retained lease");
                self.prepared_buffered_release = Some(BufferedUnicastRelease {
                    buffered,
                    release,
                    cause: BufferedReleaseCause::PsPoll,
                });
                return Ok(true);
            }
            control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)
                .map_err(AccessPointControlError::from)
                .map_err(AccessPointDatapathError::Control)?;
        }

        self.refresh_awake_demand(control.mac.engine());
        Ok(self.awake_buffered_peer.is_some())
    }

    pub(in super::super) fn refresh_awake_demand(&mut self, engine: &ApEngine<'_>) {
        if self.buffered_unicast.len == 0 {
            self.awake_buffered_peer = None;
            return;
        }
        self.buffered_unicast
            .retain(&mut self.frame_arena, |identity| {
                engine.association_is_current(identity)
            });
        self.awake_buffered_peer =
            self.buffered_unicast
                .next_releasable_peer_after(self.last_destination, |identity| {
                    engine.association_status(identity).is_some_and(|status| {
                        status.power_state == ApPeerPowerState::Active
                            && !status.buffered_release_in_flight
                    })
                });
    }

    pub(super) fn select_awake_buffered_release(
        &mut self,
        engine: &mut ApEngine<'_>,
        identity: ApAssociationIdentity,
    ) -> Result<Option<BufferedUnicastRelease>, AccessPointDatapathError> {
        if !engine.association_status(identity).is_some_and(|status| {
            status.power_state == ApPeerPowerState::Active && !status.buffered_release_in_flight
        }) {
            return Ok(None);
        }
        let Some(index) = self.buffered_unicast.oldest_index_for(identity) else {
            return Ok(None);
        };
        let Some(release) = engine
            .begin_buffered_unicast_release(identity)
            .map_err(AccessPointControlError::from)
            .map_err(AccessPointDatapathError::Control)?
        else {
            return Ok(None);
        };
        let buffered = self
            .buffered_unicast
            .take_at(index)
            .expect("selected PS head retains its arena slot");
        self.last_destination = Some(identity.address());
        self.refresh_awake_demand(engine);
        Ok(Some(BufferedUnicastRelease {
            buffered,
            release,
            cause: BufferedReleaseCause::Awake,
        }))
    }

    pub(super) fn rollback_prepared_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let Some(prepared) = self.prepared_buffered_release.take() else {
            return Ok(());
        };
        self.finish_buffered_release(control.mac.engine_mut(), prepared, false)
    }

    pub(super) fn finish_buffered_release(
        &mut self,
        engine: &mut ApEngine<'_>,
        owned: BufferedUnicastRelease,
        delivered: bool,
    ) -> Result<(), AccessPointDatapathError> {
        if !engine.association_is_current(owned.release.identity()) {
            // A new generation owns its own counters. Only the stale software
            // owner is released; never apply its completion to the new peer.
            owned.buffered.complete(&mut self.frame_arena);
            self.refresh_awake_demand(engine);
            return Ok(());
        }
        let result = engine.complete_buffered_unicast_release(owned.release, delivered);
        if !delivered || result.is_err() {
            self.buffered_unicast.restore(owned.buffered);
        } else {
            owned.buffered.complete(&mut self.frame_arena);
        }
        self.refresh_awake_demand(engine);
        result
            .map(|_| ())
            .map_err(AccessPointControlError::from)
            .map_err(AccessPointDatapathError::Control)
    }

    pub(super) fn complete_active_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        delivered: bool,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let Some(active) = self.active_buffered_release.take() else {
            return Ok(());
        };
        self.finish_buffered_release(control.mac.engine_mut(), active, delivered)?;
        let _ = self.refresh_power_save_demand(control)?;
        Ok(())
    }

    pub(super) fn start_prepared_buffered_release<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_buffered_release
            .take()
            .expect("checked prepared AP power-save release");
        if !prepared.can_publish(control.mac.engine()) {
            if let Some(accounting) = self.airtime.as_mut() {
                accounting
                    .cancel_selection_for(ApTxFlowKey::associated(prepared.release.identity()))?;
            }
            self.finish_buffered_release(control.mac.engine_mut(), prepared, false)?;
            return Ok(WifiTxProgress::Complete);
        }
        if let Some(accounting) = self.airtime.as_mut()
            && let Err(error) = (|| {
                if prepared.cause == BufferedReleaseCause::PsPoll {
                    accounting.cancel_selection()?;
                }
                accounting.reserve_active(
                    control.mac.engine(),
                    ApTxFlowKey::associated(prepared.release.identity()),
                )
            })()
        {
            self.prepared_buffered_release = Some(prepared);
            return Err(error.into());
        }
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame(&self.frame_arena).as_slice(),
            prepared.release.more_data(),
        );
        let result = self.ordinary_airtime_result(result);
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_buffered_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Err(error)
            }
        }
    }

    /// Bind the exact queue prefix announced by a successfully transmitted
    /// DTIM beacon to the oldest caller-owned group lease.
    pub(super) fn stage_dtim_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        if let Some(advertised_frames) = control.take_pending_dtim_group_frames() {
            if self.dtim_group_release_remaining != 0
                || self.prepared_group_release.is_some()
                || self.active_group_release.is_some()
            {
                return Err(AccessPointDatapathError::Control(
                    AccessPointControlError::DtimGroupReleaseAlreadyPending,
                ));
            }
            self.dtim_group_release_remaining = advertised_frames;
        }
        if self.dtim_group_release_remaining == 0
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
        {
            return Ok(false);
        }

        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_group_release()
            .map_err(AccessPointControlError::from)
            .map_err(AccessPointDatapathError::Control)?
        else {
            self.dtim_group_release_remaining = 0;
            return Err(AccessPointDatapathError::Control(
                AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let Some(index) = self.buffered_group.oldest_index() else {
            let rollback = control
                .mac
                .engine_mut()
                .complete_buffered_group_release(release, false)
                .map_err(AccessPointControlError::from)
                .map_err(AccessPointDatapathError::Control);
            self.dtim_group_release_remaining = 0;
            rollback?;
            return Err(AccessPointDatapathError::Control(
                AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let buffered = self
            .buffered_group
            .take_at(index)
            .expect("the selected AP group lease remains retained");
        self.prepared_group_release = Some(BufferedGroupRelease { buffered, release });
        Ok(true)
    }

    fn rollback_prepared_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let Some(prepared) = self.prepared_group_release.take() else {
            self.dtim_group_release_remaining = 0;
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(prepared.release, false);
        self.buffered_group.restore(prepared.buffered);
        self.dtim_group_release_remaining = 0;
        result
            .map(|_| ())
            .map_err(AccessPointControlError::from)
            .map_err(AccessPointDatapathError::Control)
    }

    pub(super) fn complete_active_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        published: bool,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let Some(active) = self.active_group_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(active.release, published);
        if !published || result.is_err() {
            self.buffered_group.restore(active.buffered);
            self.dtim_group_release_remaining = 0;
        } else {
            active.buffered.complete(&mut self.frame_arena);
            self.dtim_group_release_remaining = self
                .dtim_group_release_remaining
                .checked_sub(1)
                .ok_or(AccessPointDatapathError::Control(
                    AccessPointControlError::GroupBufferOwnershipMismatch,
                ))?;
        }
        result
            .map(|_| ())
            .map_err(AccessPointControlError::from)
            .map_err(AccessPointDatapathError::Control)?;
        if self.dtim_group_release_remaining != 0 {
            let _ = self.stage_dtim_group_release(control)?;
        }
        Ok(())
    }

    pub(super) fn start_prepared_group_release<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_group_release
            .take()
            .expect("checked prepared AP DTIM group release");
        if let Some(accounting) = self.airtime.as_mut()
            && let Err(error) = (|| {
                accounting.cancel_selection()?;
                accounting.reserve_active(
                    control.mac.engine(),
                    ApTxFlowKey::unbound_from_ethernet(
                        prepared.buffered.frame(&self.frame_arena).as_slice(),
                    ),
                )
            })()
        {
            self.prepared_group_release = Some(prepared);
            return Err(error.into());
        }
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame(&self.frame_arena).as_slice(),
            prepared.release.more_data(),
        );
        let result = self.ordinary_airtime_result(result);
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_group_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                // The control owner returns Complete without publication when
                // no authorized receiver remains. Drop both the retained
                // leases and their TIM accounting instead of advertising an
                // undeliverable queue forever.
                self.discard_group_buffer(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                Err(error)
            }
        }
    }

    pub(in super::super) fn discard_group_buffer<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        if self.active_group_release.is_some() {
            return Err(AccessPointDatapathError::Control(
                AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        self.rollback_prepared_group_release(control)?;
        let _ = control.take_pending_dtim_group_frames();
        let portable = control
            .mac
            .engine_mut()
            .discard_buffered_groups()
            .map_err(AccessPointControlError::from)
            .map_err(AccessPointDatapathError::Control)?;
        let retained = self.buffered_group.clear(&mut self.frame_arena);
        self.dtim_group_release_remaining = 0;
        if usize::from(portable) != retained {
            return Err(AccessPointDatapathError::Control(
                AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        Ok(())
    }
}

impl<'observer, B, N, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    AccessPointPowerSaveNetworkTx<P, E, T, DMA_BUFFER_SIZE, TX_BUFFER_SIZE>
    for AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    fn refresh_power_save_demand(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, AccessPointDatapathError> {
        self.refresh_power_save_demand(control)
    }

    fn refresh_awake_demand(&mut self, engine: &ApEngine<'_>) {
        self.refresh_awake_demand(engine);
    }

    fn has_power_save_release(&self) -> bool {
        self.prepared_buffered_release.is_some()
            || self.awake_buffered_peer.is_some()
            || self.active_buffered_release.is_some()
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
    }

    fn discard_group_power_save(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), AccessPointDatapathError> {
        self.discard_group_buffer(control)
    }
}
