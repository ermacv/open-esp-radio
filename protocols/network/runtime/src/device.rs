//! The port of the new stack over a device of the Ethernet contract.

use core::task::{Context, Poll};

use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FramePort, FrameSource, Holding, Keep, LentFrame,
    LinkSnapshot, NetDevice, NetTransmit, ReceivedFrame, Taken, TransmitError, Unpublished,
};

/// The memory frames are written in, in the device's own transmit buffers, so a
/// frame goes out where it was written.
pub trait TxBuffers {
    /// A written frame, as the device takes it.
    type Frame: EthernetFrame;
    /// Names one frame the device took.
    type Ticket;
    /// The end of a frame the device took, as the egress policy hears it.
    type Notice;

    /// Whether a buffer is free.
    fn available(&self) -> bool;

    /// As [`Self::available`], registering `cx` for the next free buffer.
    fn poll_free(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    /// Write a frame of `length` octets by `fill` in a free buffer.
    ///
    /// # Errors
    /// [`Unpublished`]: a [`PortRefusal`], or the error `fill` returned; the
    /// buffer stays free.
    fn write<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<Self::Frame, Unpublished<PortRefusal, E>>;

    /// A frame the device did not take: its buffer is free again.
    fn restore(&mut self, frame: Self::Frame);

    /// The ticket of `frame`, which the device is about to take: it names
    /// the frame's end in a later [`Self::Notice`].
    fn ticket(&self, frame: &Self::Frame) -> Self::Ticket;

    /// The next end of a frame the device took, for the egress policy. A
    /// buffer the device gave back is free again only here, when its end is
    /// handed on: so no more frames are unsettled than there are buffers.
    fn take_notice(&mut self) -> Option<Self::Notice>;

    /// Whether [`Self::take_notice`] has one.
    fn notice_waiting(&self) -> bool;
}

/// Why a frame did not go to the device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortRefusal {
    /// No free buffer, or a full queue: the work stays ready, and the buffer
    /// or the room wakes the service.
    NoRoom,
    /// The link is down: the work stays ready, and a change of link wakes the
    /// service.
    LinkDown,
    /// The frame does not fit a buffer, or the device reads it as malformed:
    /// the same frame would meet the same, so this is final for it.
    BadFrame,
}

/// What a refused frame needs before it is worth another try.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Waiting {
    Nothing,
    /// A free buffer.
    Buffer,
    /// Room in the device's queue.
    Room,
    /// A change of link, which the port always hears.
    Link,
}

/// A device and its transmit half as the port of a service.
pub struct DevicePort<D, T, X> {
    device: D,
    transmit: T,
    buffers: X,
    /// What the last refused frame needs.
    waiting: Waiting,
    /// Frames the device handed over and could not show.
    unreadable: u32,
}

impl<D, T, X> DevicePort<D, T, X> {
    pub const fn new(device: D, transmit: T, buffers: X) -> Self {
        Self {
            device,
            transmit,
            buffers,
            waiting: Waiting::Nothing,
            unreadable: 0,
        }
    }

    /// Frames the device handed over and could not show.
    pub const fn unreadable(&self) -> u32 {
        self.unreadable
    }
}

impl<D: NetDevice, T: NetTransmit, X: TxBuffers> DevicePort<D, T, X> {
    /// Give back every frame the device's queue holds and take back the
    /// memory the device returned, each frame's end freeing its buffer with
    /// no policy to settle it: the service that published it ended, its
    /// account with it. The device's queues outlive this port,
    /// so dropping the port returns none of them: a stop calls this before
    /// it tells the device it is over.
    pub fn give_back(&mut self) {
        while self.take_frame(|_, _| Keep::Drop).found() {}
        // Asking the transmit half takes back what the device returned.
        let _room = self.transmit.has_room();
        // The service whose policy those frames' ends settle has ended, its
        // account with it: each end frees its buffer and goes to no policy.
        while self.buffers.take_notice().is_some() {}
        // The exchange ended: no refused frame waits for anything, so a next
        // service over this port is not woken for one.
        self.waiting = Waiting::Nothing;
    }

    /// [`Self::give_back`], then register `cx` for the device's next frame
    /// and its next returned memory, looking again until neither is ready.
    /// The return path of a port whose service no longer runs: an owner
    /// that outlives a cancelled `run` (or a retired service) drives it, so
    /// what the device hands over or gives back keeps coming home without
    /// the service. Only a composition that has such an owner gets this
    /// return; cancelling the network without one serves nothing.
    pub fn serve_returns(&mut self, cx: &mut Context<'_>) {
        loop {
            self.give_back();
            let received = self.device.poll_receive_ready(cx).is_ready();
            // Takes back what came meanwhile and registers for the next.
            let _room = self.transmit.poll_room(cx);
            if !received {
                return;
            }
        }
    }
}

impl<D, T: NetTransmit, X: TxBuffers<Frame = T::Frame>> FramePort for DevicePort<D, T, X> {
    type Refusal = PortRefusal;
    /// The buffers' name of the frame, which their notice of its end
    /// carries.
    type Ticket = X::Ticket;

    fn publish<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<X::Ticket, Unpublished<PortRefusal, E>> {
        // The queue first: asking it is where a device takes back what it
        // finished, which may be the buffer asked for next.
        if !self.transmit.has_room() {
            self.waiting = Waiting::Room;
            return Err(Unpublished::Port(PortRefusal::NoRoom));
        }
        if !self.buffers.available() {
            self.waiting = Waiting::Buffer;
            return Err(Unpublished::Port(PortRefusal::NoRoom));
        }
        let frame = match self.buffers.write(length, fill) {
            Ok(frame) => frame,
            Err(unpublished) => {
                // A failed fill is the frame's, not the port's: it waits for
                // nothing.
                self.waiting = match unpublished {
                    Unpublished::Port(PortRefusal::NoRoom) => Waiting::Buffer,
                    Unpublished::Port(PortRefusal::LinkDown) => Waiting::Link,
                    Unpublished::Port(PortRefusal::BadFrame) | Unpublished::Fill(_) => {
                        Waiting::Nothing
                    }
                };
                return Err(unpublished);
            }
        };
        let ticket = self.buffers.ticket(&frame);
        match self.transmit.try_transmit(frame) {
            Ok(()) => {
                self.waiting = Waiting::Nothing;
                Ok(ticket)
            }
            Err(refused) => {
                self.buffers.restore(refused.frame);
                let (waiting, refusal) = match refused.reason {
                    TransmitError::LinkDown => (Waiting::Link, PortRefusal::LinkDown),
                    TransmitError::QueueFull => (Waiting::Room, PortRefusal::NoRoom),
                    TransmitError::InvalidLength(_) => (Waiting::Nothing, PortRefusal::BadFrame),
                };
                self.waiting = waiting;
                Err(Unpublished::Port(refusal))
            }
        }
    }

    fn is_final(refusal: &PortRefusal) -> bool {
        *refusal == PortRefusal::BadFrame
    }

    fn flush(&mut self) {
        self.transmit.flush();
    }
}

impl<D: NetDevice, T: NetTransmit, X: TxBuffers> FrameSource for DevicePort<D, T, X> {
    /// The buffers' notice of a frame's end.
    type Notice = X::Notice;

    /// A lent frame's own buffer, for a reader that keeps it.
    type Held = LentFrame<<D::RxFrame as ReceivedFrame>::Storage>;

    fn take_frame(
        &mut self,
        read: impl FnOnce(EthernetFrameView<'_>, Holding) -> Keep,
    ) -> Taken<Self::Held> {
        let Some(frame) = self.device.try_receive() else {
            return Taken::Empty;
        };
        match frame.into_lent() {
            Ok(lent) => match lent.view().map(|view| read(view, Holding::Lendable)) {
                Ok(Keep::Keep) => Taken::Kept(lent),
                Ok(Keep::Drop) => Taken::Read,
                Err(_) => {
                    self.unreadable = self.unreadable.saturating_add(1);
                    Taken::Read
                }
            },
            Err(copied) => {
                let reason = <D::RxFrame as ReceivedFrame>::copy_reason(&copied);
                // A copy stays the device's, whatever its reader answers.
                let shown = self
                    .device
                    .read_copied(copied, |view| read(view, Holding::Copied(reason)));
                if shown.is_err() {
                    self.unreadable = self.unreadable.saturating_add(1);
                }
                Taken::Read
            }
        }
    }

    fn take_notice(&mut self) -> Option<X::Notice> {
        // Asking the transmit half takes back what the device returned, which
        // becomes the buffers' notices first.
        let _room = self.transmit.has_room();
        self.buffers.take_notice()
    }

    fn link(&self) -> LinkSnapshot {
        self.device.link()
    }

    fn register(&mut self, cx: &mut Context<'_>) {
        let received = self.device.poll_receive_ready(cx).is_ready();
        let link = self.device.poll_link(cx).is_ready();
        // The transmit half takes back the memory the device returned and
        // registers for the next return, sending or not: an idle port's
        // memory comes home too. Its room wakes only a refused frame that
        // waits for room.
        let has_room = self.transmit.poll_room(cx).is_ready();
        let ended = self.buffers.notice_waiting();
        // Only what a refused frame needs: a free queue with the link down
        // or no buffer would wake the service to the same refusal, for ever,
        // and an idle port needs nothing.
        let room = match self.waiting {
            Waiting::Nothing | Waiting::Link => false,
            Waiting::Room => has_room,
            Waiting::Buffer => self.buffers.poll_free(cx).is_ready(),
        };
        if received || link || room || ended {
            cx.waker().wake_by_ref();
        }
    }
}

#[cfg(test)]
mod tests;
