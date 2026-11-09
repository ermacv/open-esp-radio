//! One console write as a bounded loop over `write`: every step either
//! advances through the bytes by checked slicing or ends in a typed error,
//! without the panic paths of `write_all` (its `Ok(0)` panic and its
//! unchecked slice).

/// Why the console did not take every byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConsoleWriteError {
    /// The endpoint refused the write.
    Refused,
    /// The endpoint took no byte of a non-empty write.
    Stalled,
    /// The endpoint reported more bytes than it was given.
    Overrun,
}

/// Write all of `bytes` to `writer`.
pub(crate) async fn write_bytes<W: embedded_io_async::Write>(
    writer: &mut W,
    bytes: &[u8],
) -> Result<(), ConsoleWriteError> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let written = writer
            .write(rest)
            .await
            .map_err(|_| ConsoleWriteError::Refused)?;
        if written == 0 {
            return Err(ConsoleWriteError::Stalled);
        }
        rest = rest.get(written..).ok_or(ConsoleWriteError::Overrun)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    fn ready<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the test writer never waits"),
        }
    }

    /// Takes at most `chunk` bytes per write and reports `report(taken)`.
    struct Writer {
        taken: Vec<u8>,
        chunk: usize,
        report: fn(usize) -> usize,
        refuse: bool,
    }

    impl embedded_io_async::ErrorType for Writer {
        type Error = embedded_io_async::ErrorKind;
    }

    impl embedded_io_async::Write for Writer {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            if self.refuse {
                return Err(embedded_io_async::ErrorKind::Other);
            }
            let take = bytes.len().min(self.chunk);
            self.taken.extend(bytes.iter().take(take));
            Ok((self.report)(take))
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn writer(chunk: usize, report: fn(usize) -> usize) -> Writer {
        Writer {
            taken: Vec::new(),
            chunk,
            report,
            refuse: false,
        }
    }

    #[test]
    fn short_writes_continue_until_every_byte_is_taken() {
        let mut console = writer(3, |taken| taken);
        assert_eq!(ready(write_bytes(&mut console, b"REBIND boot\n")), Ok(()));
        assert_eq!(console.taken, b"REBIND boot\n");
    }

    #[test]
    fn a_write_that_takes_nothing_is_an_error_not_a_panic() {
        let mut console = writer(0, |taken| taken);
        assert_eq!(
            ready(write_bytes(&mut console, b"x")),
            Err(ConsoleWriteError::Stalled)
        );
    }

    #[test]
    fn a_write_reporting_more_than_it_was_given_is_an_error() {
        let mut console = writer(8, |taken| taken + 1);
        assert_eq!(
            ready(write_bytes(&mut console, b"abc")),
            Err(ConsoleWriteError::Overrun)
        );
    }

    #[test]
    fn a_refused_write_is_an_error_and_an_empty_one_writes_nothing() {
        let mut console = writer(8, |taken| taken);
        console.refuse = true;
        assert_eq!(
            ready(write_bytes(&mut console, b"abc")),
            Err(ConsoleWriteError::Refused)
        );
        let mut console = writer(0, |taken| taken);
        assert_eq!(ready(write_bytes(&mut console, b"")), Ok(()));
    }
}
