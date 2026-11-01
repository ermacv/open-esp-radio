use super::*;
use oer_embassy_net::RxEnqueueError;

#[test]
fn the_bridge_counts_each_outcome_apart() {
    let bridge = BridgeCounter::new();
    bridge.received();
    bridge.received();
    bridge.refused(RxEnqueueError::PoolExhausted);
    bridge.refused(RxEnqueueError::QueueFull);
    bridge.refused(RxEnqueueError::LinkDown);
    bridge.copied();
    assert_eq!(
        bridge.read(),
        Bridge {
            received: 2,
            refused_pool: 1,
            refused_queue: 1,
            refused_other: 1,
            copied: 1,
        }
    );
}
