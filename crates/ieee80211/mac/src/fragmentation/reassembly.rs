//! Bounded reassembly slots and affine admission across replay validation.
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

use crate::data::{EthernetFrameParts, LLC_SNAP_HEADER_LEN, rfc1042_ether_type};

use super::{
    CcmpPacketNumber, DataFragmentProtection, OpenDataFragment, OpenDataFragmentError,
    OpenDataFragmentIdentity,
};
use oer_base_time::{Duration, Instant};

/// Borrowed Ethernet view of one fully reassembled and LLC-validated MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenReassembledData<'payload> {
    identity: OpenDataFragmentIdentity,
    ether_type: u16,
    /// The MSDU after its validated LLC/SNAP header.
    payload: &'payload [u8],
}

impl<'payload> OpenReassembledData<'payload> {
    pub const fn identity(self) -> OpenDataFragmentIdentity {
        self.identity
    }

    pub fn ethernet_frame(self) -> EthernetFrameParts<'payload> {
        EthernetFrameParts {
            destination: self.identity.destination(),
            source: self.identity.source(),
            ether_type: self.ether_type,
            payload: self.payload,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenDataDefragmentation<R> {
    Buffered {
        expired: u8,
        evicted: Option<OpenDataFragmentIdentity>,
    },
    Duplicate {
        expired: u8,
    },
    Complete {
        expired: u8,
        value: R,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDataUnfragmentedAdmission {
    Admitted { expired: u8 },
    Duplicate { expired: u8 },
}

/// Side-effect-free fragment classification after timeout expiry.
///
/// An admitted token keeps the exact reassembly owner mutably borrowed while
/// an external CCMP owner prepares (but does not commit) its replay candidate.
/// This prevents a replay-invalid fragment from evicting or modifying a live
/// train before the caller has proved that its PN can advance.
pub enum OpenDataFragmentPreflight<'owner, 'frame, const CONTEXTS: usize, const CAPACITY: usize> {
    Duplicate { expired: u8 },
    Admitted(OpenDataFragmentAdmission<'owner, 'frame, CONTEXTS, CAPACITY>),
}

pub struct OpenDataFragmentAdmission<'owner, 'frame, const CONTEXTS: usize, const CAPACITY: usize> {
    owner: &'owner mut OpenDataDefragmenter<CONTEXTS, CAPACITY>,
    fragment: OpenDataFragment<'frame>,
    admission_epoch: u64,
    now: Instant,
    expired: u8,
}

impl<'frame, const CONTEXTS: usize, const CAPACITY: usize>
    OpenDataFragmentAdmission<'_, 'frame, CONTEXTS, CAPACITY>
{
    /// Durably retain this already preflighted fragment. For a final fragment,
    /// `complete` is the only edge at which the reassembled MSDU is borrowed.
    pub fn ingest<R>(
        self,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        self.owner.ingest_admitted(
            self.fragment,
            self.admission_epoch,
            self.now,
            self.expired,
            complete,
        )
    }
}

struct ReassemblySlot<const CAPACITY: usize> {
    identity: Option<OpenDataFragmentIdentity>,
    admission_epoch: u64,
    completed: bool,
    expected_fragment: u8,
    final_fragment: u8,
    started_at: Instant,
    length: usize,
    /// Each retained fragment by its number.
    fragments: [RetainedFragment; 16],
    bytes: [u8; CAPACITY],
}

/// Where one retained fragment's bytes lie in its slot, and its PN.
#[derive(Clone, Copy)]
struct RetainedFragment {
    start: usize,
    end: usize,
    packet_number: Option<CcmpPacketNumber>,
}

impl RetainedFragment {
    const NONE: Self = Self {
        start: 0,
        end: 0,
        packet_number: None,
    };
}

impl<const CAPACITY: usize> ReassemblySlot<CAPACITY> {
    const fn new() -> Self {
        Self {
            identity: None,
            admission_epoch: 0,
            completed: false,
            expected_fragment: 0,
            final_fragment: 0,
            started_at: Instant::EPOCH,
            length: 0,
            fragments: [RetainedFragment::NONE; 16],
            bytes: [0; CAPACITY],
        }
    }

    fn clear(&mut self) {
        self.identity = None;
        self.admission_epoch = 0;
        self.completed = false;
        self.expected_fragment = 0;
        self.final_fragment = 0;
        self.started_at = Instant::EPOCH;
        self.length = 0;
        self.fragments.fill(RetainedFragment::NONE);
    }

    /// The record of fragment `number`.
    #[expect(
        clippy::indexing_slicing,
        reason = "a fragment number is the four-bit field of Sequence Control, below 16"
    )]
    fn fragment(&self, number: u8) -> &RetainedFragment {
        &self.fragments[usize::from(number)]
    }

    /// The mutable record of fragment `number`.
    #[expect(
        clippy::indexing_slicing,
        reason = "a fragment number is the four-bit field of Sequence Control, below 16"
    )]
    fn fragment_mut(&mut self, number: u8) -> &mut RetainedFragment {
        &mut self.fragments[usize::from(number)]
    }

    /// The retained bytes of every fragment so far.
    #[expect(
        clippy::expect_used,
        reason = "append_fragment stores a length only after retaining that many bytes"
    )]
    fn retained(&self) -> &[u8] {
        self.bytes
            .get(..self.length)
            .expect("the retained length is within the slot")
    }

    fn is_active(&self) -> bool {
        self.identity.is_some() && !self.completed
    }

    fn verify_retry(&self, fragment: OpenDataFragment<'_>) -> Result<(), OpenDataFragmentError> {
        let final_fragment = if self.completed {
            self.final_fragment
        } else {
            self.expected_fragment.saturating_sub(1)
        };
        if fragment.fragment_number > final_fragment {
            return Err(OpenDataFragmentError::OutOfOrder {
                expected: self.expected_fragment,
                observed: fragment.fragment_number,
            });
        }
        let expected_more = fragment.fragment_number < final_fragment || !self.completed;
        if fragment.more_fragments != expected_more {
            return Err(OpenDataFragmentError::MoreFragmentsMismatch);
        }
        let retained = self.fragment(fragment.fragment_number);
        match (retained.packet_number, fragment.packet_number) {
            (Some(expected), Some(observed)) if expected != observed => {
                return Err(OpenDataFragmentError::RetryPacketNumberMismatch {
                    fragment_number: fragment.fragment_number,
                    expected,
                    observed,
                });
            }
            (None, None) | (Some(_), Some(_)) => {}
            _ => return Err(OpenDataFragmentError::IdentityMismatch),
        }
        if self.bytes.get(retained.start..retained.end) != Some(fragment.payload) {
            return Err(OpenDataFragmentError::RetryPayloadMismatch {
                fragment_number: fragment.fragment_number,
            });
        }
        Ok(())
    }
}

/// Fixed-capacity owner of incomplete Open or CCMP data MSDUs.
///
/// The oldest live context is evicted deterministically when all slots are
/// occupied. Callers must provide a monotonic runtime timestamp; synthetic
/// users without one fail closed rather than creating immortal retained data.
pub struct OpenDataDefragmenter<const CONTEXTS: usize, const CAPACITY: usize> {
    slots: [ReassemblySlot<CAPACITY>; CONTEXTS],
    timeout: Duration,
}

impl<const CONTEXTS: usize, const CAPACITY: usize> OpenDataDefragmenter<CONTEXTS, CAPACITY> {
    pub const fn new(timeout: Duration) -> Self {
        Self {
            slots: [const { ReassemblySlot::new() }; CONTEXTS],
            timeout,
        }
    }

    pub fn active_contexts(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_active()).count()
    }

    /// Revoke every incomplete MSDU and retry fingerprint at an epoch edge.
    pub fn clear(&mut self) -> usize {
        let active = self.active_contexts();
        for slot in &mut self.slots {
            slot.clear();
        }
        active
    }

    /// Revoke contexts owned by one transmitter at an AP peer-close edge.
    pub fn forget_transmitter(&mut self, transmitter: [u8; 6]) -> usize {
        let mut discarded = 0_usize;
        for slot in &mut self.slots {
            if slot
                .identity
                .is_some_and(|identity| identity.transmitter_address == transmitter)
            {
                // At most one per slot, so at most CONTEXTS.
                discarded = discarded.saturating_add(usize::from(slot.is_active()));
                slot.clear();
            }
        }
        discarded
    }

    /// Discard one exact train after an external replay transaction failed.
    pub fn discard(&mut self, identity: OpenDataFragmentIdentity, admission_epoch: u64) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            slot.identity == Some(identity) && slot.admission_epoch == admission_epoch
        }) else {
            return false;
        };
        slot.clear();
        true
    }

    /// Fence an ordinary Open MPDU against incomplete fragment state.
    pub fn admit_unfragmented(
        &mut self,
        identity: OpenDataFragmentIdentity,
        retry: bool,
        now: Option<Instant>,
    ) -> Result<OpenDataUnfragmentedAdmission, OpenDataFragmentError> {
        self.admit_unfragmented_in_epoch(identity, 0, retry, now)
    }

    /// Fence an ordinary Open MPDU in the caller's association epoch.
    pub fn admit_unfragmented_in_epoch(
        &mut self,
        identity: OpenDataFragmentIdentity,
        admission_epoch: u64,
        retry: bool,
        now: Option<Instant>,
    ) -> Result<OpenDataUnfragmentedAdmission, OpenDataFragmentError> {
        let relevant_active = self.slots.iter().any(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        });
        let relevant_completed = retry
            && self.slots.iter().any(|slot| {
                slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
            });
        let expired = match now {
            Some(now) => self.expire(now),
            None if relevant_active || relevant_completed => {
                return Err(OpenDataFragmentError::ClockUnavailable);
            }
            None => 0,
        };
        if self.slots.iter().any(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        }) {
            return Err(OpenDataFragmentError::MoreFragmentsMismatch);
        }
        if self.slots.iter().any(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        }) {
            return Err(OpenDataFragmentError::IdentityMismatch);
        }
        if retry
            && self.slots.iter().any(|slot| {
                slot.completed
                    && slot.identity == Some(identity)
                    && slot.admission_epoch == admission_epoch
            })
        {
            return match identity.protection {
                DataFragmentProtection::Open => {
                    Ok(OpenDataUnfragmentedAdmission::Duplicate { expired })
                }
                DataFragmentProtection::Ccmp { .. } => {
                    Err(OpenDataFragmentError::MoreFragmentsMismatch)
                }
            };
        }
        if retry
            && self.slots.iter().any(|slot| {
                slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
            })
        {
            return Err(OpenDataFragmentError::IdentityMismatch);
        }
        if !retry {
            // A fresh ordinary MPDU supersedes a retained fragment-completion
            // fingerprint in the same 12-bit sequence space. At high packet
            // rates the sequence number can legitimately wrap inside the
            // completion-history timeout; keeping the old address identity
            // would then reject the new ordinary MPDU's retry before the
            // role-local duplicate filter can recognize it.
            for slot in &mut self.slots {
                if slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
                {
                    slot.clear();
                }
            }
        }
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired })
    }

    /// Ingest one parsed fragment and synchronously borrow a completed MSDU.
    ///
    /// The completion callback must copy or consume its borrowed Ethernet
    /// view before returning. The backing bytes remain privately retained as
    /// an exact Retry fingerprint until timeout/reuse, so no borrowed payload
    /// can escape this ownership edge.
    pub fn ingest<R>(
        &mut self,
        fragment: OpenDataFragment<'_>,
        now: Instant,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        self.ingest_in_epoch(fragment, 0, now, complete)
    }

    /// Ingest within an external association/key admission epoch.
    ///
    /// AP integrations bind this value to the live association generation so
    /// an AID/address reuse cannot complete bytes retained by its predecessor.
    /// Single-peer STA epochs can use [`Self::ingest`], because the complete
    /// defragmenter is already destroyed or cleared on association teardown.
    pub fn ingest_in_epoch<R>(
        &mut self,
        fragment: OpenDataFragment<'_>,
        admission_epoch: u64,
        now: Instant,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        match self.preflight_in_epoch(fragment, admission_epoch, now)? {
            OpenDataFragmentPreflight::Duplicate { expired } => {
                Ok(OpenDataDefragmentation::Duplicate { expired })
            }
            OpenDataFragmentPreflight::Admitted(admission) => admission.ingest(complete),
        }
    }

    /// Classify a fragment before an external CCMP replay candidate is
    /// prepared. Only timeout expiry can mutate the owner at this edge.
    pub fn preflight_in_epoch<'owner, 'frame>(
        &'owner mut self,
        fragment: OpenDataFragment<'frame>,
        admission_epoch: u64,
        now: Instant,
    ) -> Result<OpenDataFragmentPreflight<'owner, 'frame, CONTEXTS, CAPACITY>, OpenDataFragmentError>
    {
        if CONTEXTS == 0 {
            return Err(OpenDataFragmentError::NoReassemblyContexts);
        }
        let expired = self.expire(now);
        let identity = fragment.identity;

        if fragment.retry {
            if let Some(slot) = self.slots.iter().find(|slot| {
                slot.completed
                    && slot.identity == Some(identity)
                    && slot.admission_epoch == admission_epoch
            }) {
                slot.verify_retry(fragment)?;
                return Ok(OpenDataFragmentPreflight::Duplicate { expired });
            }
            if self.slots.iter().any(|slot| {
                slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
            }) {
                return Err(OpenDataFragmentError::IdentityMismatch);
            }
        }

        let exact = self.slots.iter().find(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        });
        if fragment.fragment_number == 0
            && fragment.retry
            && let Some(slot) = exact
        {
            slot.verify_retry(fragment)?;
            return Ok(OpenDataFragmentPreflight::Duplicate { expired });
        }
        if fragment.fragment_number == 0
            && fragment.retry
            && self.slots.iter().any(|slot| {
                slot.is_active()
                    && slot
                        .identity
                        .is_some_and(|active| active.same_sequence_space(identity))
            })
        {
            return Err(OpenDataFragmentError::IdentityMismatch);
        }
        if fragment.fragment_number != 0 {
            let Some(slot) = exact else {
                if self.slots.iter().any(|slot| {
                    slot.is_active()
                        && slot
                            .identity
                            .is_some_and(|active| active.same_sequence_space(identity))
                }) {
                    return Err(OpenDataFragmentError::IdentityMismatch);
                }
                return Err(OpenDataFragmentError::Orphan {
                    fragment_number: fragment.fragment_number,
                });
            };
            if fragment.retry && fragment.fragment_number < slot.expected_fragment {
                slot.verify_retry(fragment)?;
                return Ok(OpenDataFragmentPreflight::Duplicate { expired });
            }
            if fragment.fragment_number != slot.expected_fragment {
                return Err(OpenDataFragmentError::OutOfOrder {
                    expected: slot.expected_fragment,
                    observed: fragment.fragment_number,
                });
            }
            if fragment.more_fragments && fragment.fragment_number == 15 {
                return Err(OpenDataFragmentError::TooManyFragments);
            }
        }
        Ok(OpenDataFragmentPreflight::Admitted(
            OpenDataFragmentAdmission {
                owner: self,
                fragment,
                admission_epoch,
                now,
                expired,
            },
        ))
    }

    fn ingest_admitted<R>(
        &mut self,
        fragment: OpenDataFragment<'_>,
        admission_epoch: u64,
        now: Instant,
        expired: u8,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        let identity = fragment.identity;
        let is_exact = |slot: &ReassemblySlot<CAPACITY>| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        };
        if fragment.fragment_number != 0 {
            let Some(slot) = self.slots.iter_mut().find(|slot| is_exact(slot)) else {
                return Err(OpenDataFragmentError::Orphan {
                    fragment_number: fragment.fragment_number,
                });
            };
            if let Err(error) = append_fragment(slot, fragment) {
                slot.clear();
                return Err(error);
            }
            if fragment.more_fragments {
                // Preflight admitted only the expected fragment and refused a
                // fragment 15 with More Fragments, so this is at most 15.
                slot.expected_fragment = slot.expected_fragment.wrapping_add(1);
                return Ok(OpenDataDefragmentation::Buffered {
                    expired,
                    evicted: None,
                });
            }

            let msdu = slot
                .retained()
                .split_first_chunk::<LLC_SNAP_HEADER_LEN>()
                .and_then(|(llc, payload)| Some((rfc1042_ether_type(*llc)?, payload)));
            let Some((ether_type, payload)) = msdu else {
                slot.clear();
                return Err(OpenDataFragmentError::InvalidLlcSnap);
            };
            let value = complete(OpenReassembledData {
                identity,
                ether_type,
                payload,
            });
            slot.completed = true;
            slot.final_fragment = fragment.fragment_number;
            slot.started_at = now;
            return Ok(OpenDataDefragmentation::Complete { expired, value });
        }

        let exact = self.slots.iter().position(is_exact);
        let same_sequence = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        });
        // The slot to reuse, and whether its train is reported as evicted.
        let (index, reports_eviction) = if let Some(index) = exact.or(same_sequence) {
            (index, true)
        } else if let Some(index) = self.slots.iter().position(|slot| slot.identity.is_none()) {
            (index, false)
        } else if let Some(index) = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.completed)
            .min_by_key(|(index, slot)| (slot.started_at, *index))
            .map(|(index, _)| index)
        {
            (index, false)
        } else {
            let Some(index) = self
                .slots
                .iter()
                .enumerate()
                .min_by_key(|(index, slot)| (slot.started_at, *index))
                .map(|(index, _)| index)
            else {
                return Err(OpenDataFragmentError::NoReassemblyContexts);
            };
            (index, true)
        };
        for (other_index, slot) in self.slots.iter_mut().enumerate() {
            if other_index != index
                && slot.completed
                && slot
                    .identity
                    .is_some_and(|entry| entry.same_sequence_space(identity))
            {
                slot.clear();
            }
        }
        let slot = self.slot_mut(index);
        let evicted = if reports_eviction {
            slot.identity
        } else {
            None
        };
        slot.clear();
        slot.identity = Some(identity);
        slot.admission_epoch = admission_epoch;
        slot.expected_fragment = 1;
        slot.started_at = now;
        if let Err(error) = append_fragment(slot, fragment) {
            slot.clear();
            return Err(error);
        }
        Ok(OpenDataDefragmentation::Buffered { expired, evicted })
    }

    /// The slot at `index`.
    #[expect(
        clippy::indexing_slicing,
        reason = "every caller passes a position it found in `slots`"
    )]
    fn slot_mut(&mut self, index: usize) -> &mut ReassemblySlot<CAPACITY> {
        &mut self.slots[index]
    }

    fn expire(&mut self, now: Instant) -> u8 {
        let mut expired = 0_u8;
        for slot in &mut self.slots {
            if slot.identity.is_some() && timestamp_expired(now, slot.started_at, self.timeout) {
                expired = expired.saturating_add(u8::from(slot.is_active()));
                slot.clear();
            }
        }
        expired
    }
}

fn append_fragment<const CAPACITY: usize>(
    slot: &mut ReassemblySlot<CAPACITY>,
    fragment: OpenDataFragment<'_>,
) -> Result<(), OpenDataFragmentError> {
    if let Some(previous_number) = fragment.fragment_number.checked_sub(1)
        && let (Some(previous), Some(observed)) = (
            slot.fragment(previous_number).packet_number,
            fragment.packet_number,
        )
        && observed <= previous
    {
        return Err(OpenDataFragmentError::PacketNumberNotIncreasing { previous, observed });
    }
    let payload = fragment.payload;
    let start = slot.length;
    let end = start
        .checked_add(payload.len())
        .ok_or(OpenDataFragmentError::ReassembledTooLarge { capacity: CAPACITY })?;
    let Some(destination) = slot.bytes.get_mut(start..end) else {
        return Err(OpenDataFragmentError::ReassembledTooLarge { capacity: CAPACITY });
    };
    destination.copy_from_slice(payload);
    *slot.fragment_mut(fragment.fragment_number) = RetainedFragment {
        start,
        end,
        packet_number: fragment.packet_number,
    };
    slot.length = end;
    Ok(())
}

const fn timestamp_expired(now: Instant, then: Instant, timeout: Duration) -> bool {
    let (now, then, timeout) = (now.as_micros(), then.as_micros(), timeout.as_micros());
    match now.checked_sub(then) {
        Some(elapsed) => timeout == 0 || elapsed >= timeout,
        None => true,
    }
}

#[cfg(test)]
mod fuzz_tests;
