//! Permission-indexed cells, with the audited affine SPSC ring for order.
use super::*;

/// An affine permission whose place is the storage credit for its value.
///
/// # Safety
/// `queue_index` is stable and less than N. Two values that can coexist in
/// one queue must never own the same index. An index may be used again only
/// after its previous value has been received completely or destroyed. The
/// permission transfer establishing reuse must also order the previous read
/// before the new write across cores. A brand / sealed constructor must keep
/// independent mint domains from being mixed in one queue. Implementations
/// must not grant these properties merely by checking occupancy at runtime.
///
/// Usually a mint issues each permission once, and a terminal returns it
/// after receive has freed its cell. This trait carries that external proof;
/// the queue derives its capacity proof from distinct indices in `0..N`.
#[allow(
    unsafe_code,
    reason = "implementors prove unique affine cell ownership"
)]
pub unsafe trait UniqueQueueIndex<const N: usize> {
    fn queue_index(&self) -> usize;
}

/// One cell per permission and a FIFO of indices, with infallible publication.
/// The queue contains no free list or runtime credit pool. Receiving moves the
/// whole owner out before its permission can be returned in a response.
///
/// Uniqueness of N possible indices proves at most N outstanding values. A
/// value still held by the producer therefore proves a vacant cell AND room
/// in the order ring. Only the unsafe index trait supplies that authority;
/// ordinary uncredited values cannot use [`AffineIndexedSpscSender::send`].
pub struct AffineIndexedSpscQueue<T, const N: usize> {
    values: [UnsafeCell<MaybeUninit<T>>; N],
    order: AffineSpscQueue<usize, N>,
}
#[allow(
    unsafe_code,
    reason = "unique indexed cells use the audited SPSC publication ring"
)]
// SAFETY: split grants one non-Sync producer and consumer. Release publication
// through order initializes both the index and its value before Acquire receive.
// UniqueQueueIndex prevents concurrent reuse of a cell; the returned affine
// permission orders its next write after the complete previous read.
unsafe impl<T: Send, const N: usize> Sync for AffineIndexedSpscQueue<T, N> {}

impl<T, const N: usize> AffineIndexedSpscQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            order: AffineSpscQueue::new(),
        }
    }
    pub fn split(
        &mut self,
    ) -> (
        AffineIndexedSpscSender<'_, T, N>,
        AffineIndexedSpscReceiver<'_, T, N>,
    ) {
        (
            AffineIndexedSpscSender {
                queue: self,
                _not_sync: PhantomData,
            },
            AffineIndexedSpscReceiver {
                queue: self,
                _not_sync: PhantomData,
            },
        )
    }
    fn receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        let index = self.order.try_receive()?;
        #[allow(unsafe_code, reason = "one published index owns one initialized value")]
        // SAFETY: the order ring's Acquire observed the producer's value and
        // bounded index. Only this consumer reads it. Its permission cannot
        // return to the producer until assume_init_read has completed.
        Ok(unsafe { (*self.values.get_unchecked(index).get()).assume_init_read() })
    }
}
impl<T, const N: usize> Default for AffineIndexedSpscQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T, const N: usize> Drop for AffineIndexedSpscQueue<T, N> {
    fn drop(&mut self) {
        // The exclusive storage borrow has ended: no endpoint exists. The
        // ordinary ring tracks exactly the initialized cells, at most N.
        while let Ok(value) = self.receive() {
            drop(value);
        }
    }
}

pub struct AffineIndexedSpscSender<'q, T, const N: usize> {
    queue: &'q AffineIndexedSpscQueue<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}
impl<T: UniqueQueueIndex<N>, const N: usize> AffineIndexedSpscSender<'_, T, N> {
    /// Consume the value and its unique cell permission, with no refusal or wait.
    pub fn send(&mut self, value: T) {
        let index = value.queue_index();
        let producer = self.queue.order.producer.0.load(Ordering::Relaxed);
        #[allow(unsafe_code, reason = "the affine index owns this vacant cell")]
        // SAFETY: the trait proves index < N and no resident value at this
        // index. Reuse follows the prior complete read through permission
        // transfer. Publication below orders initialization before receive.
        unsafe {
            (*self.queue.values.get_unchecked(index).get()).write(value);
        }
        // The value owns one of N distinct indices outside the order ring:
        // at most N-1 indices can still be queued. Thus this producer cursor
        // is vacant. On a ring wrap, among the returns already observed by
        // this producer there must be one from this slot or a later receive:
        // N distinct permissions cannot account for more outstanding sends.
        // That return orders this write after the old ring read, because the
        // sole consumer reads in FIFO order. The current value need not be
        // that returned permission (it may be a previously unused place).
        // This is the same audited ring publication as a reserved send; the
        // index proof replaces the fallible occupancy check, not its algorithm.
        AffineSpscSendPermit {
            queue: &self.queue.order,
            producer,
            _not_sync: PhantomData,
        }
        .send(index);
    }
}

pub struct AffineIndexedSpscReceiver<'q, T, const N: usize> {
    queue: &'q AffineIndexedSpscQueue<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}
impl<T, const N: usize> AffineIndexedSpscReceiver<'_, T, N> {
    pub fn try_receive(&mut self) -> Result<T, AffineSpscTryReceiveError> {
        self.queue.receive()
    }
}

#[cfg(test)]
mod tests;
