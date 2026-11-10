//! The release answer's wait, bounded by the stop's deadline, and the
//! release of a stopped radio until it left.

use core::{
    future::Future,
    task::{Context, Poll},
};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_ieee80211_contracts_port::{
    DataPort, Input, LowerMacTerminal, PortSink, PortTypes, Removed, TbttEvent, Unkeeping, Unkept,
};

/// What one call of the stopped port's cleanup did: its result, and the
/// removal receipt `R` it freed, whose slot and permission the composition
/// that called it keeps, past the port's final retire, and on a refused
/// retire or reinstall too.
#[must_use = "the cleanup's receipt holds a slot and a permission"]
pub(crate) struct Cleaned<R> {
    pub result: Result<(), crate::failure::CleanupCause>,
    pub removed: Option<R>,
}

/// What the stopped port's cleanup freed for its caller, whole: the
/// removal receipt its sink kept, and an item the port could neither give
/// that sink nor keep. Whoever holds it holds their owners; it ends with
/// the retired installation after a release that succeeded.
#[must_use = "what the cleanup freed holds owners"]
pub struct Freed<'id, T: PortTypes<'id>> {
    /// The removal receipt (a key slot and its permission).
    pub removed: Option<Removed<'id, T>>,
    /// The item the port could not keep.
    pub unkept: Option<Unkept<'id, T>>,
}

impl<'id, T: PortTypes<'id>> Freed<'id, T> {
    /// What was freed, when anything was.
    fn of(removed: Option<Removed<'id, T>>, unkept: Option<Unkept<'id, T>>) -> Option<Self> {
        if removed.is_none() && unkept.is_none() {
            None
        } else {
            Some(Self { removed, unkept })
        }
    }
}

/// The stopped port's cleanup, once per call of its lifecycle owner, after
/// a confirmed stop. What it frees comes back whole to the caller
/// ([`Cleaned::removed`]); nothing here calls it again. An item the port
/// could not keep refuses the cleanup ([`CleanupCause::Unkept`]): nothing
/// is released after it.
///
/// [`CleanupCause::Unkept`]: crate::failure::CleanupCause::Unkept
pub(crate) fn clean<'id, P>(port: &mut P) -> Cleaned<Freed<'id, P::Types>>
where
    P: DataPort<'id, Fault = oer_chips_esp32s31_radio_ieee80211_backend::port::Fault>,
    P::Types: PortTypes<'id>,
{
    let mut sink = Returning { removed: None };
    let (result, unkept) = match port.clean_stopped(&mut sink) {
        Ok(answer) => (answer.map_err(crate::failure::convert::cleanup), None),
        Err(Unkeeping { unkept, input }) => {
            let result = match input {
                // The cleanup ran: its own refusal first, then the item.
                Input::Accepted(answer) => answer
                    .map_err(crate::failure::convert::cleanup)
                    .and(Err(crate::failure::CleanupCause::Unkept)),
                // An item an earlier call left refused the cleanup before
                // it ran: nothing was cleaned.
                Input::NotAccepted(()) => Err(crate::failure::CleanupCause::Unkept),
            };
            (result, Some(unkept))
        }
    };
    Cleaned {
        result,
        removed: Freed::of(sink.removed, unkept),
    }
}

/// A retired port whose stopped radio a release takes: the S31 port on the
/// chip, a port of the test on the host.
pub(crate) trait Stopped {
    /// The removal receipt its cleanup frees.
    type Receipt;
    /// The stopped radio its release takes.
    type Radio;
    /// The stopped port's cleanup, once ([`clean`]).
    fn clean(&mut self) -> Cleaned<Self::Receipt>;
    /// Take the stopped radio.
    ///
    /// # Errors
    ///
    /// Why the port gives up none, and then nothing was taken.
    fn take_stopped(&mut self) -> Result<Self::Radio, Unreleased>;
}

/// Why a retired port gave up no stopped radio: nothing was taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Unreleased {
    /// Its radio's stop is not confirmed.
    StopUnconfirmed,
    /// Its radio's stop is confirmed and it still holds owners.
    Held,
}

impl From<Unreleased> for crate::failure::ReleaseCause {
    fn from(unreleased: Unreleased) -> Self {
        match unreleased {
            Unreleased::StopUnconfirmed => Self::StopUnconfirmed,
            Unreleased::Held => Self::Held,
        }
    }
}

/// Run the stopped port's cleanup once, then release the retired port's
/// radio through `leave` ([`until_left`], every `retry` after a refusal
/// that posted nothing, each refusal's cause going to `still_active`). The
/// cleanup's removal receipt comes back with the result, whatever it is,
/// for the caller to keep.
pub(crate) async fn release<P: Stopped, F>(
    port: &mut P,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(P::Radio) -> F,
    mut still_active: impl FnMut(crate::failure::ReleaseCause),
) -> (Result<(), crate::failure::ReleaseCause>, Option<P::Receipt>)
where
    F: Future<Output = Leave<P::Radio, crate::failure::ReleaseCause>>,
{
    let Cleaned { result, removed } = port.clean();
    if let Err(cause) = result {
        return (Err(crate::failure::ReleaseCause::Cleanup(cause)), removed);
    }
    let radio = match port.take_stopped() {
        Ok(radio) => radio,
        Err(unreleased) => return (Err(unreleased.into()), removed),
    };
    let left = until_left(timer, retry, radio, leave, |cause| still_active(*cause)).await;
    (left, removed)
}

/// The sink of a stopped port's cleanup: one place for the removal receipt
/// it frees. A second receipt and every other item are refused whole: the
/// port keeps them, and then does not retire.
struct Returning<'id, T: PortTypes<'id>> {
    removed: Option<Removed<'id, T>>,
}

impl<'id, T: PortTypes<'id>> PortSink<'id, T> for Returning<'id, T> {
    fn terminal(
        &mut self,
        terminal: LowerMacTerminal<'id, T>,
    ) -> Result<(), LowerMacTerminal<'id, T>> {
        match terminal {
            LowerMacTerminal::KeyRemoved(removed) if self.removed.is_none() => {
                self.removed = Some(removed);
                Ok(())
            }
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

/// The owners a refused start returned: a member of the radio system
/// leaves it through [`Retire::retire`], and the others come back.
pub(crate) trait Retire {
    /// What comes back after the leave: every owner it did not take.
    type Left;
    /// A member of the radio system leaves it; every other owner comes
    /// back beside the leave's outcome (the cause a posted leave ended
    /// with).
    fn retire(self)
    -> impl Future<Output = (Result<(), crate::failure::ReleaseCause>, Self::Left)>;
}

/// The owners a start's refusal for `cause` returned: the radio's leave,
/// both outcomes kept, and every owner the leave did not take.
pub(crate) async fn refused<C, O: Retire>(
    cause: C,
    owners: O,
) -> (crate::failure::Unavailable<C>, O::Left) {
    let (cleanup, left) = owners.retire().await;
    (crate::failure::Unavailable { cause, cleanup }, left)
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

    /// Owners whose end the test names: a member that leaves (its leave's
    /// outcome and a count of leaves) and another owner, which comes back.
    struct Owners<'a>(
        Result<(), crate::failure::ReleaseCause>,
        &'a Cell<u8>,
        Kept<'a>,
    );

    /// An owner the leave does not take: each drop counts once.
    struct Kept<'a>(&'a Cell<u8>);

    impl Drop for Kept<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    impl<'a> Retire for Owners<'a> {
        type Left = Kept<'a>;
        async fn retire(self) -> (Result<(), crate::failure::ReleaseCause>, Kept<'a>) {
            let Self(left, leaves, kept) = self;
            leaves.set(leaves.get() + 1);
            (left, kept)
        }
    }

    #[test]
    fn a_refused_start_keeps_its_cause_and_its_failed_cleanup_and_returns_the_other_owners() {
        let (ends, kept) = (Cell::new(0_u8), Cell::new(0_u8));
        let cleanup = Err(crate::failure::ReleaseCause::LeaveUnanswered([1, 2, 3, 4]));
        let mut ended = core::pin::pin!(refused("storage", Owners(cleanup, &ends, Kept(&kept))));
        let Poll::Ready((unavailable, left)) = poll_now(ended.as_mut()) else {
            panic!("the leave of the test answers at once");
        };
        assert_eq!(
            unavailable,
            crate::failure::Unavailable {
                cause: "storage",
                cleanup,
            }
        );
        assert_eq!(ends.get(), 1, "the member left once");
        assert_eq!(kept.get(), 0, "the other owner comes back");
        drop(left);
        assert_eq!(kept.get(), 1, "and ends with its holder");
        let mut ended = core::pin::pin!(refused("radio", Owners(Ok(()), &ends, Kept(&kept))));
        let Poll::Ready((unavailable, _left)) = poll_now(ended.as_mut()) else {
            panic!("the leave of the test answers at once");
        };
        assert_eq!(
            unavailable,
            crate::failure::Unavailable::took_nothing("radio")
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
