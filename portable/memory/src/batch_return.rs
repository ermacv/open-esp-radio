//! A resource's batched return, separate from the resource's individual Drop.

/// An affine owner whose return can share a publication with other owners.
/// `return_in` gives up the payload immediately. `Batch` retains any deferred
/// accounting/notification, and publishes it on Drop. A caller ends the batch
/// before publishing credits which would let another owner reuse the payload.
pub trait BatchReturn: Sized {
    type Batch: Default;

    fn return_in(self, _batch: &mut Self::Batch) {
        drop(self);
    }
}

impl BatchReturn for core::convert::Infallible {
    type Batch = ();
}
