//! The endpoint over the fake block and the host signal executor: a write
//! completes only after the host read its packets, a read waits for a
//! packet, and an abandoned wait loses no event.

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, vec, vec::Vec};

use embedded_io_async::{Read, Write};
use oer_espressif_interrupt_table::signal::host::{Host, foreign_signal};

use super::{Endpoint, Error};
use crate::{
    Events, Interrupt, PACKET,
    fake::{Irq, Shared, Task, silent},
};

/// The block, the endpoint over it and its interrupt.
struct Console {
    block: Shared,
    endpoint: Endpoint<Host, Task>,
    interrupt: &'static Interrupt<Host, Irq>,
}

impl Console {
    fn new() -> Self {
        let block = Shared::default();
        let interrupt: &'static Interrupt<Host, Irq> =
            Box::leak(Box::new(Interrupt::for_host_test()));
        let endpoint = Endpoint::new(Task(block.clone()), interrupt.signal(), &silent());
        Self {
            block,
            endpoint,
            interrupt,
        }
    }
}

/// The host reads the packet in flight, and the interrupt fires.
fn host_reads(block: &Shared, interrupt: &'static Interrupt<Host, Irq>) {
    block.lock().host_reads();
    interrupt.service(&mut Irq(block.clone()));
}

fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn the_endpoint_enables_the_console_interrupts_with_silent_authority() {
    let block = Shared::default();
    assert!(!block.lock().enabled);
    let interrupt: &'static Interrupt<Host, Irq> = Box::leak(Box::new(Interrupt::for_host_test()));
    let authority = silent();
    assert!(!block.lock().enabled, "the proof alone enables no events");
    let _endpoint = Endpoint::new(Task(block.clone()), interrupt.signal(), &authority);
    assert!(block.lock().enabled);
}

#[test]
fn a_write_completes_only_after_the_host_read_its_packet() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    let mut write = pin!(endpoint.write(b"hello"));
    assert!(
        poll(write.as_mut()).is_pending(),
        "the host has not read yet"
    );
    assert!(block.lock().in_flight, "the packet was handed over");
    host_reads(&block, interrupt);
    assert_eq!(poll(write.as_mut()), Poll::Ready(Ok(5)));
    assert_eq!(block.lock().packets, vec![b"hello".to_vec()]);
}

#[test]
fn an_event_from_before_the_hand_over_completes_nothing() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    // IN-empty is set after reset: the event is pending before any write.
    block.lock().events.in_empty = true;
    interrupt.service(&mut Irq(block.clone()));
    let mut write = pin!(endpoint.write(b"ok"));
    assert!(
        poll(write.as_mut()).is_pending(),
        "a stale event is not the host's read"
    );
    host_reads(&block, interrupt);
    assert_eq!(poll(write.as_mut()), Poll::Ready(Ok(2)));
}

#[test]
fn an_event_without_room_keeps_the_write_waiting() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    let mut write = pin!(endpoint.write(b"wait"));
    assert!(poll(write.as_mut()).is_pending());
    // An IN-empty of an earlier packet arrives late: the FIFO is still full.
    block.lock().events.in_empty = true;
    interrupt.service(&mut Irq(block.clone()));
    assert!(poll(write.as_mut()).is_pending(), "no room yet");
    host_reads(&block, interrupt);
    assert_eq!(poll(write.as_mut()), Poll::Ready(Ok(4)));
}

#[test]
fn a_long_write_goes_out_packet_by_packet() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    let bytes: Vec<u8> = (0..100).collect();
    let mut write = pin!(endpoint.write(&bytes));
    assert!(poll(write.as_mut()).is_pending());
    host_reads(&block, interrupt);
    assert!(
        poll(write.as_mut()).is_pending(),
        "the second packet is out"
    );
    host_reads(&block, interrupt);
    assert_eq!(poll(write.as_mut()), Poll::Ready(Ok(100)));
    let packets = block.lock().packets.clone();
    assert_eq!(
        packets,
        vec![bytes[..PACKET].to_vec(), bytes[PACKET..].to_vec()]
    );
}

#[test]
fn a_write_ending_on_a_full_packet_ends_the_transfer_with_an_empty_one() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    let bytes = [0x5a; PACKET];
    let mut write = pin!(endpoint.write(&bytes));
    assert!(poll(write.as_mut()).is_pending());
    host_reads(&block, interrupt);
    assert!(
        poll(write.as_mut()).is_pending(),
        "the zero-length packet is out"
    );
    host_reads(&block, interrupt);
    assert_eq!(poll(write.as_mut()), Poll::Ready(Ok(PACKET)));
    assert_eq!(block.lock().packets, vec![bytes.to_vec(), Vec::new()]);
}

#[test]
fn a_read_takes_what_the_host_sent_and_waits_for_more() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    let mut buffer = [0; 8];
    {
        let mut read = pin!(endpoint.read(&mut buffer));
        assert!(poll(read.as_mut()).is_pending(), "nothing was sent");
        block.lock().host_sends(b"abc");
        interrupt.service(&mut Irq(block.clone()));
        assert_eq!(poll(read.as_mut()), Poll::Ready(Ok(3)));
    }
    assert_eq!(&buffer[..3], b"abc");
}

#[test]
fn a_short_buffer_leaves_the_rest_for_the_next_read() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    block.lock().host_sends(b"abcdef");
    interrupt.service(&mut Irq(block.clone()));
    let mut buffer = [0; 4];
    assert_eq!(poll(pin!(endpoint.read(&mut buffer))), Poll::Ready(Ok(4)));
    assert_eq!(poll(pin!(endpoint.read(&mut buffer))), Poll::Ready(Ok(2)));
    assert_eq!(&buffer[..2], b"ef");
}

#[test]
fn an_abandoned_read_loses_no_byte_and_no_event() {
    let Console {
        block,
        mut endpoint,
        interrupt,
    } = Console::new();
    let mut buffer = [0; 8];
    assert!(poll(pin!(endpoint.read(&mut buffer))).is_pending());
    block.lock().host_sends(b"hi");
    interrupt.service(&mut Irq(block.clone()));
    // The select that polled the read chose another branch and dropped it.
    {
        let mut write = pin!(endpoint.write(b"x"));
        assert!(poll(write.as_mut()).is_pending());
        host_reads(&block, interrupt);
        assert_eq!(poll(write.as_mut()), Poll::Ready(Ok(1)));
    }
    let mut buffer = [0; 8];
    assert_eq!(poll(pin!(endpoint.read(&mut buffer))), Poll::Ready(Ok(2)));
    assert_eq!(&buffer[..2], b"hi");
}

#[test]
fn an_endpoint_outside_its_executor_refuses_to_wait() {
    let block = Shared::default();
    let mut endpoint = Endpoint::new(Task(block.clone()), foreign_signal(), &silent());
    assert_eq!(
        poll(pin!(endpoint.write(b"x"))),
        Poll::Ready(Err(Error::ForeignContext))
    );
    let mut buffer = [0; 1];
    assert_eq!(
        poll(pin!(endpoint.read(&mut buffer))),
        Poll::Ready(Err(Error::ForeignContext))
    );
}

#[test]
fn the_handler_raises_only_what_it_acknowledged() {
    let Console {
        block, interrupt, ..
    } = Console::new();
    block.lock().host_sends(b"z");
    interrupt.service(&mut Irq(block.clone()));
    interrupt.service(&mut Irq(block.clone()));
    assert_eq!(
        block.lock().acknowledged,
        vec![
            Events {
                in_empty: false,
                out_received: true
            },
            Events::NONE
        ]
    );
}
