//! One host release, with a finite lifetime. Polling the sibling measurement
//! after a request observes the release; the request lane supplies its wake.

use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::Poll;
use oer_hil_services_load::{LoadError, MAX_DELAY_MS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Release {
    Ready {
        at_us: u64,
        first_received_us: Option<u64>,
    },
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
    pub(crate) fn new(armed_us: u64, synchronised: bool) -> Self {
        Self {
            armed_us,
            deadline_us: armed_us.saturating_add(u64::from(MAX_DELAY_MS).saturating_mul(1000)),
            synchronised,
            triggered_us: Cell::new((!synchronised).then_some(armed_us)),
        }
    }

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
            return Release::Refused(LoadError::InvalidParameters);
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

    pub(crate) fn triggered(&self) -> Option<u64> {
        self.triggered_us.get()
    }

    pub(crate) fn deadline(&self) -> u64 {
        self.deadline_us
    }
}

/// Poll the receiving/measurement siblings while Begin awaits the actual
/// window. Check after polling: opening the window need not wake a second
/// task. Keep a completed output instead of ever repolling a completed child.
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
        assert_eq!(gate.triggered(), None);
        let ready = Release::Ready {
            at_us: 2000,
            first_received_us: Some(1500),
        };
        assert_eq!(gate.begin(2000, true, Some(1500)), ready);
        assert_eq!(gate.begin(9000, true, Some(1500)), ready);
        assert_eq!(gate.triggered(), Some(2000));
    }

    #[test]
    fn tx_needs_no_incoming_load_and_an_abandoned_barrier_expires() {
        let tx = Barrier::new(100, true);
        assert!(matches!(
            tx.begin(200, false, None),
            Release::Ready { at_us: 200, .. }
        ));
        let abandoned = Barrier::new(100, true);
        assert!(matches!(
            abandoned.begin(abandoned.deadline(), false, None),
            Release::Refused(_)
        ));
        assert_eq!(abandoned.triggered(), None);
        let timed = Barrier::new(100, false);
        assert_eq!(timed.triggered(), Some(100));
        assert!(matches!(timed.begin(200, false, None), Release::Refused(_)));
    }
}
