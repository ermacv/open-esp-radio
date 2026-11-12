//! Owned Ethernet boundary, independent of a radio or an IP stack.

use crate::{FrameLengthError, LinkState};
use core::task::{Context, Poll};

/// The publication epoch changes on a new association, not on channel absences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkSnapshot {
    pub state: LinkState,
    pub epoch: u64,
}

impl LinkSnapshot {
    pub const DOWN: Self = Self {
        state: LinkState::Down,
        epoch: 0,
    };
}

/// A borrowed view of an owned Ethernet-II frame, excluding the FCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetFrameView<'a> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload: &'a [u8],
}

/// Storage stays with the concrete frame owner. A view borrows it only, and
/// the owner alone reaches its bytes, so a view takes it exclusively.
pub trait EthernetFrame {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError>;
}

/// Bytes a receive owner lends in place, stable while it lives. The owner
/// is dropped in a task of the network's executor; its drop returns the
/// bytes to whoever lent them.
pub trait FrameStorage: Send + 'static {
    /// The owner's bytes.
    fn bytes(&self) -> &[u8];
    /// [`Self::bytes`], mutably, for an adapter that rewrites a header in
    /// place.
    fn bytes_mut(&mut self) -> &mut [u8];
    /// Start the bytes `by` later, for an adapter whose packet begins
    /// inside them. A step past the bytes is refused and changes nothing.
    fn advance(&mut self, by: usize) -> Result<(), StorageAdvanceRefused>;
}

/// Memory a device lends its network for frames to send. The network
/// writes a frame into the bytes, where the device's own packets would let
/// it, and hands the memory back to the device with the frame, which the
/// device sends from it without a copy.
pub trait TxStorage: Send + 'static {
    /// The bytes a frame may take, from the place the device reads a frame
    /// written at their start.
    fn bytes_mut(&mut self) -> &mut [u8];
}

/// A refused [`FrameStorage::advance`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageAdvanceRefused {
    /// The requested step.
    pub by: usize,
    /// The bytes' length then.
    pub length: usize,
}

/// The storage of a frame that never lends: its bytes are its own copy.
impl FrameStorage for core::convert::Infallible {
    fn bytes(&self) -> &[u8] {
        match *self {}
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        match *self {}
    }
    fn advance(&mut self, _: usize) -> Result<(), StorageAdvanceRefused> {
        match *self {}
    }
}

/// A received frame held in its owner's storage: its Ethernet fields, and
/// where its payload lies in the storage.
pub struct LentFrame<S> {
    storage: S,
    destination: [u8; 6],
    source: [u8; 6],
    ether_type: u16,
    payload: core::ops::Range<usize>,
}

/// The parts of a [`LentFrame`], for the adapter that takes its storage.
pub struct LentParts<S> {
    /// The owner; its `payload` bytes are the frame's payload.
    pub storage: S,
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    /// The payload's range in the storage's bytes.
    pub payload: core::ops::Range<usize>,
}

impl<S: FrameStorage> LentFrame<S> {
    /// The frame whose payload is `payload` of `storage`; the storage comes
    /// back unchanged if that range is not inside its bytes.
    pub fn new(
        storage: S,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: core::ops::Range<usize>,
    ) -> Result<Self, S> {
        if storage.bytes().get(payload.clone()).is_none() {
            return Err(storage);
        }
        Ok(Self {
            storage,
            destination,
            source,
            ether_type,
            payload,
        })
    }

    /// The frame's fields and payload. The range was checked at
    /// construction; a storage whose bytes then shrank breaks its own
    /// contract and is refused here.
    pub fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        let payload = self
            .storage
            .bytes()
            .get(self.payload.clone())
            .ok_or(FrameLengthError::OutsideStorage)?;
        Ok(EthernetFrameView {
            destination: self.destination,
            source: self.source,
            ether_type: self.ether_type,
            payload,
        })
    }

    /// The same frame in a wrapper of its storage. `wrap` keeps the bytes:
    /// the payload range stays where it was.
    pub fn map_storage<T: FrameStorage>(self, wrap: impl FnOnce(S) -> T) -> LentFrame<T> {
        LentFrame {
            storage: wrap(self.storage),
            destination: self.destination,
            source: self.source,
            ether_type: self.ether_type,
            payload: self.payload,
        }
    }

    pub fn into_parts(self) -> LentParts<S> {
        LentParts {
            storage: self.storage,
            destination: self.destination,
            source: self.source,
            ether_type: self.ether_type,
            payload: self.payload,
        }
    }
}

impl<S: FrameStorage> EthernetFrame for LentFrame<S> {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        LentFrame::view(self)
    }
}

/// One received frame as a receive batch hands it over: in a buffer its
/// reader may keep, or in the device's own copy, shown for the call only.
#[derive(Debug)]
pub enum Incoming<'a, H> {
    /// A frame in a buffer the device lends: the reader's owner now.
    /// Dropping it gives the buffer back to the device.
    Lent(H),
    /// A frame in the device's own copy, for this reason: readable only
    /// within the call, and released after it. `Err` names a copy the
    /// device could not show.
    Copied {
        frame: Result<EthernetFrameView<'a>, FrameLengthError>,
        reason: CopyReason,
    },
}

/// How a receive batch ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchEnd {
    /// No frame was left: the batch handed every one that waited.
    Drained,
    /// The batch handed its limit: more may wait.
    Limit,
    /// The device's receive queue broke after the frames handed before it:
    /// no frame is handed after it, and the device is not to be read again.
    Fault(ReceiveFault),
}

/// What one receive batch handed over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivedBatch {
    /// The frames handed, each its reader's once handed.
    pub taken: usize,
    pub end: BatchEnd,
}

impl ReceivedBatch {
    /// Whether a frame was handed.
    pub const fn found(&self) -> bool {
        self.taken != 0
    }
}

/// Why a device copied a received frame instead of lending its buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyReason {
    /// The frame's form: its buffer carried more than this frame, such as
    /// several MSDUs, or a frame the device also hands elsewhere, or a
    /// device that lends no buffer at all.
    Format,
    /// The device would have lent the buffer and could not: it was out of
    /// lending room or stopping.
    Pressure,
}

/// A refusal before TX admission. The caller receives its unchanged frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransmitError {
    LinkDown,
    QueueFull,
    InvalidLength(FrameLengthError),
}

#[derive(Debug)]
pub struct TransmitRefused<F> {
    pub frame: F,
    pub reason: TransmitError,
}

/// How a device's receive queue broke: it gave no frame where it held one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveFault {
    /// The queue named a frame's place it does not hold.
    Lost,
    /// A frame's place the queue published held no frame.
    Unfilled,
}

/// The link and receive half of one affine Ethernet port. Poll methods
/// register before observing readiness. Cancelling a readiness wait consumes
/// no frame. RX transfers its concrete owners by value, in bounded batches.
/// The endpoints and their task wakers stay on their executor's hart.
pub trait NetDevice {
    /// The storage a lent frame keeps its bytes in.
    type Storage: FrameStorage;
    fn link(&self) -> LinkSnapshot;
    /// Ready once what the device publishes with its link changed since the
    /// last ready: the link, or what is published in the same word with it
    /// (an association's rate), so the snapshot may be unchanged.
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot>;
    fn receive_queue_len(&self) -> usize;
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    /// Hand the oldest received frames to `take`, at most `limit` of them,
    /// in order: the prefix ready now, never waiting for more. A lent frame
    /// is `take`'s owner once handed; a copy is shown and released. The
    /// queue's places of the frames handed are freed together, once, after
    /// the last of them, the batch's end among them.
    ///
    /// A fault of the queue ([`BatchEnd::Fault`]) ends the batch after the
    /// frames handed before it, which stay handed and whose places are
    /// freed all the same; no frame is handed after it.
    fn receive(
        &mut self,
        limit: usize,
        take: impl FnMut(Incoming<'_, LentFrame<Self::Storage>>),
    ) -> ReceivedBatch;
}

/// The transmit half of one affine Ethernet port, owned by the network
/// stack's sending path: a frame goes to the device where the stack wrote
/// it, in the call that sent it. Only a successful transmission transfers
/// the caller's owner; a refusal returns it.
pub trait NetTransmit {
    /// The stack's packet owner a transmission hands over.
    type Frame: EthernetFrame;
    /// Take back the memory the device returned, to the stack: what the
    /// sending path's next notices and buffers find. A caller asks when what
    /// it holds runs out, not at every frame.
    fn reclaim(&mut self);
    /// Whether the next [`Self::try_transmit`] finds room, as the memory last
    /// reclaimed leaves it, without registering anything: the sending path
    /// asks at every frame.
    fn has_room(&mut self) -> bool;
    /// [`Self::reclaim`], then as [`Self::has_room`], registering `cx` for
    /// the next room or returned memory before observing them.
    fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn try_transmit(&mut self, frame: Self::Frame) -> Result<(), TransmitRefused<Self::Frame>>;
    /// End a burst of [`Self::try_transmit`]s: a frame it took may wait for
    /// this call before its device acts on it.
    fn flush(&mut self);
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures built with slice copies"
)]
mod tests {
    use super::*;

    struct Owned([u8; 32]);
    impl FrameStorage for Owned {
        fn bytes(&self) -> &[u8] {
            &self.0
        }
        fn bytes_mut(&mut self) -> &mut [u8] {
            &mut self.0
        }
        fn advance(&mut self, by: usize) -> Result<(), StorageAdvanceRefused> {
            Err(StorageAdvanceRefused { by, length: 32 })
        }
    }

    #[test]
    fn a_lent_frame_views_its_payload_in_its_owner_storage() {
        let mut bytes = [0; 32];
        bytes[20..24].copy_from_slice(&[1, 2, 3, 4]);
        let Ok(frame) = LentFrame::new(Owned(bytes), [1; 6], [2; 6], 0x0800, 20..24) else {
            panic!("the payload lies inside the storage");
        };
        let view = LentFrame::view(&frame).unwrap();
        assert_eq!(view.payload, &[1, 2, 3, 4]);
        assert_eq!(view.ether_type, 0x0800);
        let parts = frame.into_parts();
        assert_eq!(parts.payload, 20..24);
        assert_eq!(parts.destination, [1; 6]);
    }

    #[test]
    fn a_payload_outside_the_storage_returns_the_owner_unchanged() {
        let Err(owner) = LentFrame::new(Owned([7; 32]), [1; 6], [2; 6], 0x0800, 30..33) else {
            panic!("the range ends past the storage");
        };
        assert_eq!(owner.0, [7; 32]);
        #[allow(clippy::reversed_empty_ranges, reason = "the refused shape")]
        let reversed = LentFrame::new(Owned([7; 32]), [1; 6], [2; 6], 0x0800, 5..4);
        assert!(reversed.is_err());
    }
}
