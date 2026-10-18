use super::*;

#[test]
fn mailbox_preserves_typed_hardware_order_and_capacity() {
    let mut mailbox = AccessPointProtocolMailbox::<1>::new();
    let reset =
        AccessPointProtocolAction::Hardware(AccessPointHardwareAction::ResetRxBlockAckWindow {
            hardware_index: 2,
            tid: 6,
            starting_sequence: SequenceNumber::new(0x345).unwrap(),
            window: 64,
        });
    {
        let mut publisher = mailbox.publisher();
        publisher.try_publish(reset).unwrap();
        assert_eq!(publisher.try_publish(reset), Err(reset));
    }
    let mut receiver = mailbox.receiver();
    assert_eq!(receiver.try_receive(), Some(reset));
    assert_eq!(receiver.try_receive(), None);
}
