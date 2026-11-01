//! The station's transmit contexts while their tickets are in the port.
//!
//! A submission hands the port its ticket, its MPDU or aggregate and a unit
//! context: the operation's own context (its link epoch, admission permit,
//! frame and order) stays here, at its ticket's place, and the completion's
//! ticket finds it. The port never carries, copies or returns it, so every
//! completion it queues is that much smaller.
//!
//! A ticket is affine and its index is its place in the backend's ticket
//! pool, so one place per ticket of that pool holds every context in
//! flight. A place beyond the storage, a taken place at submission or an
//! empty one at completion is a backend contract violation; the service
//! retains the context it holds in the fault.

/// Fixed storage of one context per transmit ticket of the backend's pool.
pub trait TxContextStorage<T>: Sized {
    /// Every place empty.
    fn empty() -> Self;
    /// The place of the ticket at `index`; `None` beyond the storage.
    fn place(&mut self, index: u8) -> Option<&mut Option<T>>;
}

impl<T, const N: usize> TxContextStorage<T> for [Option<T>; N] {
    fn empty() -> Self {
        [const { None }; N]
    }

    fn place(&mut self, index: u8) -> Option<&mut Option<T>> {
        self.get_mut(usize::from(index))
    }
}

/// Keep `context` at the place of the ticket at `index`; the context back
/// when that place is taken or beyond the storage.
pub(super) fn keep<T>(
    storage: &mut impl TxContextStorage<T>,
    index: u8,
    context: T,
) -> Result<(), T> {
    match storage.place(index) {
        Some(place @ None) => {
            *place = Some(context);
            Ok(())
        }
        _ => Err(context),
    }
}

/// The context at the place of the ticket at `index`, leaving it empty.
pub(super) fn take<T>(storage: &mut impl TxContextStorage<T>, index: u8) -> Option<T> {
    storage.place(index)?.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_waits_at_its_tickets_place_until_its_completion_takes_it() {
        let mut storage = <[Option<u32>; 4]>::empty();
        assert_eq!(keep(&mut storage, 2, 20), Ok(()));
        assert_eq!(keep(&mut storage, 0, 10), Ok(()));
        // A taken place and one beyond the storage refuse, returning the
        // context.
        assert_eq!(keep(&mut storage, 2, 21), Err(21));
        assert_eq!(keep(&mut storage, 4, 40), Err(40));
        assert_eq!(take(&mut storage, 2), Some(20));
        assert_eq!(take(&mut storage, 2), None, "taken once");
        assert_eq!(take(&mut storage, 9), None);
        assert_eq!(keep(&mut storage, 2, 22), Ok(()));
        assert_eq!(take(&mut storage, 0), Some(10));
        assert_eq!(take(&mut storage, 2), Some(22));
    }
}
