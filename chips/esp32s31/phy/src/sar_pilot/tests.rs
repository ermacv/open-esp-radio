use oer_esp32s31_hal::{
    domain::transaction::{Fault, FaultReason, Operation, PhyTransaction, TransactionSeat},
    root::RadioHardware,
};
use oer_memory::{AffineSpscQueue, CompactSpscCursor};
use std::{
    boxed::Box,
    panic::{AssertUnwindSafe, catch_unwind},
};

type Slot = AffineSpscQueue<PhyTransaction, 1, CompactSpscCursor>;
fn report(fault: Fault) -> ! {
    std::panic::panic_any(fault)
}

#[test]
fn cancellation_records_maintain_while_phy_is_in_request_handler_or_reply() {
    for hops in [0, 1, 2] {
        let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
        let (mut radio, _) = RadioHardware::for_host_test()
            .split::<crate::test_clocks::TestClockBackend, _>(
                crate::concurrent::ConcurrentPhy::new(),
                seat.claim().unwrap(),
            );
        let epoch = radio.begin_registration_for_host_test().epoch();
        let diagnostic = radio.fault_handle();
        let (returned, phy) = radio.detach(Operation::Maintain).split();
        let mut request = Slot::new();
        let mut reply = Slot::new();
        let (send, receive) = request.split();
        let (return_send, return_receive) = reply.split();
        assert!(send.try_send(phy).is_ok());
        let mut handler = if hops > 0 {
            Some(receive.try_receive().ok().unwrap())
        } else {
            None
        };
        if hops == 2 {
            assert!(return_send.try_send(handler.take().unwrap()).is_ok());
        }

        let stopped = catch_unwind(AssertUnwindSafe(|| drop(returned))).unwrap_err();
        let expected = Fault {
            reason: FaultReason::Cancelled(Operation::Maintain),
            registration: Some(epoch),
        };
        assert_eq!(*stopped.downcast::<Fault>().unwrap(), expected);
        assert_eq!(diagnostic.fault(), Some(expected));
        // No producer/consumer loss and no owner release on cancellation.
        assert_eq!(receive.len(), usize::from(hops == 0));
        assert_eq!(handler.is_some(), hops == 1);
        assert_eq!(return_receive.len(), usize::from(hops == 2));
    }
}
