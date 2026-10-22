//! Generative pool and reset-epoch authority. Neither token is reusable.

use core::marker::PhantomData;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Brand<'pool, 'epoch> {
    _pool: PhantomData<fn(&'pool mut ()) -> &'pool mut ()>,
    _epoch: PhantomData<fn(&'epoch mut ()) -> &'epoch mut ()>,
}

impl Brand<'_, '_> {
    pub(super) const fn new() -> Self {
        Self {
            _pool: PhantomData,
            _epoch: PhantomData,
        }
    }
}

/// An affine, fresh identity for one pool. Construction is scoped so its
/// invariant lifetime cannot be unified with a different pool's identity.
pub struct PoolScope<'pool>(PhantomData<fn(&'pool mut ()) -> &'pool mut ()>);

/// An affine, fresh identity for one reset epoch.
pub struct EpochScope<'epoch>(PhantomData<fn(&'epoch mut ()) -> &'epoch mut ()>);

/// Run with a pool identity that cannot escape or be reused for another pool.
pub fn with_pool<R>(run: impl for<'pool> FnOnce(PoolScope<'pool>) -> R) -> R {
    run(PoolScope(PhantomData))
}

/// Run with a reset epoch that cannot escape or alias another installation.
pub fn with_epoch<R>(run: impl for<'epoch> FnOnce(EpochScope<'epoch>) -> R) -> R {
    run(EpochScope(PhantomData))
}

/// A task retains the fresh pool brand across suspension without boxing its
/// future or lending the brand beyond the task body.
pub async fn with_pool_async<R>(run: impl for<'pool> AsyncFnOnce(PoolScope<'pool>) -> R) -> R {
    run(PoolScope(PhantomData)).await
}

/// Fresh reset-epoch authority for an asynchronous task body.
pub async fn with_epoch_async<R>(run: impl for<'epoch> AsyncFnOnce(EpochScope<'epoch>) -> R) -> R {
    run(EpochScope(PhantomData)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::engine::{
        DtmStorage, HostDma, SchedulerAllocationConfig, SchedulerRolePoolStorage,
    };
    use crate::dma_static::Placed;
    use core::{
        cell::Cell,
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use std::boxed::Box;

    #[test]
    fn a_task_retains_its_branded_pool_and_lease_across_suspension() {
        let stage = Cell::new(0);
        let future = with_pool_async(async |pool| {
            with_epoch_async(async |epoch| {
                let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<DtmStorage, 1>::new()));
                let numbers = SchedulerAllocationConfig::new(0, 0, 0)
                    .expect("test limits fit")
                    .direct_test_mode();
                let mut pool = pool
                    .place::<HostDma<0x2f00_1000>, _, 1>(epoch, Placed::host(storage), numbers)
                    .expect("the scoped pool fits");
                let lease = pool.acquire().expect("one slot");
                stage.set(1);
                let mut yielded = false;
                core::future::poll_fn(|_| {
                    if core::mem::replace(&mut yielded, true) {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                pool.release(lease)
                    .expect("the same pool and epoch remain owned");
                stage.set(2);
            })
            .await;
        });
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(future.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(stage.get(), 1);
        assert_eq!(future.as_mut().poll(&mut context), Poll::Ready(()));
        assert_eq!(stage.get(), 2);
    }
}
