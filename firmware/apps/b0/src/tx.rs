//! Each response owns its cursor; failure or cancellation cannot offset the next.
use oer_bluetooth_hci::HciControllerResponse;
use oer_time::{Duration, Timer};

pub trait Sink {
    type Error;
    fn write(&mut self, byte: u8) -> nb::Result<(), Self::Error>;
    fn flush(&mut self) -> nb::Result<(), Self::Error>;
}
#[derive(Debug, Eq, PartialEq)]
pub struct Failed;

pub async fn send(
    sink: &mut impl Sink,
    timer: &impl Timer,
    poll: Duration,
    response: &impl HciControllerResponse,
) -> Result<(), Failed> {
    for byte in core::iter::once(response.kind() as u8).chain(response.as_bytes().iter().copied()) {
        loop {
            match sink.write(byte) {
                Ok(()) => break,
                Err(nb::Error::WouldBlock) => timer.wait_for(poll).await.map_err(|_| Failed)?,
                Err(nb::Error::Other(_)) => return Err(Failed),
            }
        }
    }
    loop {
        match sink.flush() {
            Ok(()) => return Ok(()),
            Err(nb::Error::WouldBlock) => timer.wait_for(poll).await.map_err(|_| Failed)?,
            Err(nb::Error::Other(_)) => return Err(Failed),
        }
    }
}

/// Stop authority is issued before touching the host FIFO. No retry or wait
/// can delay returning it, even when the host never reads another byte.
pub fn fail_stop<R>(
    stop: impl FnOnce() -> R,
    sink: &mut impl Sink,
    response: &impl HciControllerResponse,
) -> R {
    let stopped = stop();
    for byte in core::iter::once(response.kind() as u8).chain(response.as_bytes().iter().copied()) {
        if sink.write(byte).is_err() {
            return stopped;
        }
    }
    let _ = sink.flush();
    stopped
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use oer_bluetooth_hci::bt_hci::PacketKind;
    use oer_time_virtual::VirtualClock;
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
    struct Output<'a> {
        bytes: Vec<u8>,
        fail_at: Option<usize>,
        flush_error: bool,
        blocked: bool,
        stopped: Option<&'a Cell<bool>>,
    }
    impl Sink for Output<'_> {
        type Error = ();
        fn write(&mut self, byte: u8) -> nb::Result<(), ()> {
            if let Some(stopped) = self.stopped {
                assert!(stopped.get());
            }
            if self.blocked {
                return Err(nb::Error::WouldBlock);
            }
            if self.fail_at == Some(self.bytes.len()) {
                return Err(nb::Error::Other(()));
            }
            self.bytes.push(byte);
            Ok(())
        }
        fn flush(&mut self) -> nb::Result<(), ()> {
            if self.flush_error {
                Err(nb::Error::Other(()))
            } else {
                Ok(())
            }
        }
    }
    fn output() -> Output<'static> {
        Output {
            bytes: Vec::new(),
            fail_at: None,
            flush_error: false,
            blocked: false,
            stopped: None,
        }
    }
    #[test]
    fn write_and_flush_errors_do_not_reuse_a_response_cursor() {
        let clock = VirtualClock::<1>::new();
        let mut cx = Context::from_waker(Waker::noop());
        for flush_error in [false, true] {
            let mut sink = output();
            sink.flush_error = flush_error;
            sink.fail_at = if flush_error { None } else { Some(2) };
            assert_eq!(
                pin!(send(&mut sink, &clock, Duration::from_millis(1), &Response)).poll(&mut cx),
                Poll::Ready(Err(Failed))
            );
            let restart = sink.bytes.len();
            sink.fail_at = None;
            sink.flush_error = false;
            assert_eq!(
                pin!(send(&mut sink, &clock, Duration::from_millis(1), &Response)).poll(&mut cx),
                Poll::Ready(Ok(()))
            );
            assert_eq!(&sink.bytes[restart..], &[4, 0x10, 1, 9]);
        }
    }
    #[test]
    fn cancellation_under_backpressure_starts_the_next_response_at_its_kind() {
        let clock = VirtualClock::<1>::new();
        let mut cx = Context::from_waker(Waker::noop());
        let mut sink = output();
        sink.blocked = true;
        {
            let mut send = pin!(send(&mut sink, &clock, Duration::from_millis(1), &Response));
            assert!(send.as_mut().poll(&mut cx).is_pending());
            assert!(clock.next_deadline().is_some());
        }
        sink.blocked = false;
        assert_eq!(
            pin!(send(&mut sink, &clock, Duration::from_millis(1), &Response)).poll(&mut cx),
            Poll::Ready(Ok(()))
        );
        assert_eq!(sink.bytes, [4, 0x10, 1, 9]);
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
            fail_stop(
                || {
                    stopped.set(true);
                    42
                },
                &mut sink,
                &Response
            ),
            42
        );
        assert!(stopped.get());
        assert!(sink.bytes.is_empty());
    }
}
