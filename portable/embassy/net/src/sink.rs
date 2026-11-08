//! A device's transmit half for this crate's tests.

extern crate std;
use crate::{NetTransmit, OwnedNetworkTxFrame, TransmitError};
use core::task::{Context, Poll, Waker};
use oer_protocols_network_port::TransmitRefused;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

pub(crate) struct SinkState {
    pub taken: VecDeque<OwnedNetworkTxFrame>,
    /// The frames it takes before it is full.
    pub room: usize,
    /// The refusal of the next transmission.
    pub refusal: Option<TransmitError>,
    /// Registered for the next room.
    pub waker: Option<Waker>,
    /// The bursts it was told ended.
    pub flushed: usize,
}

/// Keeps what it takes.
#[derive(Clone)]
pub(crate) struct Sink(pub Rc<RefCell<SinkState>>);

impl Sink {
    pub fn with_room(room: usize) -> Self {
        Self(Rc::new(RefCell::new(SinkState {
            taken: VecDeque::new(),
            room,
            refusal: None,
            waker: None,
            flushed: 0,
        })))
    }

    /// Room for `frames` more, waking who waits for it.
    pub fn free(&self, frames: usize) {
        let mut state = self.0.borrow_mut();
        state.room += frames;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl NetTransmit for Sink {
    type Frame = OwnedNetworkTxFrame;
    fn has_room(&mut self) -> bool {
        self.0.borrow().room != 0
    }
    fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.0.borrow_mut();
        state.waker = Some(cx.waker().clone());
        if state.room != 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
    fn flush(&mut self) {
        self.0.borrow_mut().flushed += 1;
    }
    /// The sink keeps nothing it returns later.
    fn take_returned(&mut self) {}
    fn poll_returned(&mut self, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
    fn try_transmit(
        &mut self,
        frame: OwnedNetworkTxFrame,
    ) -> Result<(), TransmitRefused<OwnedNetworkTxFrame>> {
        let mut state = self.0.borrow_mut();
        if let Some(reason) = state.refusal.take() {
            return Err(TransmitRefused { frame, reason });
        }
        if state.room == 0 {
            return Err(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            });
        }
        state.room -= 1;
        state.taken.push_back(frame);
        Ok(())
    }
}
