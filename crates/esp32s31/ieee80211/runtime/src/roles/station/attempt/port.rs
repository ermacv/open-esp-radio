use super::*;

/// Stateless target port. All unique resources live in `Owner`; the phantom
/// type only selects the concrete owner graph for the trait implementation.
pub struct StaAttemptTargetPort<O> {
    _owner: PhantomData<fn() -> O>,
}

impl<O> StaAttemptTargetPort<O> {
    pub const fn new() -> Self {
        Self {
            _owner: PhantomData,
        }
    }
}

impl<O> Clone for StaAttemptTargetPort<O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O> Copy for StaAttemptTargetPort<O> {}

impl<O> Default for StaAttemptTargetPort<O> {
    fn default() -> Self {
        Self::new()
    }
}
