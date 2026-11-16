//! SCRATCH: items of the goes list that the stays list still names.
#![allow(missing_docs)]

use crate::Protection;
/// Portable refusals before the backend receives a prepared source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// The supplied allocation cannot hold the described frame or header.
    StorageTooShort,
    /// No room after the MAC header for a complete CCMP header.
    CcmpHeaderTooShort,
    /// The external body overlaps the MAC/CCMP prefix.
    BodyOverlap,
    /// The buffer's memory is not the caller's.
    BufferUnavailable,
}
/// The MPDU encryption selected by its operation or aggregate builder.
/// This layout value carries no key; the preparation retains the installed
/// key separately in its `TxProtection` and derives this value from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduProtection {
    /// An unencrypted MAC frame, with no CCMP prefix.
    Open,
    /// A CCMP frame with its reserved CCMP header.
    Ccmp,
}
/// The stages a retry plan holds at most.
pub const RETRY_STAGES: usize = 4;

/// One stage of a retry plan: a rate, the transmissions at it and their
/// protection.
#[derive(Debug)]
pub struct RetryStage<R> {
    /// The rate of the stage's transmissions.
    pub rate: R,
    /// The transmissions at the rate.
    pub transmissions: core::num::NonZeroU8,
    /// The control exchange that protects each of them.
    pub protection: Protection,
}

/// The retry plan of an individual frame: its stages in order, as the
/// rate table mac80211 hands its drivers.
/// The backend transmits the head of its queue by the plan before the next
/// attempt of that queue and keeps the frame's sequence number. A transmission
/// without an ACK sets the Retry bit, which stays; a CTS timeout or a collision
/// sets nothing. An exhausted plan ends the attempt unacknowledged. A backend
/// whose hardware retries carries the plan out there.
#[derive(Debug)]
pub struct RetryPlan<R> {
    /// The first stage: a plan always has one.
    first: RetryStage<R>,
    /// The stages after the first, in order. The occupied cells form a
    /// prefix, which every operation keeps; the array's length is the bound.
    rest: [Option<RetryStage<R>>; RETRY_STAGES - 1],
}

impl<R: Copy> Clone for RetryStage<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: Copy> Copy for RetryStage<R> {}
impl<R: Copy> Clone for RetryPlan<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: Copy> Copy for RetryPlan<R> {}

impl<R> RetryPlan<R> {
    /// The plan of `first` alone.
    pub fn new(first: RetryStage<R>) -> Self {
        Self {
            first,
            rest: core::array::from_fn(|_| None),
        }
    }

    /// The plan with `stage` after its last.
    ///
    /// # Errors
    ///
    /// The plan and the stage, when the plan holds [`RETRY_STAGES`].
    pub fn then(mut self, stage: RetryStage<R>) -> Result<Self, (Self, RetryStage<R>)> {
        let unplaced = match self.rest.iter_mut().find(|cell| cell.is_none()) {
            Some(cell) => {
                *cell = Some(stage);
                None
            }
            None => Some(stage),
        };
        match unplaced {
            None => Ok(self),
            Some(stage) => Err((self, stage)),
        }
    }

    /// The first stage.
    pub const fn first(&self) -> &RetryStage<R> {
        &self.first
    }

    /// The plan past its first stage; `None` when that stage was its last.
    pub fn after_first(self) -> Option<Self> {
        let [second, third, fourth] = self.rest;
        Some(Self {
            first: second?,
            rest: [third, fourth, None],
        })
    }

    /// The first stage, by value: a frame of one rate takes it.
    pub fn into_first(self) -> RetryStage<R> {
        self.first
    }

    /// The stages in order.
    pub fn stages(&self) -> impl Iterator<Item = &RetryStage<R>> {
        core::iter::once(&self.first).chain(self.rest.iter().flatten())
    }

    /// The transmissions the plan makes at most.
    pub fn transmissions(&self) -> u16 {
        self.stages()
            .map(|stage| u16::from(stage.transmissions.get()))
            .sum()
    }

    /// The plan's first `transmissions` transmissions: its stages in order,
    /// the last one cut and the ones after it dropped.
    ///
    /// ```
    /// # use core::num::NonZeroU8;
    /// # use oer_protocols_ieee80211_contracts_port::{Protection, RetryPlan, RetryStage};
    /// let stage = |rate, transmissions| RetryStage {
    ///     rate,
    ///     transmissions: NonZeroU8::new(transmissions).unwrap(),
    ///     protection: Protection::None,
    /// };
    /// let plan = RetryPlan::new(stage('a', 2)).then(stage('b', 3)).ok().unwrap();
    /// let plan = plan.then(stage('c', 25)).ok().unwrap();
    /// let cut = plan.limit(NonZeroU8::new(4).unwrap());
    /// let left: Vec<_> = cut.stages().map(|s| (s.rate, s.transmissions.get())).collect();
    /// assert_eq!(left, [('a', 2), ('b', 2)]);
    /// assert_eq!(cut.transmissions(), 4);
    /// // Each stage's rate turns; a refused one stops the plan.
    /// let numbers = cut.try_map(|rate| Ok::<_, char>(rate as u8)).unwrap();
    /// assert_eq!(numbers.stages().next().unwrap().rate, b'a');
    /// let refused = numbers.try_map(|rate| if rate == b'b' { Err(rate) } else { Ok(rate) });
    /// assert_eq!(refused.err(), Some(b'b'));
    /// ```
    pub fn limit(mut self, transmissions: core::num::NonZeroU8) -> Self {
        let first = self.first.transmissions.min(transmissions);
        // The budget left after the first stage; it only shrinks, so once a
        // stage is dropped every later one is too: the occupied cells stay a
        // prefix.
        // `first` is at most `transmissions`, so this is their difference.
        let mut left = transmissions.get().abs_diff(first.get());
        self.first.transmissions = first;
        for cell in &mut self.rest {
            let taken = cell
                .as_ref()
                .map_or(0, |stage| stage.transmissions.get().min(left));
            // `taken` is at most `left`.
            left = left.abs_diff(taken);
            match (cell.as_mut(), core::num::NonZeroU8::new(taken)) {
                (Some(stage), Some(taken)) => stage.transmissions = taken,
                (_, None) => *cell = None,
                (None, Some(_)) => {}
            }
        }
        self
    }

    /// Map each rate without changing the nonempty plan or its budget.
    pub fn map<S>(&self, mut rate: impl FnMut(&R) -> S) -> RetryPlan<S> {
        let mut stage = |stage: &RetryStage<R>| RetryStage {
            rate: rate(&stage.rate),
            transmissions: stage.transmissions,
            protection: stage.protection,
        };
        RetryPlan {
            first: stage(&self.first),
            rest: self
                .rest
                .each_ref()
                .map(|cell| cell.as_ref().map(&mut stage)),
        }
    }

    /// The plan with each stage protected by the exchange `protection`
    /// names for its rate; rates, counts and order unchanged.
    pub fn with_protection(self, mut protection: impl FnMut(&R) -> Protection) -> Self {
        let mut stage = |stage: RetryStage<R>| RetryStage {
            protection: protection(&stage.rate),
            ..stage
        };
        Self {
            first: stage(self.first),
            rest: self.rest.map(|cell| cell.map(&mut stage)),
        }
    }

    /// Keep the stages whose rates `rate` accepts, in their original order.
    /// Each retained stage keeps its exact transmission count and protection;
    /// `None` means every stage was refused.
    pub fn filter_map<S>(self, mut rate: impl FnMut(R) -> Option<S>) -> Option<RetryPlan<S>> {
        let mut accepted = core::iter::once(self.first)
            .chain(self.rest.into_iter().flatten())
            .filter_map(|stage| {
                Some(RetryStage {
                    rate: rate(stage.rate)?,
                    transmissions: stage.transmissions,
                    protection: stage.protection,
                })
            });
        let first = accepted.next()?;
        // Taking the first accepted stage leaves at most RETRY_STAGES - 1 of
        // the input's stages, one per cell: the zip places every one.
        let mut rest: [Option<RetryStage<S>>; RETRY_STAGES - 1] = core::array::from_fn(|_| None);
        for (cell, stage) in rest.iter_mut().zip(accepted) {
            *cell = Some(stage);
        }
        Some(RetryPlan { first, rest })
    }

    /// The plan with each stage's rate turned by `rate`, the stages and
    /// their transmissions kept.
    ///
    /// # Errors
    ///
    /// The first error `rate` returned.
    pub fn try_map<S, E>(self, mut rate: impl FnMut(R) -> Result<S, E>) -> Result<RetryPlan<S>, E> {
        let mut turn = |stage: RetryStage<R>| {
            Ok(RetryStage {
                rate: rate(stage.rate)?,
                transmissions: stage.transmissions,
                protection: stage.protection,
            })
        };
        let first = turn(self.first)?;
        let mut rest: [Option<RetryStage<S>>; RETRY_STAGES - 1] = core::array::from_fn(|_| None);
        for (cell, stage) in rest.iter_mut().zip(self.rest) {
            if let Some(stage) = stage {
                *cell = Some(turn(stage)?);
            }
        }
        Ok(RetryPlan { first, rest })
    }
}


// named by upper-mac
use crate::{ReceiveOperation, RxCryptoStatus, RxEvidence, RxMetadata};
use oer_protocols_ieee80211_wire::frame::DecodedHeader;
/// The backend's memory of one received MPDU, lent with its pull: the MPDU
/// from its header to the end of its body, without the FCS.
///
/// The consumer owns it until it passes the buffer to
/// [`DataPort::recycle`](crate::DataPort::recycle). Dropping it does
/// not satisfy that protocol; the host model poisons its installation on an
/// unrecycled drop. A DMA backend can lend the receive buffer itself without
/// copying the frame. A retained buffer occupies receive capacity, so an
/// owner that needs longer storage may copy the frame and recycle this buffer.
pub trait RxBuffer {
    /// Borrow the MPDU bytes while retaining ownership of its receive buffer.
    fn bytes(&self) -> &[u8];

    /// The MPDU's MAC header over [`Self::bytes`], as
    /// [`DecodedHeader::decode`] would decode it, `None` alike. A buffer
    /// may decode it once when it takes the frame and give it again here,
    /// for its own bytes only: no reader decodes the variable layout again.
    fn header(&self) -> Option<DecodedHeader<'_>>;
}
/// A received buffer moved out of the port's receive capacity to a network
/// owner ([`DataPort::lend`](crate::DataPort::lend)).
///
/// The storage starts at the MPDU's first byte, as [`RxBuffer::bytes`] lent
/// it, and continues to the end of its allocation; every byte is
/// initialized. The owner may rewrite it in place. Dropping the owner
/// returns the memory to the backend, which counts the buffer as held by
/// the network until then; the drop happens in a task, never in an
/// interrupt handler.
pub trait LentRxBuffer: Send + 'static {
    /// The MPDU, then the rest of its allocation.
    fn storage(&self) -> &[u8];
    /// [`Self::storage`], mutably.
    fn storage_mut(&mut self) -> &mut [u8];
    /// The MPDU's length at the start of the storage.
    fn mpdu_len(&self) -> usize;
    /// Start the storage `by` bytes later, inside the MPDU, which shortens
    /// by as much. A step past the MPDU is refused and changes nothing.
    fn advance(&mut self, by: usize) -> Result<(), AdvanceRefused>;
}
/// A refused [`LentRxBuffer::advance`]: the step does not fit the MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvanceRefused {
    /// The requested step.
    pub by: usize,
    /// The MPDU's length then.
    pub mpdu_len: usize,
}
/// Why a backend kept a receive buffer instead of lending it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LendRefusal {
    /// The backend's buffers cannot leave its receive capacity.
    Unsupported,
    /// The network already holds every buffer the backend lends.
    Capacity,
    /// The installation is stopping or stopped; it lends nothing more.
    Stopping,
}
/// A refused [`DataPort::lend`](crate::DataPort::lend): the
/// unchanged buffer, still owed to
/// [`DataPort::recycle`](crate::DataPort::recycle), and why.
#[derive(Debug)]
pub struct LendRefused<B> {
    /// The buffer, unchanged.
    pub buffer: B,
    /// Why it was kept.
    pub reason: LendRefusal,
}
/// A receive count that did not hold: `operation` would return
/// `requested` units while the port held `held` of the caller's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveAccounting {
    /// The operation the port refused.
    pub operation: ReceiveOperation,
    /// The caller's units the port held then.
    pub held: usize,
    /// The units the operation would return.
    pub requested: usize,
}
/// A refused [`DataPort::recycle`](crate::DataPort::recycle): the port
/// took nothing and recorded no fault. The unit comes back unchanged, and
/// its count says why; a later recycle of the same unit is no repair, so
/// its caller keeps it and ends with the refusal.
#[derive(Debug)]
pub struct RecycleRefused<B> {
    /// The unit, unchanged.
    pub unit: B,
    /// The count that refused it.
    pub fault: ReceiveAccounting,
}
/// An installed TX key and the PN allocated by its core for one fresh MPDU.
/// Retransmissions retain the encoded header rather than requesting another.
pub struct NumberedKey<Key> {
    /// The phase's borrowed hardware key authority.
    pub key: Key,
    /// Caller-allocated, checked 48-bit packet number.
    pub packet_number: oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber,
}

// named by roles/sta
use crate::{Ieee80211Capabilities, PortRate, TxFrame};
/// Dynamic preparation failures, distinct from the outcome of accepted TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparationError {
    /// The backend source has no authority for this body resource loan.
    MissingBodyLoan,
    /// A retry names a different key installation, even in the same slot.
    RetransmissionKey,
    /// The new rate plan requires a different response kind.
    RetransmissionKind,
}

/// A class of transmit buffers: memory of one size, which the composition
/// sets.
pub trait TxBufferClass {
    /// The longest MPDU a buffer of the class holds, without its FCS.
    const CAPACITY: usize;
}

/// The phase types of a backend that keeps a pool of the buffer class `C`.
/// A frame that a buffer of the class `C` fits; only [`ClassFrame::new`]
/// makes one.
pub struct ClassFrame<'p, C, K> {
    frame: TxFrame<'p, K>,
    class: core::marker::PhantomData<C>,
}

impl<'p, C: TxBufferClass, K: Ieee80211Capabilities> ClassFrame<'p, C, K> {
    /// `frame`, when a buffer of the class `C` fits it.
    ///
    /// # Errors
    ///
    /// The frame, when it is longer than the class holds.
    pub fn new(frame: TxFrame<'p, K>) -> Result<Self, TxFrame<'p, K>> {
        if frame.mpdu_len() <= C::CAPACITY {
            Ok(Self {
                frame,
                class: core::marker::PhantomData,
            })
        } else {
            Err(frame)
        }
    }

    /// The frame.
    pub fn into_frame(self) -> TxFrame<'p, K> {
        self.frame
    }

    /// Borrow the checked description for preparation without consuming
    /// the original operation, so a refusal can return it intact.
    pub const fn frame(&self) -> &TxFrame<'p, K> {
        &self.frame
    }
}

/// A phase-independent checked description that fits its buffer class.
/// Portable machines retain it in the operation context. Adding the phase's
/// rates cannot lose or re-check the established size proof.
#[derive(Debug)]
pub struct ClassSizedFrame<C, H, K> {
    frame: crate::SizedFrame<H, K>,
    class: core::marker::PhantomData<C>,
}
impl<C, H: Copy, K> Copy for ClassSizedFrame<C, H, K> {}
impl<C, H: Copy, K> Clone for ClassSizedFrame<C, H, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C: TxBufferClass, H: crate::FrameHeader, K: Ieee80211Capabilities> ClassSizedFrame<C, H, K> {
    /// Establish the class-size proof while constructing the operation.
    pub fn new(frame: crate::SizedFrame<H, K>) -> Result<Self, crate::SizedFrame<H, K>> {
        if frame.len().get() <= C::CAPACITY {
            Ok(Self {
                frame,
                class: core::marker::PhantomData,
            })
        } else {
            Err(frame)
        }
    }
    /// The already checked description.
    pub const fn frame(&self) -> &crate::SizedFrame<H, K> {
        &self.frame
    }
}
impl<'p, C, K> ClassFrame<'p, C, K> {
    /// Add only the port-bound retry plan to an individually acknowledged MPDU.
    pub fn individual(
        frame: ClassSizedFrame<C, crate::AckHeader, K>,
        plan: RetryPlan<PortRate<'p, K>>,
    ) -> Self {
        Self {
            frame: TxFrame::Individual {
                frame: frame.frame,
                plan,
            },
            class: core::marker::PhantomData,
        }
    }
    /// Add only the port-bound rate to a group MPDU.
    pub fn group(
        frame: ClassSizedFrame<C, crate::HeaderSpec<crate::GroupAddress>, K>,
        rate: PortRate<'p, K>,
    ) -> Self {
        Self {
            frame: TxFrame::Group {
                frame: frame.frame,
                rate,
            },
            class: core::marker::PhantomData,
        }
    }
}

