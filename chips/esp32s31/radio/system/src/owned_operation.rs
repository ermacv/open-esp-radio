//! Poll an owned PHY operation into retained output storage, then return custody.
use core::{future::Future, mem::MaybeUninit, pin::Pin};
use oer_esp32s31_hal::{domain::RadioDomain, power::PlatformClockBackend};
use oer_memory::Placed;

#[inline(always)]
pub(super) fn run<B: PlatformClockBackend, A, R, E, U, F>(
    operation: Pin<&mut F>,
    destination: &mut Option<RadioDomain<B, A>>,
    finish: impl FnOnce(
        &mut Option<RadioDomain<B, A>>,
        Placed<'_, (RadioDomain<B, A>, Result<R, E>)>,
    ) -> U,
) -> impl Future<Output = U>
where
    F: Future<Output = (RadioDomain<B, A>, Result<R, E>)>,
{
    async move {
        let mut completion = MaybeUninit::uninit();
        let returned = oer_memory::poll_placed(operation, &mut completion).await;
        finish(destination, returned)
    }
}

#[inline(never)]
pub(super) fn restore<B: PlatformClockBackend, A, R, E>(
    destination: &mut Option<RadioDomain<B, A>>,
    returned: Placed<'_, (RadioDomain<B, A>, Result<R, E>)>,
) -> Result<R, E> {
    let (radio, result) = returned.split();
    *destination = Some(radio.read());
    result.read()
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::test_clocks::TestClockBackend;
    use core::{
        cell::Cell,
        future::pending,
        task::{Context, Poll, Waker},
    };
    use oer_esp32s31_hal::{
        domain::transaction::{Fault, FaultReason, Operation, TransactionSeat},
        root::RadioHardware,
    };
    use std::{
        boxed::Box,
        panic::{AssertUnwindSafe, catch_unwind},
        rc::Rc,
    };

    struct Payload(Rc<Cell<u32>>);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn report(fault: Fault) -> ! {
        std::panic::panic_any(fault)
    }

    #[test]
    fn dropping_placed_registration_after_start_records_cancellation_and_quarantines() {
        let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
        let drops = Rc::new(Cell::new(0));
        let (radio, _) = RadioHardware::for_host_test()
            .split::<TestClockBackend, _>(Payload(drops.clone()), seat.claim().unwrap())
            .expect("identity capacity");
        let initial = radio.detach(Operation::Register);
        let begun = Cell::new(None);
        let mut destination = None;
        let operation = async {
            let mut radio = initial.reunite();
            begun.set(Some(radio.begin_registration_for_host_test().epoch()));
            let transaction = radio.detach(Operation::Register);
            pending::<()>().await;
            (transaction.reunite(), Ok::<(), ()>(()))
        };
        let parent = async {
            let mut operation = core::pin::pin!(operation);
            run(operation.as_mut(), &mut destination, restore).await
        };
        let mut future = Box::pin(parent);
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        let epoch = begun
            .get()
            .expect("the registration started before cancellation");
        let error = catch_unwind(AssertUnwindSafe(|| drop(future))).unwrap_err();
        let fault = *error.downcast::<Fault>().unwrap();
        assert_eq!(fault.reason, FaultReason::Cancelled(Operation::Register));
        assert_eq!(fault.registration, Some(epoch));
        assert_eq!(seat.fault(), Some(fault));
        assert!(destination.is_none());
        assert_eq!(
            drops.get(),
            0,
            "the returned owner's payload stays quarantined"
        );
    }

    #[test]
    fn completed_operation_returns_its_domain_and_result_once() {
        let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
        let drops = Rc::new(Cell::new(0));
        let (radio, _) = RadioHardware::for_host_test()
            .split::<TestClockBackend, _>(Payload(drops.clone()), seat.claim().unwrap())
            .expect("identity capacity");
        let transaction = radio.detach(Operation::Register);
        let mut destination = None;
        {
            let parent = async {
                let mut operation =
                    core::pin::pin!(async { (transaction.reunite(), Ok::<_, ()>(17)) });
                run(operation.as_mut(), &mut destination, restore).await
            };
            let mut future = core::pin::pin!(parent);
            assert_eq!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(Ok(17))
            );
        }
        assert!(destination.is_some());
        assert_eq!(seat.fault(), None);
        assert_eq!(drops.get(), 0);
        drop(destination);
        assert_eq!(drops.get(), 1);
    }
}
