//! Compile the production lifecycle entrypoints against virtual-time execution
//! kernels. These production constructors create the owned cancellation futures;
//! only the device kernels (which cannot run on the host) are modelled here.
use super::ConcurrentPhy;
use crate::{
    executor::{test_clock::VirtualClock, wait::PhyShortDelay},
    test_clocks::TestClockBackend,
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use oer_esp32s31_hal::{
    domain::transaction::{Fault, FaultReason, Operation, RadioTransaction, TransactionSeat},
    domain::{PlatformClockProvider, RadioDomain},
    power::{PlatformClock, PlatformClockBackend, PlatformClockError, PlatformClockGuard},
    root::RadioHardware,
};
use oer_time::{Clock, Duration, Timer};
use std::{
    boxed::Box,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[path = "../target_port/concurrent/operations.rs"]
mod operations;

struct PhyRegisterConfig;
type ConcurrentPhyRegistration = ();
type ConcurrentPhyRegisterFailure = ();
type ConcurrentPhyTrackingError = ();
type ConcurrentRfError = ();
type PhyParamTrackingOutcome = ();
trait PhyTargetObserver {}
impl PhyTargetObserver for () {}
struct Delay;
impl PhyShortDelay for Delay {
    const MAX_MICROS: u32 = 0;
    fn settle_micros(_: u32) -> bool {
        panic!("no hardware settle in an ownership test")
    }
}
struct Clocks;
impl PlatformClockProvider for Clocks {
    type Backend = TestClockBackend;
    fn acquire(
        &self,
        _: PlatformClock,
    ) -> Result<PlatformClockGuard<Self::Backend>, PlatformClockError> {
        panic!("the execution model never acquires hardware clocks")
    }
}
async fn wait<B: PlatformClockBackend>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ()>) {
    timer
        .wait_until(timer.now().saturating_add(Duration::from_micros(100)))
        .await;
    (initial.reunite(), Ok(()))
}
async fn execute_register<B: PlatformClockBackend, P, D, O>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    _: &mut P,
    _: &impl PlatformClockProvider<Backend = B>,
    _: PhyRegisterConfig,
    _: O,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ()>) {
    let _short_delay = core::marker::PhantomData::<D>;
    wait(timer, initial).await
}
async fn execute_close<B: PlatformClockBackend, P, D>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    _: &mut P,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ()>) {
    let _short_delay = core::marker::PhantomData::<D>;
    wait(timer, initial).await
}
async fn execute_wake<B: PlatformClockBackend, D>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    _: &impl PlatformClockProvider<Backend = B>,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ()>) {
    let _short_delay = core::marker::PhantomData::<D>;
    wait(timer, initial).await
}
async fn execute_maintain<B: PlatformClockBackend, P, D, O>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    _: &mut P,
    _: O,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ()>) {
    let _short_delay = core::marker::PhantomData::<D>;
    wait(timer, initial).await
}

fn report(fault: Fault) -> ! {
    std::panic::panic_any(fault)
}
type Domain = RadioDomain<TestClockBackend, ConcurrentPhy>;
type Lifecycle<'a> = Pin<Box<dyn Future<Output = (Domain, Result<(), ()>)> + 'a>>;
fn future<'a>(
    operation: Operation,
    radio: Domain,
    timer: &'a VirtualClock,
    platform: &'a mut (),
) -> Lifecycle<'a> {
    match operation {
        Operation::Register => Box::pin(operations::register_concurrent_phy::<_, _, Delay, _>(
            timer,
            radio,
            platform,
            &Clocks,
            PhyRegisterConfig,
            (),
        )),
        Operation::Maintain => Box::pin(operations::maintain_concurrent_phy::<_, _, Delay, _>(
            timer,
            radio,
            platform,
            (),
        )),
        Operation::Wake => Box::pin(operations::wake_concurrent_rf::<_, Delay>(
            timer, radio, &Clocks,
        )),
        Operation::Close => Box::pin(operations::close_concurrent_rf::<_, _, Delay>(
            timer, radio, platform,
        )),
        _ => panic!("not a lifecycle operation"),
    }
}
#[test]
fn production_lifecycle_futures_record_the_operation_before_poll_and_while_waiting() {
    for operation in [
        Operation::Register,
        Operation::Maintain,
        Operation::Wake,
        Operation::Close,
    ] {
        for polled in [false, true] {
            let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
            let (mut radio, _) = RadioHardware::for_host_test()
                .split::<TestClockBackend, _>(ConcurrentPhy::new(), seat.claim().unwrap());
            let epoch = radio.begin_registration_for_host_test().epoch();
            let handle = radio.fault_handle();
            let timer = VirtualClock::new();
            let mut platform = ();
            let mut future = future(operation, radio, &timer, &mut platform);
            if polled {
                assert!(
                    future
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending()
                );
            }
            let error = catch_unwind(AssertUnwindSafe(|| drop(future))).unwrap_err();
            let fault = *error.downcast::<Fault>().unwrap();
            assert_eq!(fault.reason, FaultReason::Cancelled(operation));
            assert_eq!(fault.registration, Some(epoch));
            assert_eq!(handle.fault(), Some(fault));
        }
    }
}
#[test]
fn production_lifecycle_futures_reunite_after_virtual_time_without_a_cancellation() {
    for operation in [
        Operation::Register,
        Operation::Maintain,
        Operation::Wake,
        Operation::Close,
    ] {
        let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
        let (radio, _) = RadioHardware::for_host_test()
            .split::<TestClockBackend, _>(ConcurrentPhy::new(), seat.claim().unwrap());
        let handle = radio.fault_handle();
        let timer = VirtualClock::new();
        let mut platform = ();
        let mut future = future(operation, radio, &timer, &mut platform);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        timer.advance(Duration::from_micros(100)).unwrap();
        let Poll::Ready((_radio, result)) = future.as_mut().poll(&mut cx) else {
            panic!("virtual completion")
        };
        assert_eq!(result, Ok(()));
        assert_eq!(handle.fault(), None);
    }
}
