use super::*;
use core::{cell::Cell, future::pending, pin::pin, task::Waker};
use std::{boxed::Box, rc::Rc};

mod queues;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Executor {
    P1,
    P2,
}

struct Model {
    executor: Cell<Executor>,
    abandons: Cell<usize>,
    finishes: Cell<usize>,
    drops: Cell<usize>,
    stop_loans: Cell<usize>,
    frontend_loans: Cell<usize>,
    starts: Cell<usize>,
    stall_stop: Cell<bool>,
    stopped: bool,
}

struct Owner(Rc<Model>);
struct StopLoan(Rc<Model>);

impl Drop for StopLoan {
    fn drop(&mut self) {
        assert_eq!(self.0.executor.get(), Executor::P2);
        self.0.stop_loans.set(self.0.stop_loans.get() - 1);
    }
}

impl<Id: SlotIdentity> SlotOwner<Id> for Owner {
    type Pools = ();
    type Queues = ();
    type Frontend<'s>
        = ()
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = ()
    where
        Self: 's,
        Id: 's;
    fn queues(&mut self, mint: crate::Mint<'static, (), Id>) {
        mint.into_pools();
    }
    fn split<'s>(_: &'s mut (), _: crate::SlotWakes<'s, Id>) -> ((), ())
    where
        Self: 's,
        Id: 's,
    {
        ((), ())
    }

    fn run<'s>(
        &mut self,
        _: &mut Self::Invocation<'s>,
        authority: SlotRetire<'_, Id, Self>,
    ) -> impl Future<Output = SlotStopped<Id, Self>>
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.0.executor.get(), Executor::P2);
        self.0.starts.set(self.0.starts.get() + 1);
        async move {
            self.0.stop_loans.set(self.0.stop_loans.get() + 1);
            let _loan = StopLoan(self.0.clone());
            if self.0.stall_stop.get() {
                pending::<()>().await;
            }
            authority.requested().await
        }
    }

    fn abandon<'s>(
        &mut self,
        _: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.0.executor.get(), Executor::P2);
        assert_eq!(
            self.0.stop_loans.get(),
            0,
            "end the stop loan before cleanup"
        );
        assert_eq!(self.0.frontend_loans.get(), 0, "end P1 before P2 cleanup");
        self.0.abandons.set(self.0.abandons.get() + 1);
        if self.0.stopped {
            Abandon::Stopped(authority.stopped())
        } else {
            Abandon::Unconfirmed(authority)
        }
    }

    fn finish(self) {
        assert_eq!(self.0.executor.get(), Executor::P2);
        self.0.finishes.set(self.0.finishes.get() + 1);
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        assert_eq!(self.0.executor.get(), Executor::P2);
        self.0.drops.set(self.0.drops.get() + 1);
    }
}

async fn run_p2<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's>(p2: SlotRegistration<'s, Id, S>) {
    if let Some(completion) = p2.run().await {
        completion.unregistered();
    }
}

fn model(stopped: bool) -> Rc<Model> {
    Rc::new(Model {
        executor: Cell::new(Executor::P2),
        abandons: Cell::new(0),
        finishes: Cell::new(0),
        drops: Cell::new(0),
        stop_loans: Cell::new(0),
        frontend_loans: Cell::new(0),
        starts: Cell::new(0),
        stall_stop: Cell::new(false),
        stopped,
    })
}

fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn finish<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match poll(future.as_mut()) {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("model scope finishes without a timed wait"),
    }
}

#[test]
fn cancelled_p1_future_never_owns_or_destroys_the_registered_p2_side() {
    for stopped in [true, false] {
        for poll_p1 in [true, false] {
            let model = model(stopped);
            finish(with_local_slot(async |claim| {
                let mut slot = claim.place();
                let (frontend, mut p2) = slot
                    .register(Owner(model.clone()))
                    .ok()
                    .expect("fresh slot");
                model.executor.set(Executor::P1);
                let mut p1 = Box::pin(async move {
                    let _frontend = frontend;
                    pending::<()>().await;
                });
                if poll_p1 {
                    assert!(poll(p1.as_mut()).is_pending());
                }
                drop(p1);
                assert_eq!(model.abandons.get(), 0);
                assert_eq!(model.drops.get(), 0);
                assert!(p2.is_quarantined());
                model.executor.set(Executor::P2);
                assert!(p2.quarantine());
                let outcome = p2.signals.take_answer().expect("single P2 response");
                assert_eq!(matches!(outcome, Abandon::Stopped(_)), stopped);
                assert!(!p2.quarantine());
                assert!(p2.signals.take_answer().is_none());
                drop(p2);
                assert_eq!(model.abandons.get(), 1);
                assert_eq!(model.drops.get(), 0);
                assert!(slot.register(Owner(model.clone())).is_err());
            }));
            // The rejected, never-registered owner is dropped normally; the
            // quarantined owner is never dropped, even at model scope end.
            assert_eq!(model.drops.get(), 1);
            assert_eq!(model.finishes.get(), 0);
        }
    }
}

#[test]
fn destroying_registration_without_another_poll_cleans_up_once_on_p2() {
    let model = model(false);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        model.executor.set(Executor::P1);
        drop(frontend);
        assert_eq!(model.abandons.get(), 0);
        model.executor.set(Executor::P2);
        drop(p2);
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(model.drops.get(), 0);
    }));
    assert_eq!(model.drops.get(), 0);
}

#[test]
fn ordinary_join_follows_p2_destruction_and_is_the_only_reinstall_authority() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, mut p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        assert!(
            p2.poll_request(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        model.executor.set(Executor::P1);
        let mut join = Box::pin(frontend.finish().wait());
        assert!(poll(join.as_mut()).is_pending());
        assert_eq!(model.drops.get(), 0);
        model.executor.set(Executor::P2);
        assert!(
            p2.poll_request(&mut Context::from_waker(Waker::noop()))
                .is_ready()
        );
        run_p2(p2).await;
        assert_eq!(model.finishes.get(), 1);
        assert_eq!(model.drops.get(), 1);
        let joined = match poll(join.as_mut()) {
            Poll::Ready(Ok(joined)) => joined,
            _ => panic!("only the finished P2 owner publishes ordinary join"),
        };
        drop(join);
        assert!(slot.register(Owner(model.clone())).is_err());
        let (frontend, p2) = slot
            .reinstall(joined, Owner(model.clone()))
            .ok()
            .expect("matching joined slot");
        let join = frontend.finish();
        run_p2(p2).await;
        assert!(join.wait().await.is_ok());
        assert_eq!(model.finishes.get(), 2);
        assert_eq!(model.drops.get(), 3);
        assert_eq!(model.abandons.get(), 0);
    }));
}

#[test]
fn late_stopped_proof_never_turns_quarantine_into_joined() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, mut p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        let join = frontend.abandon();
        assert!(p2.quarantine());
        run_p2(p2).await;
        let receipt = join.wait().await.err().expect("absorbing quarantine");
        assert!(matches!(receipt.outcome(), Abandon::Stopped(_)));
        assert_eq!(model.finishes.get(), 0);
        assert_eq!(model.drops.get(), 0);
    }));
}

#[test]
fn cancelling_the_join_after_p2_finishes_still_withholds_reuse() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        let join = frontend.finish();
        run_p2(p2).await;
        drop(join);
        assert!(slot.register(Owner(model.clone())).is_err());
        assert_eq!(model.abandons.get(), 0);
        assert_eq!(model.finishes.get(), 1);
    }));
}

#[test]
fn cancelling_join_during_a_pending_p2_stop_ends_the_loan_before_cleanup() {
    let model = model(false);
    model.stall_stop.set(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        let join = frontend.finish();
        let mut stopping = Box::pin(p2.run());
        assert!(poll(stopping.as_mut()).is_pending());
        assert_eq!(model.stop_loans.get(), 1);
        model.executor.set(Executor::P1);
        drop(join);
        assert_eq!(model.abandons.get(), 0);
        assert_eq!(model.stop_loans.get(), 1);
        model.executor.set(Executor::P2);
        assert!(poll(stopping.as_mut()).is_ready());
        assert_eq!(model.stop_loans.get(), 0);
        assert_eq!(model.abandons.get(), 1);
        drop(stopping);
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(model.finishes.get(), 0);
        assert_eq!(model.drops.get(), 0);
    }));
}

#[test]
fn destroying_p2_task_during_stop_keeps_the_slot_owner_and_cleans_up_on_p2() {
    for poll_stop in [true, false] {
        let model = model(false);
        model.stall_stop.set(true);
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place();
            let (frontend, p2) = slot
                .register(Owner(model.clone()))
                .ok()
                .expect("fresh slot");
            let join = frontend.finish();
            let mut stopping = Box::pin(p2.run());
            if poll_stop {
                assert!(poll(stopping.as_mut()).is_pending());
            }
            drop(stopping);
            assert!(join.wait().await.is_err());
            assert_eq!(model.stop_loans.get(), 0);
            assert_eq!(model.abandons.get(), 1);
            assert_eq!(model.finishes.get(), 0);
            assert_eq!(model.drops.get(), 0);
        }));
    }
}

#[test]
fn repeated_and_parallel_local_scopes_share_no_declaration_latch() {
    std::thread::scope(|threads| {
        for _ in 0..4 {
            threads.spawn(|| {
                for _ in 0..32 {
                    let model = model(true);
                    finish(with_local_slot(async |claim| {
                        let mut slot = claim.place();
                        let (frontend, p2) = slot
                            .register(Owner(model.clone()))
                            .ok()
                            .expect("each invocation has its own backing and identity");
                        let join = frontend.finish();
                        run_p2(p2).await;
                        assert!(join.wait().await.is_ok());
                    }));
                    assert_eq!(model.finishes.get(), 1);
                    assert_eq!(model.drops.get(), 1);
                }
            });
        }
    });
}

#[test]
fn local_runner_joins_after_the_inner_frontend_future_is_cancelled() {
    for poll_inner in [true, false] {
        let model = model(false);
        let cancel = Cell::new(!poll_inner);
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place();
            let outer = crate::LocalExecution
                .scope(&mut slot, Owner(model.clone()), async |frontend| {
                    let mut inner = pin!(Some(async move {
                        let _frontend = frontend;
                        pending::<()>().await;
                    }));
                    poll_fn(|cx| {
                        model.executor.set(Executor::P1);
                        let result = if cancel.get() {
                            inner.as_mut().set(None);
                            assert_eq!(model.abandons.get(), 0);
                            Poll::Ready("cancelled")
                        } else {
                            assert!(
                                inner
                                    .as_mut()
                                    .as_pin_mut()
                                    .expect("live P1")
                                    .poll(cx)
                                    .is_pending()
                            );
                            Poll::Pending
                        };
                        model.executor.set(Executor::P2);
                        result
                    })
                    .await
                })
                .ok()
                .expect("fresh local registration");
            let mut outer = Box::pin(outer);
            if poll_inner {
                assert!(poll(outer.as_mut()).is_pending());
                assert_eq!(model.stop_loans.get(), 1);
                cancel.set(true);
            }
            assert_eq!(poll(outer.as_mut()), Poll::Ready("cancelled"));
            assert_eq!(model.starts.get(), usize::from(poll_inner));
            assert_eq!(model.stop_loans.get(), 0);
            assert_eq!(model.abandons.get(), 1);
            assert_eq!(model.drops.get(), 0);
        }));
    }
}

#[test]
fn local_runner_publishes_join_only_after_p2_finishes() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let scope = crate::LocalExecution
            .scope(&mut slot, Owner(model.clone()), async |frontend| {
                frontend.finish().wait().await.expect("ordinary P2 join")
            })
            .ok()
            .expect("fresh registration");
        let joined = finish(scope);
        assert_eq!(model.finishes.get(), 1);
        assert_eq!(model.drops.get(), 1);
        let (frontend, p2) = slot
            .reinstall(joined, Owner(model.clone()))
            .ok()
            .expect("joined slot");
        let join = frontend.finish();
        run_p2(p2).await;
        assert!(join.wait().await.is_ok());
        assert_eq!(model.finishes.get(), 2);
    }));
}

#[test]
fn dropping_outer_scope_ends_the_body_before_p2_cleanup_even_before_first_poll() {
    struct FrontendLoan(Rc<Model>);
    impl Drop for FrontendLoan {
        fn drop(&mut self) {
            self.0.frontend_loans.set(0);
        }
    }
    for poll_outer in [false, true] {
        let model = model(false);
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place();
            model.frontend_loans.set(1);
            let loan = FrontendLoan(model.clone());
            let outer = crate::LocalExecution
                .scope(&mut slot, Owner(model.clone()), async move |frontend| {
                    let _loan = loan;
                    let _frontend = frontend;
                    pending::<()>().await;
                })
                .ok()
                .expect("synchronous registration");
            let mut outer = Box::pin(outer);
            if poll_outer {
                assert!(poll(outer.as_mut()).is_pending());
            }
            assert_eq!(model.abandons.get(), 0);
            drop(outer);
            assert_eq!(model.frontend_loans.get(), 0);
            assert_eq!(model.stop_loans.get(), 0);
            assert_eq!(model.abandons.get(), 1);
            assert_eq!(model.drops.get(), 0);
        }));
    }
}

#[test]
fn returning_after_forgetting_the_frontend_cannot_leave_p2_registered() {
    let model = model(false);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let scope = crate::LocalExecution
            .scope(&mut slot, Owner(model.clone()), async |frontend| {
                core::mem::forget(frontend);
            })
            .ok()
            .expect("fresh registration");
        scope.await;
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(model.starts.get(), 0);
        assert_eq!(model.drops.get(), 0);
    }));
}

#[test]
fn completion_cannot_publish_join_before_the_old_p2_future_is_removed() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        let mut join = Box::pin(frontend.finish().wait());
        let mut p2 = Box::pin(p2.run());
        let completion = match poll(p2.as_mut()) {
            Poll::Ready(Some(completion)) => completion,
            _ => panic!("P2 stopped and destroyed its owner"),
        };
        assert_eq!(model.drops.get(), 1);
        assert!(poll(join.as_mut()).is_pending());
        drop(p2);
        completion.unregistered();
        assert!(matches!(poll(join.as_mut()), Poll::Ready(Ok(_))));
    }));
}

#[test]
fn a_lost_unregistration_completion_never_issues_joined() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        let join = frontend.finish();
        let completion = p2.run().await.expect("ordinary stop");
        drop(completion);
        let receipt = join.wait().await.err().expect("no unregistration proof");
        assert!(matches!(receipt.outcome(), Abandon::Unconfirmed(_)));
        assert_eq!(model.finishes.get(), 1);
        assert_eq!(model.abandons.get(), 0);
        assert!(slot.register(Owner(model.clone())).is_err());
    }));
}

#[test]
fn a_late_p2_answer_cannot_upgrade_the_supervisors_expired_result() {
    use oer_time::{Instant, Timer};

    let model = model(true);
    let clock = oer_time_virtual::VirtualClock::<4>::starting_at(Instant::EPOCH);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, p2) = slot
            .register(Owner(model.clone()))
            .ok()
            .expect("fresh slot");
        model.executor.set(Executor::P1);
        let mut join = Box::pin(
            frontend
                .abandon()
                .wait_within(clock.wait_until(Instant::from_micros(10))),
        );
        assert!(
            poll(join.as_mut()).is_pending(),
            "a request alone is not a P2 answer"
        );
        clock.advance_to(Instant::from_micros(10));
        let receipt = match poll(join.as_mut()) {
            Poll::Ready(Err(receipt)) => receipt,
            _ => panic!("the named bound completes with unconfirmed quarantine"),
        };
        assert!(matches!(receipt.outcome(), Abandon::Unconfirmed(_)));
        assert_eq!(model.abandons.get(), 0);
        model.executor.set(Executor::P2);
        assert!(p2.run().await.is_none());
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(model.starts.get(), 0);
        assert!(matches!(receipt.outcome(), Abandon::Unconfirmed(_)));
        drop(join);
        assert!(slot.register(Owner(model.clone())).is_err());
    }));
}
