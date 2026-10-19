//! Transport completion for an already encoded wire frame.

/// Write all frame bytes and flush the transport before reporting completion.
///
/// In particular, a USB transfer ending on a full packet needs its terminating
/// packet even when all bytes were accepted. The caller owns one deadline for
/// both operations; cancellation leaves delivery uncertain.
pub async fn write_frame<W: embedded_io_async::Write>(
    writer: &mut W,
    frame: &[u8],
) -> Result<(), W::Error> {
    writer.write_all(frame).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    use embedded_io_async::{ErrorKind, ErrorType, Write};

    #[derive(Default)]
    struct BufferedTransport {
        pending: heapless::Vec<u8, 256>,
        delivered: heapless::Vec<u8, 256>,
        fail_write: bool,
        fail_flush: bool,
        flushed: bool,
    }

    impl ErrorType for BufferedTransport {
        type Error = ErrorKind;
    }

    impl Write for BufferedTransport {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            if self.fail_write {
                return Err(ErrorKind::BrokenPipe);
            }
            let count = bytes.len().min(17);
            self.pending.extend_from_slice(&bytes[..count]).unwrap();
            Ok(count)
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            self.flushed = true;
            if self.fail_flush {
                return Err(ErrorKind::NotConnected);
            }
            core::mem::swap(&mut self.pending, &mut self.delivered);
            Ok(())
        }
    }

    fn ready<T>(future: impl Future<Output = T>) -> T {
        match core::pin::pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("test transport unexpectedly blocked"),
        }
    }

    #[test]
    fn full_and_short_packets_are_delivered_without_a_following_frame() {
        for length in [63, 64, 65, 128] {
            let frame = [0xa5; 128];
            let mut transport = BufferedTransport::default();
            ready(write_frame(&mut transport, &frame[..length])).unwrap();
            assert_eq!(transport.delivered.as_slice(), &frame[..length]);
            assert!(transport.pending.is_empty());
        }
    }

    #[test]
    fn write_and_flush_failures_cannot_report_delivery() {
        let mut transport = BufferedTransport {
            fail_write: true,
            ..Default::default()
        };
        assert_eq!(
            ready(write_frame(&mut transport, &[1; 64])),
            Err(ErrorKind::BrokenPipe)
        );
        assert!(!transport.flushed);
        transport.fail_write = false;
        transport.fail_flush = true;
        assert_eq!(
            ready(write_frame(&mut transport, &[1; 64])),
            Err(ErrorKind::NotConnected)
        );
        assert!(transport.delivered.is_empty());
    }
}
