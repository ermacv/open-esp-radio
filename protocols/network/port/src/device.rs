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

/// A received frame a [`NetDevice`] hands over: lent in its owner's
/// storage, or a copy the device keeps, which [`NetDevice::read_copied`]
/// reads where it lies. The frame a device hands over stays small either
/// way: queues and the bridge move it by value.
pub trait ReceivedFrame: Sized {
    /// The storage a lent frame keeps its bytes in.
    type Storage: FrameStorage;
    /// What stands for a copy the device keeps.
    type Copied;
    /// The frame in its owner's storage, or what stands for its copy.
    fn into_lent(self) -> Result<LentFrame<Self::Storage>, Self::Copied>;
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

/// The link and receive half of one affine Ethernet port. Poll methods
/// register before observing readiness. Cancelling a readiness wait consumes
/// no frame. RX transfers its concrete owner by value. The endpoints and
/// their task wakers stay on their executor's hart.
pub trait NetDevice {
    type RxFrame: ReceivedFrame;
    fn link(&self) -> LinkSnapshot;
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot>;
    fn receive_queue_len(&self) -> usize;
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn try_receive(&mut self) -> Option<Self::RxFrame>;
    /// Read the copy `copied` stands for, in place, and release it. A copy
    /// is read in the order the device handed its frames over.
    ///
    /// # Errors
    ///
    /// [`FrameLengthError`] when the copy is not a frame the device can
    /// view; the copy is released all the same.
    fn read_copied<R>(
        &mut self,
        copied: <Self::RxFrame as ReceivedFrame>::Copied,
        read: impl FnOnce(EthernetFrameView<'_>) -> R,
    ) -> Result<R, FrameLengthError>;
}

/// The transmit half of one affine Ethernet port, owned by the network
/// stack's sending path: a frame goes to the device where the stack wrote
/// it, in the call that sent it. Only a successful transmission transfers
/// the caller's owner; a refusal returns it.
pub trait NetTransmit {
    /// The stack's packet owner a transmission hands over.
    type Frame: EthernetFrame;
    /// Whether the next [`Self::try_transmit`] finds room, without
    /// registering anything: the sending path asks at every frame.
    fn has_room(&mut self) -> bool;
    /// As [`Self::has_room`], registering `cx` for the next room or returned
    /// memory before observing them.
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
