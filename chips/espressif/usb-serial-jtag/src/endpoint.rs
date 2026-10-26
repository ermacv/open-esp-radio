//! The console's duplex endpoint: one task reads and writes the CDC-ACM
//! FIFOs and waits for the interrupt's events on its static signal.
//!
//! A write fills the IN FIFO while it has room, at most one packet, hands it
//! to the host and waits until the host read it: a new IN-empty event after
//! the hand-over and room in the FIFO. A write that ends on a full packet
//! sends a zero-length packet after it, which ends the host's transfer. A
//! read takes what the OUT FIFO holds and otherwise waits for the next
//! packet. Events the task took but did not need yet stay in the endpoint,
//! so dropping a pending read or write loses none.

use core::future::poll_fn;
use core::task::Poll;

use oer_interrupt_table::EventAuthority;
use oer_interrupt_table::signal::{IrqSignal, SignalExecutor};

use crate::{EndpointRegisters, IN_EMPTY, OUT_RECEIVED, PACKET};

#[cfg(test)]
mod tests;

/// The endpoint ran outside the executor its signal belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    ForeignContext,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("the console's endpoint ran outside its signal's executor")
    }
}

impl core::error::Error for Error {}

impl embedded_io_async::Error for Error {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

/// The console's endpoint over the task's registers `R`, woken by the
/// interrupt's signal of executor `E`.
pub struct Endpoint<E: SignalExecutor, R> {
    registers: R,
    signal: &'static IrqSignal<E>,
    /// Facts taken from the signal and not yet used.
    facts: u32,
}

impl<E: SignalExecutor, R: EndpointRegisters> Endpoint<E, R> {
    /// The endpoint over `registers`, which the interrupt that raises
    /// `signal` wakes: it enables both console interrupts with the part's
    /// binding or silence proof.
    pub fn new(
        mut registers: R,
        signal: &'static IrqSignal<E>,
        authority: &impl EventAuthority<R::Interrupt>,
    ) -> Self {
        registers.enable_events(authority);
        Self {
            registers,
            signal,
            facts: 0,
        }
    }

    /// Moves the facts raised so far into the endpoint.
    fn take_facts(&mut self) -> Result<(), Error> {
        self.facts |= self.signal.take().map_err(|_| Error::ForeignContext)?;
        Ok(())
    }

    /// Waits for `fact`, then uses it up.
    async fn wait(&mut self, fact: u32) -> Result<(), Error> {
        poll_fn(|cx| {
            loop {
                if self.facts & fact != 0 {
                    self.facts &= !fact;
                    return Poll::Ready(Ok(()));
                }
                match self.signal.poll_take(cx) {
                    Poll::Ready(Ok(facts)) => self.facts |= facts,
                    Poll::Ready(Err(_)) => return Poll::Ready(Err(Error::ForeignContext)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        })
        .await
    }

    /// Hands the IN FIFO to the host and waits until the host read it.
    async fn send(&mut self) -> Result<(), Error> {
        // IN-empty is an event: only one after this hand-over completes it.
        self.take_facts()?;
        self.facts &= !IN_EMPTY;
        self.registers.send();
        loop {
            self.wait(IN_EMPTY).await?;
            // Room tells the host read the packet; the event alone may be
            // the one of an earlier packet that came late.
            if self.registers.has_room() {
                return Ok(());
            }
        }
    }

    /// Takes what the OUT FIFO holds, up to `buffer`'s length.
    fn drain(&mut self, buffer: &mut [u8]) -> usize {
        let mut length = 0;
        for slot in buffer.iter_mut() {
            if !self.registers.has_data() {
                break;
            }
            *slot = self.registers.pop();
            length += 1;
        }
        length
    }
}

impl<E: SignalExecutor, R> embedded_io_async::ErrorType for Endpoint<E, R> {
    type Error = Error;
}

impl<E: SignalExecutor, R: EndpointRegisters> embedded_io_async::Write for Endpoint<E, R> {
    /// Writes all of `buffer`, packet by packet, and returns once the host
    /// read the last one.
    async fn write(&mut self, buffer: &[u8]) -> Result<usize, Error> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut rest = buffer;
        loop {
            let mut packet = 0;
            while packet < PACKET
                && self.registers.has_room()
                && let Some((&byte, tail)) = rest.split_first()
            {
                self.registers.push(byte);
                rest = tail;
                packet += 1;
            }
            self.send().await?;
            if rest.is_empty() {
                if packet == PACKET {
                    // A full packet leaves the host's transfer open.
                    self.send().await?;
                }
                return Ok(buffer.len());
            }
        }
    }

    /// Every write already waited for the host.
    async fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

impl<E: SignalExecutor, R: EndpointRegisters> embedded_io_async::Read for Endpoint<E, R> {
    /// Reads what the OUT FIFO holds; with nothing there, waits for a packet.
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let length = self.drain(buffer);
            if length > 0 {
                return Ok(length);
            }
            self.wait(OUT_RECEIVED).await?;
        }
    }
}
