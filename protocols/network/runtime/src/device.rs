//! The port of the new stack over a device of the Ethernet contract.

use core::task::{Context, Poll};

use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FramePort, FrameSource, NetDevice, NetTransmit,
    ReceivedFrame, SlotTooShort, TransmitError,
};

/// The memory frames are written in, in the device's own transmit buffers, so a
/// frame goes out where it was written.
pub trait TxBuffers {
    /// A written frame, as the device takes it.
    type Frame: EthernetFrame;

    /// Whether a buffer is free.
    fn available(&self) -> bool;

    /// Write a frame of `length` octets by `fill` in a free buffer.
    ///
    /// # Errors
    /// [`PortRefusal`]; the buffer stays free.
    fn write(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<Self::Frame, PortRefusal>;

    /// A frame the device did not take: its buffer is free again.
    fn restore(&mut self, frame: Self::Frame);
}

/// Why a frame did not go to the device. Each leaves the work ready, and the
/// port wakes the service when room or the link comes back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortRefusal {
    /// No free buffer, or a full queue.
    NoRoom,
    /// The link is down.
    LinkDown,
    /// The frame does not fit a buffer, or the device reads it as malformed.
    BadFrame,
}

/// A device and its transmit half as the port of a service.
pub struct DevicePort<D, T, X> {
    device: D,
    transmit: T,
    buffers: X,
    /// A frame was refused: the service hears the next room.
    blocked: bool,
    /// Frames the device handed over and could not show.
    unreadable: u32,
}

impl<D, T, X> DevicePort<D, T, X> {
    pub const fn new(device: D, transmit: T, buffers: X) -> Self {
        Self {
            device,
            transmit,
            buffers,
            blocked: false,
            unreadable: 0,
        }
    }

    /// Frames the device handed over and could not show.
    pub const fn unreadable(&self) -> u32 {
        self.unreadable
    }
}

impl<D, T: NetTransmit, X: TxBuffers<Frame = T::Frame>> FramePort for DevicePort<D, T, X> {
    type Refusal = PortRefusal;
    /// The device says nothing of a frame's end to this stack.
    type Ticket = ();

    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<(), PortRefusal> {
        if !self.buffers.available() || !self.transmit.has_room() {
            self.blocked = true;
            return Err(PortRefusal::NoRoom);
        }
        let frame = self.buffers.write(length, fill)?;
        match self.transmit.try_transmit(frame) {
            Ok(()) => {
                self.blocked = false;
                Ok(())
            }
            Err(refused) => {
                self.buffers.restore(refused.frame);
                self.blocked = true;
                Err(match refused.reason {
                    TransmitError::LinkDown => PortRefusal::LinkDown,
                    TransmitError::QueueFull => PortRefusal::NoRoom,
                    TransmitError::InvalidLength(_) => PortRefusal::BadFrame,
                })
            }
        }
    }

    fn flush(&mut self) {
        self.transmit.flush();
    }
}

impl<D: NetDevice, T: NetTransmit, X> FrameSource for DevicePort<D, T, X> {
    type Notice = ();

    fn take_frame(&mut self, read: impl FnOnce(EthernetFrameView<'_>)) -> bool {
        let Some(frame) = self.device.try_receive() else {
            return false;
        };
        let shown = match frame.into_lent() {
            Ok(lent) => lent.view().map(read),
            Err(copied) => self.device.read_copied(copied, read),
        };
        if shown.is_err() {
            self.unreadable = self.unreadable.saturating_add(1);
        }
        true
    }

    fn take_notice(&mut self) -> Option<()> {
        None
    }

    fn register(&mut self, cx: &mut Context<'_>) {
        let received = self.device.poll_receive_ready(cx).is_ready();
        let link = self.device.poll_link(cx).is_ready();
        // Room matters only to a frame that met none: otherwise an idle port
        // with room would wake the service for nothing, for ever.
        let room = self.blocked && matches!(self.transmit.poll_room(cx), Poll::Ready(()));
        if received || link || room {
            cx.waker().wake_by_ref();
        }
    }
}

#[cfg(test)]
mod tests;
