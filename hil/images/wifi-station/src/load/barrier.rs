//! A run's start barrier: the host's one release, within a bound. A
//! synchronised run opens its window only after the host's `Begin`, and a
//! receiving run only once its incoming flow delivered a load datagram; a
//! run no release came for within [`MAX_DELAY_MS`] of its Start ends without
//! a window, refused with [`LoadError::NotReleased`].

use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::Poll;
use embassy_futures::select::{Either, select};
use oer_hil_services_load::{LoadError, MAX_DELAY_MS};

/// What one `Begin` found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Release {
    /// Released at `at_us`; the run's first load datagram came at
    /// `first_received_us`.
    Ready {
        at_us: u64,
        first_received_us: Option<u64>,
    },
    /// A receiving run whose flow delivered nothing yet.
    WaitingForLoad,
    Refused(LoadError),
}

pub(crate) struct Barrier {
    pub(crate) armed_us: u64,
    deadline_us: u64,
    synchronised: bool,
    triggered_us: Cell<Option<u64>>,
}

impl Barrier {
    /// Armed at `armed_us`: a timed run is released at once.
    pub(crate) fn new(armed_us: u64, synchronised: bool) -> Self {
        Self {
            armed_us,
            deadline_us: armed_us.saturating_add(u64::from(MAX_DELAY_MS).saturating_mul(1000)),
            synchronised,
            triggered_us: Cell::new((!synchronised).then_some(armed_us)),
        }
    }

    /// The host's `Begin` at `now_us`: with `receive`, only once the run's
    /// first load datagram came (`first_received_us`). A repeated Begin
    /// answers the same release.
    pub(crate) fn begin(
        &self,
        now_us: u64,
        receive: bool,
        first_received_us: Option<u64>,
    ) -> Release {
        if !self.synchronised {
            return Release::Refused(LoadError::InvalidParameters);
        }
        if let Some(at_us) = self.triggered_us.get() {
            return Release::Ready {
                at_us,
                first_received_us,
            };
        }
        if now_us >= self.deadline_us {
            return Release::Refused(LoadError::NotReleased);
        }
        if receive && first_received_us.is_none() {
            return Release::WaitingForLoad;
        }
        self.triggered_us.set(Some(now_us));
        Release::Ready {
            at_us: now_us,
            first_received_us,
        }
    }

    /// The instant past which no Begin releases the run.
    pub(crate) fn deadline(&self) -> u64 {
        self.deadline_us
    }

    /// The release, once it came; [`LoadError::NotReleased`] once `expiry`
    /// (the wait until [`Self::deadline`]) ended first.
    pub(crate) async fn released(
        &self,
        expiry: impl Future<Output = ()>,
    ) -> Result<u64, LoadError> {
        let triggered = poll_fn(|_| self.triggered_us.get().map_or(Poll::Pending, Poll::Ready));
        match select(triggered, expiry).await {
            Either::First(at_us) => Ok(at_us),
            // A release in the same poll as the expiry still counts.
            Either::Second(()) => self.triggered_us.get().ok_or(LoadError::NotReleased),
        }
    }
}

/// Poll the receiving and measuring siblings while a Begin waits for the
/// window to open: opening it need not wake another task. A sibling that
/// ended gives its output, never polled again.
pub(crate) async fn until_opened<F: Future + Unpin>(
    activity: &mut F,
    opened: &Cell<Option<u64>>,
) -> Option<F::Output> {
    poll_fn(|cx| {
        let ended = match Pin::new(&mut *activity).poll(cx) {
            Poll::Ready(output) => Some(output),
            Poll::Pending => None,
        };
        if opened.get().is_some() || ended.is_some() {
            Poll::Ready(ended)
        } else {
            Poll::Pending
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::task::{Context, Waker};

    /// Poll `future` once with a waker that does nothing.
    fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
        let mut future = core::pin::pin!(future);
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn begin_acknowledges_opening_without_waiting_for_the_window_end() {
        let opened = Cell::new(None);
        let ready = Cell::new(false);
        let mut activity = poll_fn(|_| {
            if ready.get() {
                opened.set(Some(5000));
            }
            Poll::<()>::Pending
        });
        let mut waiting = core::pin::pin!(until_opened(&mut activity, &opened));
        let cx = &mut Context::from_waker(Waker::noop());
        assert!(waiting.as_mut().poll(cx).is_pending());
        ready.set(true);
        assert_eq!(waiting.as_mut().poll(cx), Poll::Ready(None));
        assert_eq!(opened.get(), Some(5000));
    }

    #[test]
    fn a_completed_measurement_is_returned_without_repolling_it() {
        let opened = Cell::new(None);
        let mut activity = core::future::ready(7);
        let mut waiting = core::pin::pin!(until_opened(&mut activity, &opened));
        assert_eq!(
            waiting
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Some(7))
        );
    }

    #[test]
    fn the_window_waits_for_real_load_and_one_explicit_release() {
        let gate = Barrier::new(100, true);
        assert_eq!(gate.armed_us, 100);
        assert_eq!(gate.begin(1000, true, None), Release::WaitingForLoad);
        assert!(poll_once(gate.released(core::future::pending())).is_pending());
        let ready = Release::Ready {
            at_us: 2000,
            first_received_us: Some(1500),
        };
        assert_eq!(gate.begin(2000, true, Some(1500)), ready);
        assert_eq!(gate.begin(9000, true, Some(1500)), ready);
        assert_eq!(
            poll_once(gate.released(core::future::pending())),
            Poll::Ready(Ok(2000))
        );
    }

    /// A receiving run whose flow never delivers: every Begin waits until
    /// the bound, MAX_DELAY_MS after Start; from then on Begin is refused
    /// and the run's wait for its release ends, both as `NotReleased`.
    #[test]
    fn a_flow_that_never_becomes_ready_ends_as_a_named_refusal_at_its_bound() {
        let gate = Barrier::new(100, true);
        assert_eq!(gate.deadline(), 100 + u64::from(MAX_DELAY_MS) * 1000);
        assert_eq!(
            gate.begin(gate.deadline() - 1, true, None),
            Release::WaitingForLoad
        );
        assert_eq!(
            gate.begin(gate.deadline(), true, None),
            Release::Refused(LoadError::NotReleased)
        );
        assert_eq!(
            poll_once(gate.released(core::future::ready(()))),
            Poll::Ready(Err(LoadError::NotReleased))
        );
        // A late load datagram releases nothing.
        assert_eq!(
            gate.begin(gate.deadline() + 1, true, Some(gate.deadline())),
            Release::Refused(LoadError::NotReleased)
        );
    }

    #[test]
    fn tx_needs_no_incoming_load_and_a_timed_run_is_released_at_once() {
        let tx = Barrier::new(100, true);
        assert!(matches!(
            tx.begin(200, false, None),
            Release::Ready { at_us: 200, .. }
        ));
        let timed = Barrier::new(100, false);
        assert_eq!(
            poll_once(timed.released(core::future::ready(()))),
            Poll::Ready(Ok(100))
        );
        assert_eq!(
            timed.begin(200, false, None),
            Release::Refused(LoadError::InvalidParameters)
        );
    }
}
