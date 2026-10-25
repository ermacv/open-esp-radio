//! H4 commands from the host's reader: what one read brings is kept until
//! the commands take it, and an unfinished command expires after its gap.
use embassy_futures::select::{Either, select};
use embedded_io_async::Read;
use oer_time::Timer;

use crate::h4::Commands;

/// The bytes of one read: a full-speed bulk packet.
const READ: usize = 64;

/// Bytes read from the host and not yet taken by the commands. A read can
/// bring more than one command; the rest waits for the next [`receive`].
pub struct Inbox {
    bytes: [u8; READ],
    next: usize,
    end: usize,
}

impl Inbox {
    pub const fn new() -> Self {
        Self {
            bytes: [0; READ],
            next: 0,
            end: 0,
        }
    }

    fn take(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.next..self.end)?.first()?;
        self.next += 1;
        Some(byte)
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`receive`] ended with.
#[derive(Debug, Eq, PartialEq)]
pub enum Received {
    /// `commands` holds one complete command.
    Command,
    /// A stray byte or an expired frame: `commands` was reset.
    Framing,
}

/// The host's reader failed or ended.
#[derive(Debug, Eq, PartialEq)]
pub enum Error<E> {
    Read(E),
    /// The reader returned no byte for a non-empty buffer.
    Closed,
}

/// Reads until `commands` holds a complete command, or its framing broke.
/// While a command is unfinished the wait for the host ends at its deadline.
///
/// # Errors
/// The reader failed or ended.
pub async fn receive<R: Read>(
    reader: &mut R,
    inbox: &mut Inbox,
    commands: &mut Commands,
    timer: &impl Timer,
) -> Result<Received, Error<R::Error>> {
    while !commands.complete() {
        if commands.expire(timer.now()) {
            return Ok(Received::Framing);
        }
        let Some(byte) = inbox.take() else {
            let length = match commands.deadline() {
                Some(deadline) => {
                    match select(reader.read(&mut inbox.bytes), timer.wait_until(deadline)).await {
                        Either::First(read) => read.map_err(Error::Read)?,
                        // The loop's expiry check names the broken frame.
                        Either::Second(()) => continue,
                    }
                }
                None => reader.read(&mut inbox.bytes).await.map_err(Error::Read)?,
            };
            if length == 0 {
                return Err(Error::Closed);
            }
            inbox.next = 0;
            inbox.end = length.min(READ);
            continue;
        };
        if commands.push(byte, timer.now()).is_err() {
            commands.reset();
            return Ok(Received::Framing);
        }
    }
    Ok(Received::Command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h4::FRAME_GAP;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embedded_io_async::{ErrorKind, ErrorType};
    use oer_time_virtual::VirtualClock;
    use std::{collections::VecDeque, vec::Vec};

    /// A host that sends its chunks one read at a time, then stays silent.
    struct Host(VecDeque<Vec<u8>>);
    impl ErrorType for Host {
        type Error = ErrorKind;
    }
    impl Read for Host {
        async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, ErrorKind> {
            match self.0.pop_front() {
                Some(chunk) => {
                    buffer[..chunk.len()].copy_from_slice(&chunk);
                    Ok(chunk.len())
                }
                None => core::future::pending().await,
            }
        }
    }
    fn host(chunks: &[&[u8]]) -> Host {
        Host(chunks.iter().map(|chunk| chunk.to_vec()).collect())
    }
    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn a_command_split_over_reads_completes_when_its_last_byte_arrives() {
        let clock = VirtualClock::<1>::new();
        let mut host = host(&[&[1, 3], &[12, 0]]);
        let (mut inbox, mut commands) = (Inbox::new(), Commands::new());
        assert_eq!(
            poll(pin!(receive(&mut host, &mut inbox, &mut commands, &clock))),
            Poll::Ready(Ok(Received::Command))
        );
        assert!(commands.packet().is_ok());
    }

    #[test]
    fn the_bytes_after_a_command_wait_for_the_next_receive() {
        let clock = VirtualClock::<1>::new();
        let mut host = host(&[&[1, 3, 12, 0, 1, 3, 12, 0]]);
        let (mut inbox, mut commands) = (Inbox::new(), Commands::new());
        for _ in 0..2 {
            assert_eq!(
                poll(pin!(receive(&mut host, &mut inbox, &mut commands, &clock))),
                Poll::Ready(Ok(Received::Command))
            );
            commands.consume().expect("a complete command");
        }
    }

    #[test]
    fn a_stray_byte_is_a_framing_error_and_the_next_command_survives() {
        let clock = VirtualClock::<1>::new();
        let mut host = host(&[&[2, 1, 3, 12, 0]]);
        let (mut inbox, mut commands) = (Inbox::new(), Commands::new());
        assert_eq!(
            poll(pin!(receive(&mut host, &mut inbox, &mut commands, &clock))),
            Poll::Ready(Ok(Received::Framing))
        );
        assert_eq!(
            poll(pin!(receive(&mut host, &mut inbox, &mut commands, &clock))),
            Poll::Ready(Ok(Received::Command))
        );
    }

    #[test]
    fn a_silent_host_ends_an_unfinished_command_at_its_gap() {
        let clock = VirtualClock::<2>::new();
        let mut host = host(&[&[1, 3]]);
        let (mut inbox, mut commands) = (Inbox::new(), Commands::new());
        {
            let mut receiving = pin!(receive(&mut host, &mut inbox, &mut commands, &clock));
            assert!(poll(receiving.as_mut()).is_pending());
            assert!(clock.next_deadline().is_some(), "the gap is waited on");
            clock.advance(FRAME_GAP).expect("representable gap");
            assert_eq!(poll(receiving.as_mut()), Poll::Ready(Ok(Received::Framing)));
        }
        host.0.push_back(std::vec![1, 3, 12, 0]);
        assert_eq!(
            poll(pin!(receive(&mut host, &mut inbox, &mut commands, &clock))),
            Poll::Ready(Ok(Received::Command))
        );
    }

    #[test]
    fn an_idle_host_is_waited_for_without_a_deadline() {
        let clock = VirtualClock::<1>::new();
        let mut host = host(&[]);
        let (mut inbox, mut commands) = (Inbox::new(), Commands::new());
        let mut receiving = pin!(receive(&mut host, &mut inbox, &mut commands, &clock));
        assert!(poll(receiving.as_mut()).is_pending());
        assert!(clock.next_deadline().is_none());
    }

    #[test]
    fn a_reader_that_ends_is_reported() {
        struct Ended;
        impl ErrorType for Ended {
            type Error = ErrorKind;
        }
        impl Read for Ended {
            async fn read(&mut self, _: &mut [u8]) -> Result<usize, ErrorKind> {
                Ok(0)
            }
        }
        let clock = VirtualClock::<1>::new();
        let (mut inbox, mut commands) = (Inbox::new(), Commands::new());
        assert_eq!(
            poll(pin!(receive(&mut Ended, &mut inbox, &mut commands, &clock))),
            Poll::Ready(Err(Error::Closed))
        );
    }
}
