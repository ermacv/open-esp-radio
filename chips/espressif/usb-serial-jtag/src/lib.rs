#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]

//! The USB Serial/JTAG console of Espressif chips, over the registers the
//! chip HAL lends ([`EndpointRegisters`], [`InterruptRegisters`]).
//!
//! The block has one holder at a time, which the chip HAL's [`ownership`]
//! state names and every writer takes with one compare-and-swap, never
//! waiting for it. From reset the early writer may hold it for one record at
//! a time: it builds the HAL's owner for each record, writes while the FIFO
//! has room, waits for room within a budget before an executor exists, and
//! drops and counts the rest, as it does with a record that finds the block
//! held. The console's driver takes only a free block; then every early
//! record goes to the driver. The driver is one duplex [`Endpoint`]
//! (`embedded-io-async` `Read` and `Write`) of one task and its
//! [`Interrupt`]: the handler, which holds the interrupt's registers as its
//! table entry's part, reads the pending events, acknowledges exactly those
//! and raises the endpoint's static
//! [`IrqSignal`](oer_interrupt_table::signal::IrqSignal); it calls no
//! waker, and the interrupt enables belong to the task, which sets them once
//! with the part's [`EventAuthority`](oer_interrupt_table::EventAuthority),
//! so the two share no register and no lock. A fatal path takes only a free
//! block, for good, and never one the driver or an early record holds.

#[cfg(test)]
extern crate std;

mod endpoint;
mod interrupt;
mod lines;
pub mod ownership;

pub use endpoint::{Endpoint, Error};
pub use interrupt::Interrupt;
pub use lines::{NoteError, write_note};

/// The bytes of one full-speed bulk packet: the size of the IN FIFO.
pub const PACKET: usize = 64;

/// The console's interrupt events, as the handler takes them and the
/// endpoint's signal carries them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Events {
    /// The IN FIFO became empty: the host read the last packet.
    pub in_empty: bool,
    /// The OUT endpoint received a packet.
    pub out_received: bool,
}

/// The signal's fact of [`Events::in_empty`].
const IN_EMPTY: u32 = 1 << 0;
/// The signal's fact of [`Events::out_received`].
const OUT_RECEIVED: u32 = 1 << 1;

impl Events {
    /// No event.
    pub const NONE: Self = Self {
        in_empty: false,
        out_received: false,
    };

    /// The events as the facts of the endpoint's signal.
    const fn facts(self) -> u32 {
        (if self.in_empty { IN_EMPTY } else { 0 })
            | (if self.out_received { OUT_RECEIVED } else { 0 })
    }
}

/// The task's registers of the block: the data FIFO, its configuration and
/// the interrupt enables.
pub trait EndpointRegisters {
    /// The interrupt part whose binding or silence permits event enablement.
    type Interrupt: oer_interrupt_table::InterruptSources;

    /// Whether the IN FIFO has room for a byte. After [`send`](Self::send)
    /// it has none until the host read the packet.
    fn has_room(&self) -> bool;
    /// Appends `byte` to the IN FIFO; called only while it has room.
    fn push(&mut self, byte: u8);
    /// Hands the IN FIFO's bytes to the host as one packet; with none, a
    /// zero-length packet. Never waits.
    fn send(&mut self);
    /// Whether the OUT FIFO holds a byte.
    fn has_data(&self) -> bool;
    /// Takes one byte from the OUT FIFO; called only while it holds one.
    fn pop(&mut self) -> u8;
    /// Enables both console interrupts, keeping every other enable, with
    /// proof that this part is bound to its handler or remains silent.
    fn enable_events(
        &mut self,
        authority: &impl oer_interrupt_table::EventAuthority<Self::Interrupt>,
    );
}

/// The interrupt handler's registers of the block.
pub trait InterruptRegisters {
    /// Reads the pending console events and acknowledges exactly those.
    fn take_events(&mut self) -> Events;
}

#[cfg(test)]
mod fake;
