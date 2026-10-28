//! Each response is one frame written whole: failure or cancellation cannot
//! offset the next.
use core::{future::poll_fn, pin::pin, task::Poll};

use embedded_io_async::Write;
use oer_bluetooth_hci::{HciControllerResponse, h4};

/// The longest frame: the H4 indicator, an event's two header bytes and its
/// 255 parameter bytes.
pub const MAX_FRAME: usize = 1 + 2 + 255;

#[derive(Debug, Eq, PartialEq)]
pub struct Failed;

/// The response as one H4 frame, or `None` when it outgrows [`MAX_FRAME`].
fn frame(response: &impl HciControllerResponse) -> Option<([u8; MAX_FRAME], usize)> {
    let mut frame = [0; MAX_FRAME];
    let length = h4::write(response.kind(), response.as_bytes(), &mut frame).ok()?;
    Some((frame, length))
}

/// Writes `response` and returns once the host read it. The frame goes to
/// the writer in one call, so a dropped `send` leaves the next response to
/// start at its indicator whenever the writer takes a call whole or not at
/// all.
pub async fn send(
    writer: &mut impl Write,
    response: &impl HciControllerResponse,
) -> Result<(), Failed> {
    let (frame, length) = frame(response).ok_or(Failed)?;
    let bytes = frame.get(..length).ok_or(Failed)?;
    writer.write_all(bytes).await.map_err(|_| Failed)
}

/// Stop authority is issued before touching the host. The frame is offered
/// to the writer once and never waited for: no retry or wait can delay
/// returning it, even when the host never reads another byte. The writer
/// takes what its FIFO has room for; the rest is dropped.
pub async fn fail_stop<R>(
    stop: impl FnOnce() -> R,
    writer: &mut impl Write,
    response: &impl HciControllerResponse,
) -> R {
    let stopped = stop();
    if let Some((frame, length)) = frame(response)
        && let Some(bytes) = frame.get(..length)
    {
        let mut write = pin!(writer.write_all(bytes));
        // One poll hands the frame over; dropping the write abandons the wait
        // for the host.
        poll_fn(|cx| {
            let _ = write.as_mut().poll(cx);
            Poll::Ready(())
        })
        .await;
    }
    stopped
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        convert::Infallible,
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embedded_io_async::{ErrorKind, ErrorType};
    use oer_bluetooth_hci::bt_hci::PacketKind;
    use std::vec::Vec;

    struct Response;
    impl HciControllerResponse for Response {
        fn kind(&self) -> PacketKind {
            PacketKind::Event
        }
        fn as_bytes(&self) -> &[u8] {
            &[0x10, 1, 9]
        }
    }

    /// A writer that takes a call whole, refuses it, or never completes it.
    struct Output<'a> {
        bytes: Vec<u8>,
        refuse: bool,
        blocked: bool,
        stopped: Option<&'a Cell<bool>>,
    }
    impl ErrorType for Output<'_> {
        type Error = ErrorKind;
    }
    impl Write for Output<'_> {
        async fn write(&mut self, buffer: &[u8]) -> Result<usize, ErrorKind> {
            if let Some(stopped) = self.stopped {
                assert!(stopped.get());
            }
            if self.blocked {
                core::future::pending::<Infallible>().await;
            }
            if self.refuse {
                return Err(ErrorKind::Other);
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        async fn flush(&mut self) -> Result<(), ErrorKind> {
            Ok(())
        }
    }
    fn output() -> Output<'static> {
        Output {
            bytes: Vec::new(),
            refuse: false,
            blocked: false,
            stopped: None,
        }
    }
    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn a_response_is_one_frame_with_its_indicator() {
        let mut sink = output();
        assert_eq!(poll(pin!(send(&mut sink, &Response))), Poll::Ready(Ok(())));
        assert_eq!(sink.bytes, [4, 0x10, 1, 9]);
    }

    #[test]
    fn a_refused_frame_does_not_offset_the_next_one() {
        let mut sink = output();
        sink.refuse = true;
        assert_eq!(
            poll(pin!(send(&mut sink, &Response))),
            Poll::Ready(Err(Failed))
        );
        sink.refuse = false;
        assert_eq!(poll(pin!(send(&mut sink, &Response))), Poll::Ready(Ok(())));
        assert_eq!(sink.bytes, [4, 0x10, 1, 9]);
    }

    #[test]
    fn cancellation_under_backpressure_starts_the_next_response_at_its_kind() {
        let mut sink = output();
        sink.blocked = true;
        {
            let mut send = pin!(send(&mut sink, &Response));
            assert!(poll(send.as_mut()).is_pending());
        }
        sink.blocked = false;
        assert_eq!(poll(pin!(send(&mut sink, &Response))), Poll::Ready(Ok(())));
        assert_eq!(sink.bytes, [4, 0x10, 1, 9]);
    }

    #[test]
    fn a_response_longer_than_a_frame_is_refused_unwritten() {
        struct Long([u8; MAX_FRAME]);
        impl HciControllerResponse for Long {
            fn kind(&self) -> PacketKind {
                PacketKind::Event
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
        }
        let mut sink = output();
        assert_eq!(
            poll(pin!(send(&mut sink, &Long([0; MAX_FRAME])))),
            Poll::Ready(Err(Failed))
        );
        assert!(sink.bytes.is_empty());
    }

    #[test]
    fn a_full_fifo_cannot_delay_or_precede_fail_stop() {
        let stopped = Cell::new(false);
        let mut sink = Output {
            stopped: Some(&stopped),
            blocked: true,
            ..output()
        };
        assert_eq!(
            poll(pin!(fail_stop(
                || {
                    stopped.set(true);
                    42
                },
                &mut sink,
                &Response
            ))),
            Poll::Ready(42)
        );
        assert!(stopped.get());
        assert!(sink.bytes.is_empty());
    }

    #[test]
    fn fail_stop_offers_its_frame_when_the_host_listens() {
        let mut sink = output();
        assert_eq!(
            poll(pin!(fail_stop(|| 7, &mut sink, &Response))),
            Poll::Ready(7)
        );
        assert_eq!(sink.bytes, [4, 0x10, 1, 9]);
    }
}
