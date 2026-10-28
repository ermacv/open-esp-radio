//! Keep owned PHY results in the parent future until custody is returned.
use core::{future::Future, mem::MaybeUninit, pin::Pin};
use oer_esp32s31_phy::executor::boundary::Boundary;
use oer_memory::Placed;

// LLVM retains whole-future construction scratch even for uninitialized
// suspension fields. Keep it off the stack while the child is being polled.
#[inline(never)]
pub fn construct<F: Future>(initialize: impl FnOnce() -> F) -> F {
    initialize()
}

// Construct directly in the parent's return place: an async fn adds a
// whole-future temporary before it can be pinned by the caller.
#[inline(always)]
pub fn run<D, R, U, F>(
    operation: Pin<&mut F>,
    destination: &mut Option<D>,
    finish: impl FnOnce(&mut Option<D>, Placed<'_, (D, R)>) -> U,
) -> impl Future<Output = U>
where
    F: Future<Output = (D, R)>,
{
    async move {
        let mut completion = MaybeUninit::uninit();
        // Borrow the existing owner through PHY's out-of-line poll boundary.
        // Its reference is small; the owned future is never wrapped by value.
        let mut operation = core::pin::pin!(Boundary::new(operation));
        let returned = oer_memory::poll_placed(operation.as_mut(), &mut completion).await;
        finish(destination, returned)
    }
}

#[inline(never)]
pub fn restore<D, R>(destination: &mut Option<D>, returned: Placed<'_, (D, R)>) -> R {
    let (domain, result) = returned.split();
    *destination = Some(domain.read());
    result.read()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        future::poll_fn,
        task::{Context, Poll, Waker},
    };

    struct Owner<'a>(&'a Cell<usize>);
    impl Drop for Owner<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn construction_transfers_custody_without_polling_the_operation() {
        let drops = Cell::new(0);
        let polled = Cell::new(false);
        let owner = Owner(&drops);
        let operation = construct(|| async {
            polled.set(true);
            core::future::pending::<()>().await;
            owner
        });
        assert!(!polled.get());
        assert_eq!(drops.get(), 0);
        drop(operation);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn pending_completion_keeps_custody_then_returns_it_once() {
        let drops = Cell::new(0);
        let owner = Owner(&drops);
        let mut destination = None;
        {
            let mut ready = false;
            let mut operation = core::pin::pin!(async move {
                poll_fn(|_| {
                    if core::mem::replace(&mut ready, true) {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                (owner, 17)
            });
            let mut parent = core::pin::pin!(run(operation.as_mut(), &mut destination, restore));
            let mut context = Context::from_waker(Waker::noop());
            assert!(parent.as_mut().poll(&mut context).is_pending());
            assert_eq!(drops.get(), 0);
            assert_eq!(parent.as_mut().poll(&mut context), Poll::Ready(17));
        }
        assert!(destination.is_some());
        assert_eq!(drops.get(), 0);
        drop(destination);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn cancellation_drops_the_operation_without_publishing_an_owner() {
        let drops = Cell::new(0);
        let finished = Cell::new(false);
        let owner = Owner(&drops);
        let mut destination = None;
        {
            let mut operation = core::pin::pin!(async move {
                core::future::pending::<()>().await;
                (owner, ())
            });
            let mut parent = core::pin::pin!(run(
                operation.as_mut(),
                &mut destination,
                |destination, returned| {
                    finished.set(true);
                    restore(destination, returned)
                },
            ));
            assert!(
                parent
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        assert!(!finished.get());
        assert!(destination.is_none());
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn terminal_result_can_be_inspected_and_dropped_in_its_place() {
        let domain_drops = Cell::new(0);
        let result_drops = Cell::new(0);
        let mut destination = None;
        {
            let mut operation = core::pin::pin!(async {
                (Owner(&domain_drops), Err::<(), _>(Owner(&result_drops)))
            });
            let mut parent = core::pin::pin!(run(
                operation.as_mut(),
                &mut destination,
                |destination, returned| {
                    let (domain, result) = returned.split();
                    *destination = Some(domain.read());
                    result.is_err()
                },
            ));
            assert_eq!(
                parent
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(true)
            );
        }
        assert!(destination.is_some());
        assert_eq!(domain_drops.get(), 0);
        assert_eq!(result_drops.get(), 1);
        drop(destination);
        assert_eq!(domain_drops.get(), 1);
    }
}
