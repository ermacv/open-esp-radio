//! The release answer's wait, bounded by the stop's deadline, and the
//! release of a stopped radio until it left.

use core::{
    future::Future,
    task::{Context, Poll},
};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_ieee80211_contracts_port::{
    DataPort, LowerMacTerminal, PortSink, PortTypes, TbttEvent,
};

/// The stopped port's cleanup, once per call of its lifecycle owner, after
/// a confirmed stop: the removal receipts it frees end here with the
/// retiring installation's other tokens ([`Retiring`]).
///
/// # Errors
///
/// The cleanup did not finish: the port keeps its owners and stays
/// stopped; nothing here calls it again.
pub(crate) fn clean<'id, P>(port: &mut P) -> Result<(), crate::failure::CleanupCause>
where
    P: DataPort<'id, Fault = oer_chips_esp32s31_radio_ieee80211_backend::port::Fault>,
{
    port.clean_stopped(&mut Retiring)
        .map_err(crate::failure::convert::cleanup)
}

/// The sink of a retiring port's cleanup. It takes a removal receipt,
/// whose slot and permission end with the installation the port retires,
/// and refuses every other item whole: the port keeps it, and then does
/// not retire.
struct Retiring;

impl<'id, T: PortTypes<'id>> PortSink<'id, T> for Retiring {
    fn terminal(
        &mut self,
        terminal: LowerMacTerminal<'id, T>,
    ) -> Result<(), LowerMacTerminal<'id, T>> {
        match terminal {
            LowerMacTerminal::KeyRemoved(_removed) => Ok(()),
            other => Err(other),
        }
    }

    fn tbtt(&mut self, _event: TbttEvent) {}

    fn received(
        &mut self,
        frame: T::RxBuffer,
        meta: T::RxMeta,
    ) -> Result<(), (T::RxBuffer, T::RxMeta)> {
        Err((frame, meta))
    }
}

/// The owners a refused start returned, which end through [`Retire::retire`].
pub(crate) trait Retire {
    /// End the owners: a member of the radio system leaves it.
    ///
    /// # Errors
    ///
    /// The cause a posted leave ended with.
    fn retire(self) -> impl Future<Output = Result<(), crate::failure::ReleaseCause>>;
}

/// End the owners a start's refusal for `cause` returned, keeping both
/// outcomes.
pub(crate) async fn refused<C, O: Retire>(cause: C, owners: O) -> crate::failure::Unavailable<C> {
    crate::failure::Unavailable {
        cause,
        cleanup: owners.retire().await,
    }
}

/// What one release of a stopped radio `R` did.
pub(crate) enum Leave<R, C> {
    /// It left the radio system.
    Left,
    /// It refused before it posted anything, for `cause`: the radio comes
    /// back for a later release.
    NotPosted { cause: C, radio: R },
    /// The leave was posted and ended for `cause`: it is never posted
    /// again.
    Ended(C),
}

/// Release `radio` through `leave` until it left or a posted leave ended. A
/// release that posted nothing is tried again `retry` after its refusal,
/// each refusal's cause going to `refused`; a posted leave is waited for by
/// `leave` and never posted again.
///
/// # Errors
///
/// The cause a posted leave ended with.
pub(crate) async fn until_left<R, C, F>(
    timer: &impl Timer,
    retry: Duration,
    radio: R,
    mut leave: impl FnMut(R) -> F,
    mut refused: impl FnMut(&C),
) -> Result<(), C>
where
    F: Future<Output = Leave<R, C>>,
{
    let mut radio = radio;
    loop {
        match leave(radio).await {
            Leave::Left => return Ok(()),
            Leave::Ended(cause) => return Err(cause),
            Leave::NotPosted { cause, radio: back } => {
                refused(&cause);
                radio = back;
                timer.wait_until(timer.now().saturating_add(retry)).await;
            }
        }
    }
}

/// Wait for `answer` until `deadline`, which is checked before each poll of
/// it: once the deadline has passed `answer` is not polled again, so an
/// answer ready only after it stays where it is, and the wait ends with
/// `None`. An answer taken by a poll that began before the deadline is
/// returned. The answer's owner keeps what it was handed; nothing here
/// takes it back.
pub(crate) async fn within<A>(
    timer: &impl Timer,
    deadline: Option<Instant>,
    mut answer: impl FnMut(&mut Context<'_>) -> Poll<A>,
) -> Option<A> {
    let mut alarm = core::pin::pin!(async {
        match deadline {
            Some(deadline) => timer.wait_until(deadline).await,
            None => core::future::pending().await,
        }
    });
    core::future::poll_fn(|cx| {
        if alarm.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        answer(cx).map(Some)
    })
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]
mod tests {
    use super::*;
    use core::cell::Cell;
    use oer_portable_time::{Clock as _, Duration};
    use oer_portable_time_virtual::VirtualClock;

    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(core::task::Waker::noop()))
    }

    /// A stopped radio of the test: its number names the one owner.
    #[derive(Debug, Eq, PartialEq)]
    struct Radio(u8);

    fn poll_now<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        poll(future)
    }

    #[test]
    fn a_release_refused_before_its_post_is_tried_again_with_the_same_radio_until_it_left() {
        let clock = VirtualClock::<2>::new();
        let attempts = Cell::new(0_u8);
        let refusals = Cell::new(0_u8);
        let retry = Duration::from_millis(1);
        let mut release = core::pin::pin!(until_left(
            &clock,
            retry,
            Radio(7),
            |radio: Radio| {
                attempts.set(attempts.get() + 1);
                let attempt = attempts.get();
                async move {
                    // The owner every attempt holds is the first one.
                    assert_eq!(radio, Radio(7));
                    if attempt < 3 {
                        Leave::NotPosted {
                            cause: "still active",
                            radio,
                        }
                    } else {
                        Leave::Left
                    }
                }
            },
            |_| refusals.set(refusals.get() + 1),
        ));
        assert!(poll_now(release.as_mut()).is_pending());
        assert_eq!((attempts.get(), refusals.get()), (1, 1));
        // Nothing is tried again before the retry time.
        assert!(poll_now(release.as_mut()).is_pending());
        assert_eq!(attempts.get(), 1);
        clock.advance(retry).unwrap();
        assert!(poll_now(release.as_mut()).is_pending());
        assert_eq!((attempts.get(), refusals.get()), (2, 2));
        clock.advance(retry).unwrap();
        assert_eq!(poll_now(release.as_mut()), Poll::Ready(Ok(())));
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn a_posted_leave_that_ended_is_never_posted_again() {
        let clock = VirtualClock::<2>::new();
        let attempts = Cell::new(0_u8);
        let mut release = core::pin::pin!(until_left(
            &clock,
            Duration::from_millis(1),
            Radio(1),
            |_radio: Radio| {
                attempts.set(attempts.get() + 1);
                async { Leave::Ended("unanswered") }
            },
            |_: &&str| panic!("a posted leave is no refusal before its post"),
        ));
        assert_eq!(poll_now(release.as_mut()), Poll::Ready(Err("unanswered")));
        assert_eq!(attempts.get(), 1);
    }

    /// Owners whose end the test names.
    struct Owners<'a>(Result<(), crate::failure::ReleaseCause>, &'a Cell<u8>);

    impl Retire for Owners<'_> {
        async fn retire(self) -> Result<(), crate::failure::ReleaseCause> {
            self.1.set(self.1.get() + 1);
            self.0
        }
    }

    #[test]
    fn a_refused_start_keeps_its_cause_and_its_failed_cleanup() {
        let ends = Cell::new(0_u8);
        let cleanup = Err(crate::failure::ReleaseCause::LeaveUnanswered([1, 2, 3, 4]));
        let mut ended = core::pin::pin!(refused("storage", Owners(cleanup, &ends)));
        assert_eq!(
            poll_now(ended.as_mut()),
            Poll::Ready(crate::failure::Unavailable {
                cause: "storage",
                cleanup,
            })
        );
        assert_eq!(ends.get(), 1, "the owners end once");
        let mut ended = core::pin::pin!(refused("radio", Owners(Ok(()), &ends)));
        assert_eq!(
            poll_now(ended.as_mut()),
            Poll::Ready(crate::failure::Unavailable::took_nothing("radio"))
        );
    }

    #[test]
    fn an_answer_in_time_is_taken_once() {
        let clock = VirtualClock::<2>::new();
        let ready = Cell::new(false);
        let taken = Cell::new(0);
        let deadline = Some(clock.now().saturating_add(Duration::from_millis(10)));
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            if ready.get() {
                taken.set(taken.get() + 1);
                Poll::Ready(7)
            } else {
                Poll::Pending
            }
        }));
        assert!(poll(wait.as_mut()).is_pending());
        ready.set(true);
        assert_eq!(poll(wait.as_mut()), Poll::Ready(Some(7)));
        assert_eq!(taken.get(), 1);
    }

    #[test]
    fn an_answer_ready_only_after_the_deadline_is_not_read() {
        let clock = VirtualClock::<2>::new();
        let ready = Cell::new(false);
        let polls = Cell::new(0);
        let taken = Cell::new(0);
        let deadline = Some(clock.now().saturating_add(Duration::from_millis(10)));
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            polls.set(polls.get() + 1);
            if ready.get() {
                taken.set(taken.get() + 1);
                Poll::Ready(7)
            } else {
                Poll::Pending
            }
        }));
        assert!(poll(wait.as_mut()).is_pending());
        assert_eq!(polls.get(), 1);
        // Both happen before the next poll: the deadline wins.
        clock.advance(Duration::from_millis(10)).unwrap();
        ready.set(true);
        assert_eq!(poll(wait.as_mut()), Poll::Ready(None));
        assert_eq!(
            polls.get(),
            1,
            "the answer is not polled after the deadline"
        );
        assert_eq!(taken.get(), 0, "the late answer stays in its slot");
    }

    #[test]
    fn a_deadline_that_passed_before_the_wait_reads_no_answer() {
        let clock = VirtualClock::<2>::new();
        clock.advance(Duration::from_millis(5)).unwrap();
        let deadline = Some(clock.now());
        let taken = Cell::new(0);
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            taken.set(taken.get() + 1);
            Poll::Ready(7)
        }));
        assert_eq!(poll(wait.as_mut()), Poll::Ready(None));
        assert_eq!(taken.get(), 0);
    }
}
