//! Notes on the console's writer: a line reaches the host whole and in
//! order, and a refused line reports the writer's error.

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, vec::Vec};

use embedded_io_async::ErrorType;
use oer_diagnostics::Note;
use oer_interrupt_table::signal::host::Host;

use super::{NoteError, write_note};
use crate::{
    Endpoint, Interrupt,
    fake::{Irq, Shared, Task, silent},
};

fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// A writer that keeps what it was given, or refuses it.
struct Sink {
    written: Vec<u8>,
    refuse: bool,
}

impl ErrorType for Sink {
    type Error = embedded_io_async::ErrorKind;
}

impl embedded_io_async::Write for Sink {
    async fn write(&mut self, buffer: &[u8]) -> Result<usize, Self::Error> {
        if self.refuse {
            return Err(embedded_io_async::ErrorKind::Other);
        }
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn a_note_is_written_as_one_whole_line() {
    let mut sink = Sink {
        written: Vec::new(),
        refuse: false,
    };
    assert_eq!(
        poll(pin!(write_note(&mut sink, &Note::w3_pass()))),
        Poll::Ready(Ok(()))
    );
    assert!(sink.written.starts_with(b"\r\nOER-DIAG:01"));
    assert!(sink.written.ends_with(b"\r\n"));
}

#[test]
fn a_refused_line_reports_the_writer_error() {
    let mut sink = Sink {
        written: Vec::new(),
        refuse: true,
    };
    assert_eq!(
        poll(pin!(write_note(&mut sink, &Note::w3_pass()))),
        Poll::Ready(Err(NoteError::Write(embedded_io_async::ErrorKind::Other)))
    );
    assert!(sink.written.is_empty());
}

#[test]
fn a_note_over_the_endpoint_waits_for_the_host_and_arrives_whole() {
    let block = Shared::default();
    let interrupt: &'static Interrupt<Host, Irq> = Box::leak(Box::new(Interrupt::for_host_test()));
    let mut endpoint = Endpoint::new(Task(block.clone()), interrupt.signal(), &silent());
    let note = Note::w3_pass();
    let mut write = pin!(write_note(&mut endpoint, &note));
    loop {
        match poll(write.as_mut()) {
            Poll::Ready(result) => {
                assert_eq!(result, Ok(()));
                break;
            }
            Poll::Pending => {
                block.lock().host_reads();
                interrupt.service(&mut Irq(block.clone()));
            }
        }
    }
    let received: Vec<u8> = block.lock().packets.iter().flatten().copied().collect();
    assert!(received.starts_with(b"\r\nOER-DIAG:01"));
    assert!(received.ends_with(b"\r\n"));
}
