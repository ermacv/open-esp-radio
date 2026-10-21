//! Short endpoint loans restricted to their original execution context.

use core::marker::PhantomData;

use super::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError,
};

/// A short producer loan, shared only within one execution context.
///
/// The original affine endpoint remains exclusively borrowed. Copies permit
/// disjoint body views (including return-on-drop views) to publish complete
/// owners synchronously in that one context. This type is neither Send nor
/// Sync and has no reserve operation: no outstanding permit can race another
/// local copy's publication. Cross-context transfer uses the original affine
/// endpoint after all of these loans have ended.
pub struct LocalSpscSender<'p, T, const DEPTH: usize> {
    queue: &'p AffineSpscQueue<T, DEPTH>,
    local: PhantomData<*const ()>,
}

impl<T, const DEPTH: usize> Copy for LocalSpscSender<'_, T, DEPTH> {}
impl<T, const DEPTH: usize> Clone for LocalSpscSender<'_, T, DEPTH> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, const DEPTH: usize> LocalSpscSender<'_, T, DEPTH> {
    /// Publish one whole value; a full queue returns it unchanged.
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.queue.try_send(value)
    }
}

impl<T, const DEPTH: usize> AffineSpscSender<'_, T, DEPTH> {
    /// Lend synchronous local publications while suspending this endpoint.
    pub fn borrow_local(&mut self) -> LocalSpscSender<'_, T, DEPTH> {
        LocalSpscSender {
            queue: self.queue,
            local: PhantomData,
        }
    }
}

/// The unique short consumer loan in its original execution context.
/// It cannot be cloned, sent or shared between execution contexts.
pub struct LocalSpscReceiver<'p, T, const DEPTH: usize> {
    queue: &'p AffineSpscQueue<T, DEPTH>,
    local: PhantomData<*const ()>,
}

impl<T, const DEPTH: usize> LocalSpscReceiver<'_, T, DEPTH> {
    /// Take the next whole owner in publication order.
    pub fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        self.queue.try_receive()
    }
}

impl<T, const DEPTH: usize> AffineSpscReceiver<'_, T, DEPTH> {
    /// Lend the single local consumer while suspending this endpoint.
    pub fn borrow_local(&mut self) -> LocalSpscReceiver<'_, T, DEPTH> {
        LocalSpscReceiver {
            queue: self.queue,
            local: PhantomData,
        }
    }
}
