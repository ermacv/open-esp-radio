//! The port of the new stack over a device of the Ethernet contract.

use core::task::Context;

use oer_protocols_network_port::{
    BatchEnd, FrameMeta, FramePort, FrameSource, Incoming, LentFrame, LinkSnapshot, NetDevice,
    OwnedTransmit, PortReady, PortRefusal, ReceiveFault, ReceivedBatch, Unpublished,
};

/// What a refused frame needs before it is worth another try.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Waiting {
    Nothing,
    /// A free buffer and room to hand it on.
    Room,
    /// A change of link, which the port always hears.
    Link,
}

/// A device and the transmit half that owns its buffers, as the port of a
/// service.
pub struct DevicePort<D, X> {
    device: D,
    transmit: X,
    /// What the last refused frame needs.
    waiting: Waiting,
    /// The device's receive queue broke in a give-back: it is read no more.
    broken: Option<ReceiveFault>,
}

impl<D, X> DevicePort<D, X> {
    pub const fn new(device: D, transmit: X) -> Self {
        Self {
            device,
            transmit,
            waiting: Waiting::Nothing,
            broken: None,
        }
    }
}

impl<D: NetDevice, X: OwnedTransmit> DevicePort<D, X> {
    /// Give back every frame the device's queue holds and take back the
    /// memory the device returned, each frame's end freeing its buffer with
    /// no policy to settle it: the service that published it ended, its
    /// account with it. The device's queues outlive this port,
    /// so dropping the port returns none of them: a stop calls this before
    /// it tells the device it is over.
    ///
    /// # Errors
    ///
    /// The device's receive queue broke ([`ReceiveFault`]): the frames
    /// before the break went back, the transmit memory and notices too, and
    /// the frames after it stay in the queue, which this port alone reaches
    /// and never reads again. A dropped port strands them: its owner keeps
    /// it. Each later give-back answers the same fault without reading.
    pub fn give_back(&mut self) -> Result<(), ReceiveFault> {
        // Every frame the queue holds, in batches; a broken queue is read no
        // further, and what it still holds stays the device's.
        if self.broken.is_none() {
            loop {
                match self.device.receive(usize::MAX, |frame| drop(frame)).end {
                    BatchEnd::Limit => {}
                    BatchEnd::Drained => break,
                    BatchEnd::Fault(fault) => {
                        self.broken = Some(fault);
                        break;
                    }
                }
            }
        }
        self.transmit.reclaim();
        // The service whose policy those frames' ends settle has ended, its
        // account with it: each end frees its buffer and goes to no policy.
        while self.transmit.take_notice().is_some() {}
        // The exchange ended: no refused frame waits for anything, so a next
        // service over this port is not woken for one.
        self.waiting = Waiting::Nothing;
        self.broken.map_or(Ok(()), Err)
    }

    /// End the port: [`Self::give_back`], then let it go, its device's
    /// queues emptied.
    ///
    /// # Errors
    ///
    /// The device's receive queue broke ([`BrokenPort`]): the port comes
    /// back with the fault, for its owner to keep, as it alone reaches the
    /// frames after the break.
    pub fn close(mut self) -> Result<(), BrokenPort<Self>> {
        match self.give_back() {
            Ok(()) => Ok(()),
            Err(fault) => Err(BrokenPort { fault, port: self }),
        }
    }

    /// [`Self::give_back`], then register `cx` for the device's next frame
    /// and its next returned memory, looking again until neither is ready.
    /// The return path of a port whose service no longer runs: an owner
    /// that outlives a cancelled `run` (or a retired service) drives it, so
    /// what the device hands over or gives back keeps coming home without
    /// the service. Only a composition that has such an owner gets this
    /// return; cancelling the network without one serves nothing.
    ///
    /// # Errors
    ///
    /// The device's receive queue broke ([`Self::give_back`]): its frames
    /// are served no more, and `cx` hears only returned transmit memory.
    pub fn serve_returns(&mut self, cx: &mut Context<'_>) -> Result<(), ReceiveFault> {
        loop {
            let given = self.give_back();
            // Takes back what came meanwhile and registers for the next.
            let _room = self.transmit.poll_room(cx);
            given?;
            if !self.device.poll_receive_ready(cx).is_ready() {
                return Ok(());
            }
        }
    }
}

/// A port [`DevicePort::close`] could not end: its device's receive queue
/// broke, and the frames after the break stay in it, reached through this
/// port alone, which is never read again.
#[derive(Debug)]
pub struct BrokenPort<P> {
    pub fault: ReceiveFault,
    pub port: P,
}

impl<D, X: OwnedTransmit> FramePort for DevicePort<D, X> {
    type Refusal = PortRefusal;
    /// The transmit half's name of the frame, which its notice of the
    /// frame's end carries.
    type Ticket = X::Ticket;

    fn publish<E>(
        &mut self,
        meta: FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<X::Ticket, Unpublished<PortRefusal, E>> {
        let published = self.transmit.publish(meta, fill);
        // A failed fill is the frame's, not the port's: it waits for nothing.
        self.waiting = match &published {
            Err(Unpublished::Port(PortRefusal::NoRoom)) => Waiting::Room,
            Err(Unpublished::Port(PortRefusal::LinkDown)) => Waiting::Link,
            Ok(_)
            | Err(
                Unpublished::Port(PortRefusal::BadFrame | PortRefusal::BufferLost { .. })
                | Unpublished::Fill(_),
            ) => Waiting::Nothing,
        };
        published
    }

    fn is_final(refusal: &PortRefusal) -> bool {
        *refusal == PortRefusal::BadFrame
    }

    fn is_fault(refusal: &PortRefusal) -> bool {
        matches!(refusal, PortRefusal::BufferLost { .. })
    }

    fn flush(&mut self) {
        self.transmit.flush();
    }
}

impl<D: NetDevice, X: OwnedTransmit> FrameSource for DevicePort<D, X> {
    /// The buffers' notice of a frame's end.
    type Notice = X::Notice;

    /// A lent frame's own buffer, for a reader that keeps it.
    type Held = LentFrame<D::Storage>;

    fn take_frames(
        &mut self,
        limit: usize,
        take: impl FnMut(Incoming<'_, Self::Held>),
    ) -> ReceivedBatch {
        self.device.receive(limit, take)
    }

    fn take_notice(&mut self) -> Option<X::Notice> {
        self.transmit.take_notice()
    }

    fn link(&self) -> LinkSnapshot {
        self.device.link()
    }

    fn register(&mut self, cx: &mut Context<'_>) -> PortReady {
        let received = self.device.poll_receive_ready(cx).is_ready();
        let link = self.device.poll_link(cx).is_ready();
        // The transmit half takes back the memory the device returned and
        // registers for the next return, sending or not: an idle port's
        // memory comes home too. Its room wakes only a refused frame that
        // waits for room.
        let has_room = self.transmit.poll_room(cx).is_ready();
        let ended = self.transmit.notice_waiting();
        // Only what a refused frame needs: room with the link down would
        // wake the service to the same refusal, for ever, and an idle port
        // needs nothing.
        let room = self.waiting == Waiting::Room && has_room;
        PortReady {
            received,
            other: link || room || ended,
        }
    }
}

#[cfg(test)]
mod tests;
