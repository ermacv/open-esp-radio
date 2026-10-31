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

/// Storage stays with the concrete frame owner. A view borrows it only.
pub trait EthernetFrame {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError>;
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

/// One affine Ethernet port. Poll methods register before observing readiness.
/// Cancelling a readiness wait consumes no frame or capacity. Only successful
/// TX transfers the caller's owner; RX transfers its concrete owner by value.
/// Readiness is an observation; a later TX refusal still returns the owner.
/// The endpoints and their task wakers stay on their executor's hart.
pub trait NetDevice {
    type RxFrame: EthernetFrame;
    fn link(&self) -> LinkSnapshot;
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot>;
    fn receive_queue_len(&self) -> usize;
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn try_receive(&mut self) -> Option<Self::RxFrame>;
    /// EXPERIMENT: lend the next received frame in place.
    fn receive_with<R>(&mut self, use_frame: impl FnOnce(&Self::RxFrame) -> R) -> Option<R> {
        self.try_receive().map(|frame| use_frame(&frame))
    }
    fn poll_transmit_ready(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn try_transmit<F: EthernetFrame>(&mut self, frame: F) -> Result<(), TransmitRefused<F>>;
}
