//! AP binding for the role-neutral receive BlockAck reorder machines.
//!
//! The AP control and RX paths execute in one DATAPATH owner, so no mailbox is
//! needed. Out-of-order MPDUs still move into the same independent cold frame
//! storage used by connected STA; in-order frames remain zero-copy views of
//! the current DMA descriptor.

use crate::datapath::rx::reorder::{
    RX_REORDER_BACKING_SLOT_COUNT, RX_REORDER_CURRENT_SLOT, RX_REORDER_SLOT_DOMAIN, RxReorderFrame,
    RxReorderFrameStorage, RxReorderStorageError,
};
use oer_espressif_ieee80211_policy::block_ack::RX_REORDER_GAP_TIMEOUT_MICROS;
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    rx::{
        RxSegment,
        ampdu::{
            RX_BLOCK_ACK_BANK_COUNT, RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckIdentity,
            RxBlockAckMpduKey, RxBlockAckReorderBanks, RxBlockAckRequestKey, RxBlockAckSnapshot,
        },
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointRxReorderError {
    Sequence(RxAmpduError),
    Storage(RxReorderStorageError),
}

impl From<RxAmpduError> for AccessPointRxReorderError {
    fn from(error: RxAmpduError) -> Self {
        Self::Sequence(error)
    }
}

impl From<RxReorderStorageError> for AccessPointRxReorderError {
    fn from(error: RxReorderStorageError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccessPointRxReorderDrop {
    StorageExhausted,
    FrameTooLong,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct AccessPointRxReorderProgress {
    pub active: bool,
    pub buffered: bool,
    pub duplicate: bool,
    pub dropped: Option<AccessPointRxReorderDrop>,
    pub dispatched: u8,
    pub hardware_window_reset: Option<AccessPointRxWindowReset>,
}

/// One vendor-equivalent synchronization edge for a stale first A-MPDU on a
/// newly activated extra-SoftAP receive BlockAck bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AccessPointRxWindowReset {
    pub hardware_index: u8,
    pub starting_sequence: SequenceNumber,
}

struct PendingReleasedFrame<'storage, const CAPACITY: usize> {
    frame: RxReorderFrame<'storage, CAPACITY, RX_REORDER_BACKING_SLOT_COUNT>,
    identity: RxBlockAckIdentity,
}

/// Static AP reorder arena. The frame bytes belong to the separate cold
/// storage; this value owns only their affine leases and sequence state.
pub struct AccessPointRxReorder<'storage, const CAPACITY: usize> {
    banks: RxBlockAckReorderBanks<RX_REORDER_SLOT_DOMAIN>,
    pending_hardware_window_reset: [bool; RX_BLOCK_ACK_BANK_COUNT],
    deadlines: [Option<oer_time::Instant>; RX_BLOCK_ACK_BANK_COUNT],
    retained: [Option<RxReorderFrame<'storage, CAPACITY, RX_REORDER_BACKING_SLOT_COUNT>>;
        RX_REORDER_BACKING_SLOT_COUNT],
    pending_released:
        [Option<PendingReleasedFrame<'storage, CAPACITY>>; RX_REORDER_BACKING_SLOT_COUNT],
    pending_released_head: usize,
    pending_released_count: usize,
}

impl<'storage, const CAPACITY: usize> AccessPointRxReorder<'storage, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            banks: RxBlockAckReorderBanks::new(),
            pending_hardware_window_reset: [false; RX_BLOCK_ACK_BANK_COUNT],
            deadlines: [None; RX_BLOCK_ACK_BANK_COUNT],
            retained: [const { None }; RX_REORDER_BACKING_SLOT_COUNT],
            pending_released: [const { None }; RX_REORDER_BACKING_SLOT_COUNT],
            pending_released_head: 0,
            pending_released_count: 0,
        }
    }

    pub(super) fn start(
        &mut self,
        agreement: RxBlockAckSnapshot,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> Result<(), AccessPointRxReorderError> {
        let bank = usize::from(agreement.hardware_index);
        let replaced = self.banks.identity(bank);
        let released = self.banks.start(agreement)?;
        // SOURCE: complete `ht_recv_action_ba_addba_request` sets agreement
        // flag 0x40 after publishing the hardware entry. Complete
        // `libnet80211[ieee80211_ht.o]::ieee80211_ampdu_reorder` consumes that flag when it observes the
        // first physical A-MPDU. It reloads the hardware entry only when that
        // MPDU is behind the current software sequence frontier.
        self.pending_hardware_window_reset[bank] = true;
        self.deadlines[bank] = None;
        if let Some(release) = released {
            self.dispatch_retained_release(
                release,
                replaced.expect("a replaced reorder bank owns one identity"),
                &mut dispatch,
            );
        }
        Ok(())
    }

    pub(super) fn stop(
        &mut self,
        identity: RxBlockAckIdentity,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> bool {
        let bank = usize::from(identity.hardware_index);
        let Some(release) = self.banks.stop(identity) else {
            return false;
        };
        self.pending_hardware_window_reset[bank] = false;
        self.deadlines[bank] = None;
        self.dispatch_retained_release(release, identity, &mut dispatch);
        true
    }

    pub(super) fn stop_discard(&mut self, identity: RxBlockAckIdentity) -> u8 {
        let mut discarded = self.discard_pending_releases(identity);
        let bank = usize::from(identity.hardware_index);
        let Some(release) = self.banks.stop(identity) else {
            return discarded;
        };
        self.pending_hardware_window_reset[bank] = false;
        self.deadlines[bank] = None;
        for released in release.iter() {
            if self.retained[usize::from(released.slot)].take().is_some() {
                discarded = discarded.saturating_add(1);
            }
        }
        discarded
    }

    pub(super) fn ingest(
        &mut self,
        storage: &'storage RxReorderFrameStorage<CAPACITY, RX_REORDER_BACKING_SLOT_COUNT>,
        segment: RxSegment<'_>,
        key: RxBlockAckMpduKey,
        ampdu_baseband_format: Option<u8>,
        now: oer_time::Instant,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> Result<AccessPointRxReorderProgress, AccessPointRxReorderError> {
        let Some(bank) = self
            .banks
            .find(MacInterface::AccessPoint, key.peer, key.tid)
        else {
            dispatch(segment);
            return Ok(AccessPointRxReorderProgress {
                dispatched: 1,
                ..Default::default()
            });
        };
        let identity = RxBlockAckIdentity {
            hardware_index: bank as u8,
            interface: MacInterface::AccessPoint,
            peer: key.peer,
            tid: key.tid,
        };
        let mut hardware_window_reset = None;
        let mut resync_dispatched = 0_u8;
        if let Some(baseband_format) = ampdu_baseband_format
            && self.pending_hardware_window_reset[bank]
        {
            self.pending_hardware_window_reset[bank] = false;
            let resync = self
                .banks
                .state_mut(bank)
                .expect("one bank identity owns one reorder state")
                .resynchronize_stale_initial_ampdu(key.sequence, baseband_format > 3);
            if let Some((release, starting_sequence)) = resync {
                resync_dispatched =
                    self.dispatch_retained_release(release, identity, &mut dispatch);
                self.deadlines[bank] = None;
                hardware_window_reset = Some(AccessPointRxWindowReset {
                    hardware_index: bank as u8,
                    starting_sequence,
                });
            }
        }
        // Match connected-station RX for the common case: an in-order MPDU
        // with no retained successor does not need the generic release-list
        // owner graph. The state transition is complete before dispatch and
        // the current staging owner remains borrowed throughout. Gaps,
        // duplicates, window advances and a frontier with a retained
        // successor still fall through to the full reorder path below.
        let immediate = RxAmpduMpdu {
            sequence: key.sequence,
            slot: RX_REORDER_CURRENT_SLOT as u8,
        };
        if self
            .banks
            .state_mut(bank)
            .expect("one bank identity owns one reorder state")
            .try_ingest_immediate(immediate)?
            .is_some()
        {
            self.update_deadline(bank, now);
            dispatch(segment);
            return Ok(AccessPointRxReorderProgress {
                active: true,
                dispatched: resync_dispatched.saturating_add(1),
                hardware_window_reset,
                ..Default::default()
            });
        }
        let retain = self
            .banks
            .state(bank)
            .expect("one bank identity owns one reorder state")
            .retains_on_ingest(key.sequence)?;
        let retained = if retain {
            let reservation = match storage.try_reserve() {
                Ok(reservation) => reservation,
                Err(RxReorderStorageError::Exhausted) => {
                    return Ok(AccessPointRxReorderProgress {
                        active: true,
                        dropped: Some(AccessPointRxReorderDrop::StorageExhausted),
                        ..Default::default()
                    });
                }
                Err(error) => return Err(error.into()),
            };
            match reservation.copy_from(segment) {
                Ok(frame) => Some(frame),
                Err((RxReorderStorageError::TooLong(_), _reservation)) => {
                    return Ok(AccessPointRxReorderProgress {
                        active: true,
                        dropped: Some(AccessPointRxReorderDrop::FrameTooLong),
                        ..Default::default()
                    });
                }
                Err((error, _reservation)) => return Err(error.into()),
            }
        } else {
            None
        };
        let slot = retained
            .as_ref()
            .map_or(RX_REORDER_CURRENT_SLOT, RxReorderFrame::slot);
        let release = self
            .banks
            .state_mut(bank)
            .expect("one bank identity owns one reorder state")
            .ingest(RxAmpduMpdu {
                sequence: key.sequence,
                slot: slot as u8,
            })?;
        self.update_deadline(bank, now);

        let mut progress = AccessPointRxReorderProgress {
            active: true,
            buffered: release.buffered,
            duplicate: release.rejected.is_some(),
            dropped: None,
            dispatched: resync_dispatched,
            hardware_window_reset,
        };
        if let Some(retained) = retained {
            debug_assert!(self.retained[slot].is_none());
            self.retained[slot] = Some(retained);
            // A frame predicted to require backing can still be released by
            // this same ingest when a window advance closes a complete run.
            // The release token names the retained slot, so keep that exact
            // owner in the slot domain until dispatch consumes it instead of
            // deriving ownership from the final `buffered` state.
            progress.dispatched = progress
                .dispatched
                .saturating_add(self.dispatch_retained_release(release, identity, &mut dispatch));
        } else {
            progress.dispatched =
                progress
                    .dispatched
                    .saturating_add(self.dispatch_release_with_current(
                        release,
                        identity,
                        RX_REORDER_CURRENT_SLOT,
                        Some(segment),
                        &mut dispatch,
                    ));
        }
        Ok(progress)
    }

    /// Consume only the common in-order current-buffer transition.
    ///
    /// `Ok(None)` is strictly non-mutating and leaves the caller's staged
    /// owner available to the complete reorder path. A newly activated AP BA
    /// bank also falls back until the first physical A-MPDU has performed its
    /// vendor-equivalent hardware/software frontier synchronization.
    pub(super) fn try_ingest_immediate(
        &mut self,
        key: RxBlockAckMpduKey,
        now: oer_time::Instant,
    ) -> Result<Option<AccessPointRxReorderProgress>, AccessPointRxReorderError> {
        let Some(bank) = self
            .banks
            .find(MacInterface::AccessPoint, key.peer, key.tid)
        else {
            return Ok(Some(AccessPointRxReorderProgress {
                dispatched: 1,
                ..Default::default()
            }));
        };
        if self.pending_hardware_window_reset[bank] {
            return Ok(None);
        }
        let immediate = RxAmpduMpdu {
            sequence: key.sequence,
            slot: RX_REORDER_CURRENT_SLOT as u8,
        };
        let admitted = self
            .banks
            .state_mut(bank)
            .expect("one bank identity owns one reorder state")
            .try_ingest_immediate(immediate)?;
        let Some(_) = admitted else {
            return Ok(None);
        };
        self.update_deadline(bank, now);
        Ok(Some(AccessPointRxReorderProgress {
            active: true,
            dispatched: 1,
            ..Default::default()
        }))
    }

    /// Release at most one due gap per finite DATAPATH turn.
    pub(super) fn expire_due(
        &mut self,
        now: oer_time::Instant,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> u8 {
        let Some(bank) = self
            .deadlines
            .iter()
            .position(|deadline| deadline.is_some_and(|deadline| deadline <= now))
        else {
            return 0;
        };
        self.deadlines[bank] = None;
        let identity = self
            .banks
            .identity(bank)
            .expect("a live gap deadline owns one identity");
        let release = self
            .banks
            .state_mut(bank)
            .expect("a live gap deadline owns one reorder state")
            .expire_gap();
        self.update_deadline(bank, now);
        self.dispatch_retained_release(release, identity, &mut dispatch)
    }

    /// Move a client's receive window to a BlockAckReq's starting sequence.
    ///
    /// The client sends one when it gives up on MPDUs it never got
    /// acknowledged: `missing` counts the sequence numbers it gave up on, as
    /// this access point never received them.
    ///
    /// Frames released by the move join the ordered pending-release queue,
    /// which [`Self::dispatch_pending`] drains before any newer MPDU. Returns
    /// how many frames were released and how many sequence numbers the
    /// window moved past without an MPDU, or `None` when the peer has no
    /// agreement for the TID or the sequence is at or behind the window
    /// start. The hardware window is not touched, as the vendor's
    /// `ieee80211_process_bar_info` moves only the software window.
    pub(super) fn move_window(
        &mut self,
        request: RxBlockAckRequestKey,
        now: oer_time::Instant,
    ) -> Option<WindowMove> {
        let bank = self
            .banks
            .find(MacInterface::AccessPoint, request.peer, request.tid)?;
        let identity = self
            .banks
            .identity(bank)
            .expect("a found reorder bank owns one identity");
        let release = self
            .banks
            .state_mut(bank)
            .expect("a found reorder bank owns one state")
            .move_window_to(request.starting_sequence)?;
        self.update_deadline(bank, now);
        let mut released = 0_u8;
        for frame in release.iter() {
            let frame = self.retained[usize::from(frame.slot)]
                .take()
                .expect("a released reorder slot owns its retained backing");
            self.push_pending_release(PendingReleasedFrame { frame, identity });
            released = released.saturating_add(1);
        }
        Some(WindowMove {
            released,
            missing: release.missing,
        })
    }

    /// Publish at most one frame already released from sequence ownership.
    pub(super) fn dispatch_pending(&mut self, mut dispatch: impl FnMut(RxSegment<'_>)) -> bool {
        let Some(pending) = self.pop_pending_release() else {
            return false;
        };
        let frame = pending.frame;
        let view = frame.segment();
        dispatch(view.as_segment());
        drop(view);
        drop(frame);
        true
    }

    pub(super) const fn has_pending_release(&self) -> bool {
        self.pending_released_count != 0
    }

    /// Ordered software work visible to the outer RX scheduler without a new
    /// hardware completion. A released frame is ready immediately; a retained
    /// gap becomes ready at its absolute age deadline.
    pub(super) fn work_due(&self, now: oer_time::Instant) -> bool {
        self.has_pending_release() || self.next_deadline().is_some_and(|deadline| deadline <= now)
    }

    pub(super) fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.deadlines.iter().copied().flatten().min()
    }

    /// Whether a hardware-rejected public MPDU identity is already outside
    /// the software agreement frontier. This does not interpret the private
    /// hardware `rx_state`; it proves independently that publishing this
    /// sequence would be a duplicate or stale delivery.
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn is_duplicate_or_stale(&self, key: RxBlockAckMpduKey) -> bool {
        let Some(bank) = self
            .banks
            .find(MacInterface::AccessPoint, key.peer, key.tid)
        else {
            return false;
        };
        let state = self
            .banks
            .state(bank)
            .expect("active AP reorder identity owns one state");
        if key.sequence == state.next_sequence() {
            return false;
        }
        matches!(
            state.retains_on_ingest(key.sequence),
            Ok(false) | Err(RxAmpduError::DuplicateSequence(_))
        )
    }

    pub(super) fn discard_all(&mut self) -> u8 {
        let mut discarded = 0_u8;
        for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
            self.deadlines[bank] = None;
            self.pending_hardware_window_reset[bank] = false;
            let _ = self.banks.stop_bank(bank);
        }
        for retained in &mut self.retained {
            if retained.take().is_some() {
                discarded = discarded.saturating_add(1);
            }
        }
        while let Some(pending) = self.pop_pending_release() {
            drop(pending);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }

    fn update_deadline(&mut self, bank: usize, now: oer_time::Instant) {
        if self
            .banks
            .state(bank)
            .is_some_and(|state| state.occupied() != 0)
        {
            self.deadlines[bank].get_or_insert(now.saturating_add(
                oer_time::Duration::from_micros(RX_REORDER_GAP_TIMEOUT_MICROS),
            ));
        } else {
            self.deadlines[bank] = None;
        }
    }

    #[inline(always)]
    fn dispatch_retained_release(
        &mut self,
        release: RxAmpduRelease,
        identity: RxBlockAckIdentity,
        dispatch: &mut impl FnMut(RxSegment<'_>),
    ) -> u8 {
        self.dispatch_release_with_current(release, identity, usize::MAX, None, dispatch)
    }

    #[inline(always)]
    fn dispatch_release_with_current(
        &mut self,
        release: RxAmpduRelease,
        identity: RxBlockAckIdentity,
        current_slot: usize,
        current: Option<RxSegment<'_>>,
        dispatch: &mut impl FnMut(RxSegment<'_>),
    ) -> u8 {
        let mut current = current;
        let releases_current = release
            .iter()
            .any(|released| usize::from(released.slot) == current_slot);
        let mut dispatched = false;
        for (position, released) in release.iter().enumerate() {
            let slot = usize::from(released.slot);
            if slot == current_slot {
                debug_assert_eq!(position, 0, "current gap closer must release first");
                dispatch(
                    current
                        .take()
                        .expect("current AP reorder release is unique"),
                );
                dispatched = true;
            } else if !releases_current && !dispatched {
                let frame = self.retained[slot]
                    .take()
                    .unwrap_or_else(|| {
                        panic!(
                            "AP direct release lost backing slot={slot} current={current_slot} pending={} release={release:?}",
                            self.pending_released_count,
                        )
                    });
                let view = frame.segment();
                dispatch(view.as_segment());
                drop(view);
                drop(frame);
                dispatched = true;
            } else {
                let frame = self.retained[slot]
                    .take()
                    .unwrap_or_else(|| {
                        panic!(
                            "AP queued release lost backing slot={slot} current={current_slot} pending={} release={release:?}",
                            self.pending_released_count,
                        )
                    });
                self.push_pending_release(PendingReleasedFrame { frame, identity });
            }
        }
        if release.rejected.is_some() {
            let _ = current.take();
        }
        debug_assert!(current.is_none());
        u8::from(dispatched)
    }

    fn push_pending_release(&mut self, pending: PendingReleasedFrame<'storage, CAPACITY>) {
        assert!(
            self.pending_released_count < self.pending_released.len(),
            "AP released-frame queue cannot exceed retained backing"
        );
        let tail = (self.pending_released_head + self.pending_released_count)
            % self.pending_released.len();
        debug_assert!(self.pending_released[tail].is_none());
        self.pending_released[tail] = Some(pending);
        self.pending_released_count += 1;
    }

    fn pop_pending_release(&mut self) -> Option<PendingReleasedFrame<'storage, CAPACITY>> {
        if self.pending_released_count == 0 {
            return None;
        }
        let pending = self.pending_released[self.pending_released_head]
            .take()
            .expect("non-empty AP release queue owns its head");
        self.pending_released_head = (self.pending_released_head + 1) % self.pending_released.len();
        self.pending_released_count -= 1;
        Some(pending)
    }

    fn discard_pending_releases(&mut self, identity: RxBlockAckIdentity) -> u8 {
        let count = self.pending_released_count;
        let mut discarded = 0_u8;
        for _ in 0..count {
            let pending = self
                .pop_pending_release()
                .expect("snapshotted AP release count remains exact");
            if pending.identity == identity {
                drop(pending);
                discarded = discarded.saturating_add(1);
            } else {
                self.push_pending_release(pending);
            }
        }
        discarded
    }
}

impl<const CAPACITY: usize> Default for AccessPointRxReorder<'_, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// What one BlockAckReq's window move did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WindowMove {
    /// Retained MPDUs it released.
    pub(super) released: u8,
    /// Sequence numbers it moved past without an MPDU.
    pub(super) missing: u16,
}

#[cfg(test)]
mod tests;
