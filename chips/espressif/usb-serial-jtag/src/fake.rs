//! A host model of the block for the tests: the IN FIFO with its packets
//! and the host that reads them, the OUT FIFO and the pending events.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    vec::Vec,
};

use crate::{EndpointRegisters, Events, InterruptRegisters, PACKET};

/// The block's state, shared by the task's and the handler's registers.
#[derive(Default)]
pub(crate) struct Block {
    /// Bytes in the IN FIFO, not yet handed over.
    pub(crate) fifo: Vec<u8>,
    /// Whether a handed-over packet waits for the host.
    pub(crate) in_flight: bool,
    /// Packets the host read, a zero-length one empty.
    pub(crate) packets: Vec<Vec<u8>>,
    /// Bytes waiting in the OUT FIFO.
    pub(crate) out: VecDeque<u8>,
    /// Pending events.
    pub(crate) events: Events,
    /// Whether the task enabled the interrupts.
    pub(crate) enabled: bool,
    /// Every acknowledgement the handler wrote.
    pub(crate) acknowledged: Vec<Events>,
}

/// The block as both sides of the fake see it.
#[derive(Clone, Default)]
pub(crate) struct Shared(Arc<Mutex<Block>>);

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Block> {
        self.0.lock().expect("the fake block's lock")
    }
}

impl Block {
    /// The host reads the packet in flight: the FIFO empties and an
    /// IN-empty event is pending.
    pub(crate) fn host_reads(&mut self) {
        if self.in_flight {
            let packet = core::mem::take(&mut self.fifo);
            self.packets.push(packet);
            self.in_flight = false;
            self.events.in_empty = true;
        }
    }

    /// The host sends `bytes` as one packet.
    pub(crate) fn host_sends(&mut self, bytes: &[u8]) {
        self.out.extend(bytes);
        self.events.out_received = true;
    }
}

pub(crate) struct Task(pub(crate) Shared);

impl EndpointRegisters for Task {
    type Interrupt = Irq;

    fn has_room(&self) -> bool {
        let block = self.0.lock();
        !block.in_flight && block.fifo.len() < PACKET
    }

    fn push(&mut self, byte: u8) {
        let mut block = self.0.lock();
        assert!(
            !block.in_flight && block.fifo.len() < PACKET,
            "push without room"
        );
        block.fifo.push(byte);
    }

    fn send(&mut self) {
        self.0.lock().in_flight = true;
    }

    fn has_data(&self) -> bool {
        !self.0.lock().out.is_empty()
    }

    fn pop(&mut self) -> u8 {
        self.0
            .lock()
            .out
            .pop_front()
            .expect("pop with data available")
    }

    fn enable_events(
        &mut self,
        _authority: &impl oer_espressif_interrupt_table::EventAuthority<Self::Interrupt>,
    ) {
        self.0.lock().enabled = true;
    }
}

pub(crate) struct Irq(pub(crate) Shared);

impl oer_espressif_interrupt_table::InterruptSource for Irq {
    // Synthetic source in the host controller, independent of chip identities.
    const SOURCE: u16 = 0;
}

oer_espressif_interrupt_table::__fake_controller!();

/// A real router-issued proof over the fake's unrouted source.
pub(crate) fn silent() -> oer_espressif_interrupt_table::Silent<Irq> {
    oer_espressif_interrupt_table::Router::install(FakeController::new(0), &[])
        .and_then(|router| router.silent())
        .expect("the empty table leaves the fake USB source silent")
}

impl InterruptRegisters for Irq {
    fn take_events(&mut self) -> Events {
        let mut block = self.0.lock();
        let events = core::mem::take(&mut block.events);
        block.acknowledged.push(events);
        events
    }
}
