//! The contract of a port frames are published to and taken from in place.

use core::task::Context;

/// Where a frame goes out, and the one answer to it. A port that says `Ok`
/// without calling `fill` contradicts itself, and whoever publishes treats
/// that as a fault of the port.
pub trait FramePort {
    /// Why the port took nothing, which names what it waits for.
    type Refusal;

    /// Take room for `length` octets, let `fill` write the frame there and
    /// accept it. `Ok` says the frame is the port's.
    ///
    /// # Errors
    /// The port accepted nothing: it did not have the room, or `fill` failed.
    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<(), Self::Refusal>;
}

/// Frames coming in, and the wakes of a port.
pub trait FrameSource {
    /// Show the next received frame to `read`; whether there was one.
    fn take_frame(&mut self, read: impl FnOnce(&[u8])) -> bool;

    /// Make `cx` the one a received frame or room to publish wakes, at once
    /// if either came since the last look.
    fn register(&mut self, cx: &mut Context<'_>);
}

/// The room a port gave does not hold the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotTooShort;
