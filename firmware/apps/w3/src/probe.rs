//! A failed local echo probe is an observation, not a link-loss decision.
use core::ops::AsyncFnMut;
use embassy_futures::select::{Either, select};
use oer_time::{Clock, Duration, Instant, Timer};
use owned_embassy_net::raw::SendError;

use crate::echo::ECHO_COUNT;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProbeCounters {
    pub socket_errors: u32,
    pub timeouts: u32,
    pub last_socket_error: Option<SendError>,
}

impl ProbeCounters {
    pub const fn last_socket_error_text(&self) -> oer_diagnostics::Name<32> {
        match self.last_socket_error {
            None => oer_diagnostics::name!("None"; 32),
            Some(SendError::InvalidState) => oer_diagnostics::name!("Some(InvalidState)"; 32),
            Some(SendError::Unaddressable) => oer_diagnostics::name!("Some(Unaddressable)"; 32),
            Some(SendError::BufferFull) => oer_diagnostics::name!("Some(BufferFull)"; 32),
            Some(SendError::Malformed) => oer_diagnostics::name!("Some(Malformed)"; 32),
        }
    }
}

/// Run the scheduled series while the link remains healthy. Each attempt has
/// its own send deadline; an error or expiry consumes that probe and the next
/// one keeps its original schedule. The caller retains the normal completion
/// and link-loss departure paths, independently of these counters.
pub async fn run<T: Clock + Timer>(
    clock: &T,
    first: Instant,
    mut send: impl AsyncFnMut(u16) -> Result<(), SendError>,
    mut healthy: impl FnMut() -> bool,
) -> ProbeCounters {
    let mut counters = ProbeCounters::default();
    let mut next = first;
    for sequence in 1..=ECHO_COUNT {
        while clock.now() < next && healthy() {
            let wake = clock
                .now()
                .saturating_add(Duration::from_millis(10))
                .min(next);
            clock.wait_until(wake).await;
        }
        if !healthy() {
            break;
        }
        match select(send(sequence), clock.wait_for(Duration::from_millis(250))).await {
            Either::First(Ok(())) => {}
            Either::First(Err(error)) => {
                counters.socket_errors = counters.socket_errors.saturating_add(1);
                counters.last_socket_error = Some(error);
            }
            Either::Second(_) => counters.timeouts = counters.timeouts.saturating_add(1),
        }
        next = next
            .checked_add(Duration::from_secs(1))
            .expect("W3 ping schedule");
    }
    counters
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{cell::Cell, future::Future, task::Context};
    use oer_time_virtual::VirtualClock;

    #[test]
    fn socket_errors_and_timeouts_do_not_finish_the_workload_or_request_departure() {
        let clock = VirtualClock::<8>::new();
        let link = Cell::new(true);
        let attempts = Cell::new(0);
        let mut future = core::pin::pin!(run(
            &clock,
            clock.now(),
            async |sequence| {
                attempts.set(attempts.get() + 1);
                match sequence % 3 {
                    1 => Err(SendError::Unaddressable),
                    2 => core::future::pending().await,
                    _ => Ok(()),
                }
            },
            || link.get(),
        ));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        let start = clock.now();
        for sequence in 1..=ECHO_COUNT {
            clock.advance_to(start.saturating_add(Duration::from_secs(u32::from(sequence - 1))));
            // Even the immediate first error cannot finish the series and
            // return to the application's final Disconnect path.
            let result = future.as_mut().poll(&mut cx);
            assert_eq!(attempts.get(), sequence);
            if sequence % 3 == 2 {
                assert!(result.is_pending());
                clock.advance(Duration::from_millis(250)).unwrap();
            } else if sequence != ECHO_COUNT {
                assert!(result.is_pending());
                continue;
            }
            let result = if sequence % 3 == 2 {
                future.as_mut().poll(&mut cx)
            } else {
                result
            };
            if sequence != ECHO_COUNT {
                assert!(result.is_pending());
            } else {
                let core::task::Poll::Ready(counters) = result else {
                    panic!("the complete 100-probe series finishes");
                };
                assert_eq!(counters.socket_errors, 34);
                assert_eq!(counters.timeouts, 33);
                assert_eq!(counters.last_socket_error, Some(SendError::Unaddressable));
            }
        }
        assert!(link.get());
    }

    #[test]
    fn losing_the_link_still_ends_the_series_before_the_next_probe() {
        let clock = VirtualClock::<8>::new();
        let link = Cell::new(true);
        let attempts = Cell::new(0);
        let mut future = core::pin::pin!(run(
            &clock,
            clock.now(),
            async |_| {
                attempts.set(attempts.get() + 1);
                Err(SendError::InvalidState)
            },
            || link.get(),
        ));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        link.set(false);
        clock.advance(Duration::from_millis(10)).unwrap();
        let core::task::Poll::Ready(counters) = future.as_mut().poll(&mut cx) else {
            panic!("lost link ends probes");
        };
        assert_eq!(attempts.get(), 1);
        assert_eq!(counters.socket_errors, 1);
        assert_eq!(counters.timeouts, 0);
    }
}
