use super::*;
use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_radio_port::{Permits, branded};

use core::cell::RefCell;
use oer_radio_port::{Permit, PermitPool, PermitSupply, Seat};

struct TestPool<'id, Kind>(RefCell<Option<Permit<'id, Kind>>>);

impl<'id, Kind> TestPool<'id, Kind> {
    fn new(supply: PermitSupply<'id, Kind, 1>) -> Self {
        let [permit] = supply.into_permits();
        Self(RefCell::new(Some(permit)))
    }
}

impl<'id, Kind> PermitPool<'id, Kind> for TestPool<'id, Kind> {
    async fn take<'a>(&'a self, _: &'a mut Seat<'id, Kind>) -> Permit<'id, Kind> {
        self.0
            .borrow_mut()
            .take()
            .expect("test capacity is available")
    }

    fn release(&self, permit: Permit<'id, Kind>) {
        assert!(self.0.borrow_mut().replace(permit).is_none());
    }
}
fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("test pool has available capacity"),
    }
}

#[test]
fn admission_borrow_conflict_returns_the_original_guard() {
    ready(branded::<Permits<Operation, 1, 1>, _>(async |mint| {
        let (permits, [mut seat]) =
            OperationPermits::new(mint.into_pools(), |supply, _| TestPool::new(supply));
        let permit = permits.guard(ready(permits.pool().take(&mut seat)));
        let borrow = permits.in_flight.borrow_mut();
        let interface = Interface::new(1);
        let id = RequestId::new(17);
        let Err(refused) = permits.admit(interface, id, permit) else {
            panic!("exclusive access must refuse admission");
        };
        assert_eq!((refused.interface, refused.id), (interface, id));
        assert!(borrow.is_none());
        drop(borrow);
        assert!(permits.admit(interface, id, refused.permit).is_ok());
        assert_eq!(permits.in_flight(), Ok(Some((interface, id))));
        assert!(permits.complete(id).is_ok());
        drop(permits.guard(ready(permits.pool().take(&mut seat))));
    }));
}

#[test]
fn completion_and_query_conflicts_retain_the_admitted_owner() {
    ready(branded::<Permits<Operation, 1, 1>, _>(async |mint| {
        let (permits, [mut seat]) =
            OperationPermits::new(mint.into_pools(), |supply, _| TestPool::new(supply));
        let permit = permits.guard(ready(permits.pool().take(&mut seat)));
        let interface = Interface::new(0);
        let id = RequestId::new(23);
        assert!(permits.admit(interface, id, permit).is_ok());
        let borrow = permits.in_flight.borrow_mut();
        assert!(matches!(permits.complete(id),
            Err(CompletionError::BorrowConflict { received }) if received == id));
        assert_eq!(permits.in_flight(), Err(WorkStateBorrowConflict));
        assert_eq!(
            borrow.as_ref().map(|held| (held.interface, held.id)),
            Some((interface, id))
        );
        drop(borrow);
        assert!(permits.complete(id).is_ok());
        assert_eq!(permits.in_flight(), Ok(None));
        drop(permits.guard(ready(permits.pool().take(&mut seat))));
    }));
}
