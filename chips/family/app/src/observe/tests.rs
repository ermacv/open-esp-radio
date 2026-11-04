use super::*;
use oer_portable_embassy_net::RxEnqueueError;

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
            tx: BridgeTx::default(),
        }
    );
}

#[test]
fn a_bridge_turn_counts_its_outcome_a_full_queue_and_a_pool_without_free_buffers() {
    use oer_portable_embassy_net::ethernet::{TxOutcome, TxTurn};
    let bridge = BridgeCounter::new();
    let turn = |queued, waited, outcome| TxTurn {
        queued,
        capacity: 16,
        waited,
        outcome,
    };
    // 63 buffers: the station holds 40, the queue 3; 19 are free.
    bridge.turned(turn(3, false, TxOutcome::Moved), 40, 63);
    // The queue is full.
    bridge.turned(turn(16, false, TxOutcome::Moved), 30, 63);
    // All but the spare are taken: 45 + 16 + 1 waiting.
    bridge.turned(turn(16, true, TxOutcome::Held), 45, 63);
    bridge.turned(turn(0, false, TxOutcome::Idle), 10, 63);
    bridge.turned(turn(1, false, TxOutcome::Dropped), 10, 63);
    assert_eq!(
        bridge.read().tx,
        BridgeTx {
            turns: 5,
            moved: 2,
            held: 1,
            dropped: 1,
            queue_full: 2,
            starved: 1,
            queued_sum: 36,
            station_sum: 135,
        }
    );
}
