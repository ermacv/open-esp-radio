//! The port of the new stack over a device of the Ethernet contract.

use core::task::Context;

use oer_protocols_network_port::{
    EthernetFrameView, FrameMeta, FramePort, FrameSource, Holding, Keep, LentFrame, LinkSnapshot,
    NetDevice, OwnedTransmit, PortRefusal, ReceivedFrame, Taken, Unpublished,
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
    /// Frames the device handed over and could not show.
    unreadable: u32,
}

impl<D, X> DevicePort<D, X> {
    pub const fn new(device: D, transmit: X) -> Self {
        Self {
            device,
            transmit,
            waiting: Waiting::Nothing,
            unreadable: 0,
        }
    }

    /// Frames the device handed over and could not show.
    pub const fn unreadable(&self) -> u32 {
        self.unreadable
    }
}

impl<D: NetDevice, X: OwnedTransmit> DevicePort<D, X> {
    /// Give back every frame the device's queue holds and take back the
    /// memory the device returned, each frame's end freeing its buffer with
    /// no policy to settle it: the service that published it ended, its
    /// account with it. The device's queues outlive this port,
    /// so dropping the port returns none of them: a stop calls this before
    /// it tells the device it is over.
    pub fn give_back(&mut self) {
        while self.take_frame(|_, _| Keep::Drop).found() {}
        self.transmit.reclaim();
        // The service whose policy those frames' ends settle has ended, its
        // account with it: each end frees its buffer and goes to no policy.
        while self.transmit.take_notice().is_some() {}
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
    type Held = LentFrame<<D::RxFrame as ReceivedFrame>::Storage>;

    fn take_frame(
        &mut self,
        read: impl FnOnce(EthernetFrameView<'_>, Holding) -> Keep,
    ) -> Taken<Self::Held> {
        let frame = match self.device.try_receive() {
            Ok(Some(frame)) => frame,
            Ok(None) => return Taken::Empty,
            Err(fault) => return Taken::Fault(fault),
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
        self.transmit.take_notice()
    }

    fn link(&self) -> LinkSnapshot {
        self.device.link()
    }

    fn register(&mut self, cx: &mut Context<'_>) -> bool {
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
        received || link || room || ended
    }
}

#[cfg(test)]
mod tests;
