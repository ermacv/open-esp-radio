//! Bounded reassembly slots and affine admission across replay validation.

use crate::data::{EthernetFrameParts, LLC_SNAP_HEADER_LEN};

use super::{
    CcmpPacketNumber, DataFragmentProtection, OpenDataFragment, OpenDataFragmentError,
    OpenDataFragmentIdentity,
};
use oer_time::{Duration, Instant};

const RFC1042_LLC_SNAP_PREFIX: [u8; 6] = [0xaa, 0xaa, 0x03, 0, 0, 0];

/// Borrowed Ethernet view of one fully reassembled and LLC-validated MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenReassembledData<'payload> {
    identity: OpenDataFragmentIdentity,
    msdu: &'payload [u8],
}

impl<'payload> OpenReassembledData<'payload> {
    pub const fn identity(self) -> OpenDataFragmentIdentity {
        self.identity
    }

    pub fn ethernet_frame(self) -> EthernetFrameParts<'payload> {
        EthernetFrameParts {
            destination: self.identity.destination(),
            source: self.identity.source(),
            ether_type: u16::from_be_bytes([self.msdu[6], self.msdu[7]]),
            payload: &self.msdu[LLC_SNAP_HEADER_LEN..],
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
    fragment_offsets: [usize; 16],
    fragment_lengths: [usize; 16],
    packet_numbers: [Option<CcmpPacketNumber>; 16],
    bytes: [u8; CAPACITY],
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
            fragment_offsets: [0; 16],
            fragment_lengths: [0; 16],
            packet_numbers: [None; 16],
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
        self.fragment_offsets.fill(0);
        self.fragment_lengths.fill(0);
        self.packet_numbers.fill(None);
    }

    fn is_active(&self) -> bool {
        self.identity.is_some() && !self.completed
    }

    fn verify_retry(&self, fragment: OpenDataFragment<'_>) -> Result<(), OpenDataFragmentError> {
        let number = usize::from(fragment.fragment_number);
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
        match (self.packet_numbers[number], fragment.packet_number) {
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
        let offset = self.fragment_offsets[number];
        let length = self.fragment_lengths[number];
        if fragment.payload.len() != length
            || self.bytes.get(offset..offset + length) != Some(fragment.payload)
        {
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
        let mut discarded = 0;
        for slot in &mut self.slots {
            if slot
                .identity
                .is_some_and(|identity| identity.transmitter_address == transmitter)
            {
                discarded += usize::from(slot.is_active());
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

        let exact = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        });
        if fragment.fragment_number == 0
            && fragment.retry
            && let Some(index) = exact
        {
            self.slots[index].verify_retry(fragment)?;
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
            let Some(index) = exact else {
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
            let slot = &mut self.slots[index];
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
        let exact = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        });
        if fragment.fragment_number != 0 {
            let Some(index) = exact else {
                return Err(OpenDataFragmentError::Orphan {
                    fragment_number: fragment.fragment_number,
                });
            };
            let slot = &mut self.slots[index];
            if let Err(error) = append_fragment(slot, fragment) {
                slot.clear();
                return Err(error);
            }
            if fragment.more_fragments {
                slot.expected_fragment += 1;
                return Ok(OpenDataDefragmentation::Buffered {
                    expired,
                    evicted: None,
                });
            }

            let length = slot.length;
            if length < LLC_SNAP_HEADER_LEN
                || slot.bytes[..RFC1042_LLC_SNAP_PREFIX.len()] != RFC1042_LLC_SNAP_PREFIX
            {
                slot.clear();
                return Err(OpenDataFragmentError::InvalidLlcSnap);
            }
            let value = complete(OpenReassembledData {
                identity,
                msdu: &slot.bytes[..length],
            });
            slot.completed = true;
            slot.final_fragment = fragment.fragment_number;
            slot.started_at = now;
            return Ok(OpenDataDefragmentation::Complete { expired, value });
        }

        let same_sequence = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        });
        let (index, evicted) = if let Some(index) = exact.or(same_sequence) {
            let evicted = self.slots[index].identity;
            self.slots[index].clear();
            (index, evicted)
        } else if let Some(index) = self.slots.iter().position(|slot| slot.identity.is_none()) {
            (index, None)
        } else if let Some(index) = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.completed)
            .min_by_key(|(index, slot)| (slot.started_at, *index))
            .map(|(index, _)| index)
        {
            self.slots[index].clear();
            (index, None)
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
            let evicted = self.slots[index].identity;
            self.slots[index].clear();
            (index, evicted)
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
        let slot = &mut self.slots[index];
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
    let number = usize::from(fragment.fragment_number);
    if number != 0
        && let (Some(previous), Some(observed)) =
            (slot.packet_numbers[number - 1], fragment.packet_number)
        && observed <= previous
    {
        return Err(OpenDataFragmentError::PacketNumberNotIncreasing { previous, observed });
    }
    let payload = fragment.payload;
    let end = slot
        .length
        .checked_add(payload.len())
        .ok_or(OpenDataFragmentError::ReassembledTooLarge { capacity: CAPACITY })?;
    let Some(destination) = slot.bytes.get_mut(slot.length..end) else {
        return Err(OpenDataFragmentError::ReassembledTooLarge { capacity: CAPACITY });
    };
    destination.copy_from_slice(payload);
    slot.fragment_offsets[number] = slot.length;
    slot.fragment_lengths[number] = payload.len();
    slot.packet_numbers[number] = fragment.packet_number;
    slot.length = end;
    Ok(())
}

const fn timestamp_expired(now: Instant, then: Instant, timeout: Duration) -> bool {
    let (now, then, timeout) = (now.as_micros(), then.as_micros(), timeout.as_micros());
    timeout == 0 || now < then || now - then >= timeout
}
