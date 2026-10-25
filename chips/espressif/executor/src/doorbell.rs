//! The radio doorbell: a software interrupt at the radio interrupts' level.
//!
//! The radio executor (P2) cannot write the registers the radio interrupts
//! (P3) own without sharing them with a handler that preempts it. It records
//! a request in memory both sides see and rings the doorbell; the doorbell's
//! handler runs at P3, after or before the radio handlers but never inside
//! one, and applies the request. [`SameLevel`] proves when the image compiles
//! that the doorbell's entry has the radio interrupt's level and core.
//!
//! The doorbell consumes its software interrupt, so no executor can be woken
//! through the same line. Its handler, named in the image's interrupt table,
//! first calls [`DoorbellAcknowledger::acknowledge`], then takes the requests: a ring after that
//! raises the interrupt again and is not lost.

use core::sync::atomic::{Ordering, fence};

use oer_interrupt_table::Entry;

use crate::chip::{Line, platform, software_interrupt_source};
use crate::executor::SameLevel;
use crate::software_line::SoftwareLine;

/// The radio doorbell on software interrupt `SWI` (`FROM_CPU_INTR<SWI>`).
pub struct Doorbell<const SWI: u8> {
    line: Line,
}

/// Check that `Bell` is the token of software interrupt `SWI`.
const fn bell_source<const SWI: u8, Bell: Entry>() {
    assert!(
        <Bell::Part as oer_interrupt_table::InterruptSource>::SOURCE
            == software_interrupt_source(SWI) as u16,
        "the token is not the doorbell's software interrupt"
    );
}

macro_rules! impl_doorbell {
    ($number:literal, $block:ident, $take:ident) => {
        impl Doorbell<$number> {
            /// The doorbell on `interrupt`, whose source `bell` is the token of;
            /// `level` proves that it shares the radio interrupt's level and core.
            /// Moves the line's block from the reset root and lowers a stale
            /// raise. Bind the interrupt part through the router only after
            /// the handler owns the returned acknowledger.
            pub fn new<Radio, Bell>(
                interrupt: platform::$block,
                level: SameLevel<Radio, Bell>,
            ) -> (Self, DoorbellAcknowledger<$number>)
            where
                Radio: Entry,
                Bell: Entry,
            {
                const { bell_source::<$number, Bell>() };
                let _ = level;
                let line = crate::chip::$take(interrupt);
                line.lower();
                // Return both owners before the image enables the route: the
                // handler must own its acknowledger before any ring can reach it.
                (
                    Self { line },
                    DoorbellAcknowledger {
                        line: LineAcknowledger { line },
                    },
                )
            }
        }
    };
}

impl_doorbell!(0, FromCpu0, own_line_0);
impl_doorbell!(1, FromCpu1, own_line_1);
impl_doorbell!(2, FromCpu2, own_line_2);
impl_doorbell!(3, FromCpu3, own_line_3);

impl<const SWI: u8> Doorbell<SWI> {
    /// Raise the doorbell. The fence orders the caller's request writes
    /// before the raise; on ESP32-S31 the raise reads the line back, so it
    /// has reached the interrupt matrix when this returns. Never waits.
    pub fn ring(&self) {
        fence(Ordering::SeqCst);
        self.line.raise();
    }
}

/// The one lowering half of software interrupt line `SWI`, for its handler.
pub struct LineAcknowledger<const SWI: u8> {
    line: Line,
}

impl<const SWI: u8> LineAcknowledger<SWI> {
    /// Lower the line before taking the requests its raises announced: a
    /// ring after this raises the line again and is not lost.
    #[inline(always)]
    pub fn acknowledge(&mut self) {
        self.line.lower();
    }
}

/// The acknowledgement and route authority moved into the doorbell's ISR.
pub struct DoorbellAcknowledger<const SWI: u8> {
    line: LineAcknowledger<SWI>,
}

impl<const SWI: u8> DoorbellAcknowledger<SWI> {
    /// Lower the line before taking requests. Only the handler owns this half.
    #[inline(always)]
    pub fn acknowledge(&mut self) {
        self.line.acknowledge();
    }
}
