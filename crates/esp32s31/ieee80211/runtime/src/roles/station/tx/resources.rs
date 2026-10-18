use super::*;

impl<
    'slot,
    'ampdu,
    B,
    P,
    E,
    T,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedTx<'slot, 'ampdu, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
where
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// Cancel the arena currently selected by `self.ampdu` without changing
    /// the logical transaction stored in `self.active`.
    ///
    /// Standby preparation temporarily swaps the secondary arena into this
    /// field while the primary aggregate remains hardware-owned. Clearing
    /// `active` from that nested operation loses the primary transaction and
    /// leaves its metadata outside the completion state machine.
    pub(super) fn cancel_current_reservation(&mut self) {
        if let Some(cookie) = self.cookie.take() {
            let _ = self.ampdu.active_mut().cancel(cookie);
        }
        self.release_frames();
    }

    pub(super) fn cancel_prepared(&mut self) {
        self.cancel_current_reservation();
        self.active = ConnectedTxActive::Idle;
    }

    pub(super) fn cancel_prepared_network(&mut self) -> Result<(), AggregateTxError> {
        self.standby_error = None;
        drop(self.deferred_network.take());
        if self.standby_prepared.take().is_none() {
            return Ok(());
        }
        let cookie = self
            .standby_cookie
            .take()
            .ok_or(AggregateTxError::MissingCookie)?;
        let standby = self
            .ampdu
            .standby_mut()
            .ok_or(AggregateTxError::InvalidPublicationState)?;
        standby.cancel(cookie)?;
        standby.release_free_backings()?;
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::StandbyCancelled);
        }
        Ok(())
    }

    pub(super) fn release_completed(&mut self) -> Result<(), AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        self.ampdu.active_mut().release_completed(cookie)?;
        self.cookie = None;
        self.release_frames();
        Ok(())
    }

    pub(super) fn release_frames(&mut self) {
        if self.ampdu.active_mut().release_free_backings().is_err() {
            self.ampdu.active_mut().forget_backings();
        }
    }

    pub(super) fn forget_frames(&mut self) {
        self.ampdu.active_mut().forget_backings();
    }

    pub(super) fn reset_required(
        &mut self,
        reason: AggregateTxResetReason,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if self.cancel_prepared_network().is_err()
            && let Some(standby) = self.ampdu.standby_mut()
        {
            standby.forget_backings();
        }
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        self.ampdu.active_mut().require_reset(cookie)?;
        self.forget_frames();
        Err(AggregateTxError::RadioResetRequired(reason))
    }
}

impl<
    B: MaterializedTxFrame,
    P,
    E,
    T,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> Drop for ConnectedTx<'_, '_, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    fn drop(&mut self) {
        if !self.ordinary.is_present() || !self.ampdu.is_present() {
            return;
        }
        if self.standby_prepared.is_some()
            && self.cancel_prepared_network().is_err()
            && let Some(standby) = self.ampdu.standby_mut()
        {
            standby.forget_backings();
        }
        match self.ampdu.active().state() {
            TxSlotState::Free => self.release_frames(),
            TxSlotState::Reserved => {
                if self
                    .cookie
                    .is_some_and(|cookie| self.ampdu.active_mut().cancel(cookie).is_ok())
                {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::Completed => {
                if self
                    .cookie
                    .is_some_and(|cookie| self.ampdu.active_mut().release_completed(cookie).is_ok())
                {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::HardwareOwned | TxSlotState::ResetRequired => self.forget_frames(),
        }
    }
}
