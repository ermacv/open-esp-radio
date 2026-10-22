//! Short endpoint loans restricted to their original execution context.

use core::marker::PhantomData;

use super::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError, CachedSpscCursor, SpscCursor,
};

/// A short producer loan, shared only within one execution context.
///
/// The original affine endpoint remains exclusively borrowed. Copies permit
/// disjoint body views (including return-on-drop views) to publish complete
/// owners synchronously in that one context. This type is neither Send nor
/// Sync and has no reserve operation: no outstanding permit can race another
/// local copy's publication. Cross-context transfer uses the original affine
/// endpoint after all of these loans have ended.
pub struct LocalSpscSender<'p, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'p AffineSpscQueue<T, DEPTH, C>,
    local: PhantomData<*const ()>,
}

impl<T, const DEPTH: usize, C: SpscCursor> Copy for LocalSpscSender<'_, T, DEPTH, C> {}
impl<T, const DEPTH: usize, C: SpscCursor> Clone for LocalSpscSender<'_, T, DEPTH, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> LocalSpscSender<'_, T, DEPTH, C> {
    /// Publish one whole value; a full queue returns it unchanged.
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.queue.try_send(value)
    }

    pub fn is_empty(&self) -> bool {
        self.queue.len() == 0
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> AffineSpscSender<'_, T, DEPTH, C> {
    /// Lend synchronous local publications while suspending this endpoint.
    pub fn borrow_local(&mut self) -> LocalSpscSender<'_, T, DEPTH, C> {
        LocalSpscSender {
            queue: self.queue,
            local: PhantomData,
        }
    }
}

/// The unique short consumer loan in its original execution context.
/// It cannot be cloned, sent or shared between execution contexts.
pub struct LocalSpscReceiver<'p, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'p AffineSpscQueue<T, DEPTH, C>,
    local: PhantomData<*const ()>,
}

impl<T, const DEPTH: usize, C: SpscCursor> LocalSpscReceiver<'_, T, DEPTH, C> {
    /// Take the next whole owner in publication order.
    pub fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        self.queue.try_receive()
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> AffineSpscReceiver<'_, T, DEPTH, C> {
    /// Lend the single local consumer while suspending this endpoint.
    pub fn borrow_local(&mut self) -> LocalSpscReceiver<'_, T, DEPTH, C> {
        LocalSpscReceiver {
            queue: self.queue,
            local: PhantomData,
        }
    }
}
