//! One descriptor plus explicit command, shutdown and cancellation notifications.

use mio::{Events, Interest, Poll, Token, Waker, unix::SourceFd};
use std::{
    io,
    os::fd::AsRawFd,
    sync::Arc,
    time::{Duration, Instant},
};

const IO: Token = Token(0);
const CONTROL: Token = Token(1);

pub struct EventPoll {
    poll: Poll,
    events: Events,
    wake: Arc<Waker>,
    _cancellation: oer_process::CancellationNotification,
}

#[derive(Default)]
pub struct Ready {
    pub readable: bool,
    pub writable: bool,
}

impl EventPoll {
    pub fn new() -> io::Result<Self> {
        let poll = Poll::new()?;
        let wake = Arc::new(Waker::new(poll.registry(), CONTROL)?);
        let notify = Arc::clone(&wake);
        let cancellation = oer_process::notify_on_cancel(move || {
            let _ = notify.wake();
        });
        Ok(Self {
            poll,
            events: Events::with_capacity(8),
            wake,
            _cancellation: cancellation,
        })
    }

    pub fn waker(&self) -> Arc<Waker> {
        Arc::clone(&self.wake)
    }

    pub fn register(&self, io: &impl AsRawFd, writable: bool) -> io::Result<()> {
        self.poll
            .registry()
            .register(&mut SourceFd(&io.as_raw_fd()), IO, interest(writable))
    }

    pub fn interest(&self, io: &impl AsRawFd, writable: bool) -> io::Result<()> {
        self.poll
            .registry()
            .reregister(&mut SourceFd(&io.as_raw_fd()), IO, interest(writable))
    }

    pub fn wait(&mut self, deadline: Option<Instant>) -> io::Result<Ready> {
        let timeout = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        match self.poll.poll(&mut self.events, timeout) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(Ready::default()),
            result => result?,
        }
        let mut ready = Ready::default();
        for event in &self.events {
            if event.token() == IO {
                ready.readable |= event.is_readable() || event.is_read_closed() || event.is_error();
                ready.writable |=
                    event.is_writable() || event.is_write_closed() || event.is_error();
            }
        }
        Ok(ready)
    }
}

fn interest(writable: bool) -> Interest {
    if writable {
        Interest::READABLE.add(Interest::WRITABLE)
    } else {
        Interest::READABLE
    }
}

/// A deadline bounds failure detection; it never establishes successful progress.
pub fn deadline_after(timeout: Duration) -> Instant {
    let deadline = Instant::now() + timeout;
    oer_process::cleanup_deadline().map_or(deadline, |cleanup| deadline.min(cleanup))
}
