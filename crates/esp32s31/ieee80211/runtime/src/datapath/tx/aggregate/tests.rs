use oer_esp32s31_ieee80211_mac::irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};

use super::*;

#[test]
fn unrelated_irq_bits_do_not_forge_a_tx_event() {
    assert_eq!(
        AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
            events: 0x8000_0000,
        }),
        Ok(AggregateTxServiceEvent::Pending)
    );
}

#[test]
fn deadline_and_each_hardware_reason_remain_distinct() {
    assert_eq!(
        AggregateTxServiceEvent::classify(WifiTxWake::Deadline),
        Ok(AggregateTxServiceEvent::ExecutorDeadline)
    );
    assert_eq!(
        AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE,
        }),
        Ok(AggregateTxServiceEvent::Completion)
    );
    assert_eq!(
        AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
            events: EVENT_TX_TIMEOUT,
        }),
        Ok(AggregateTxServiceEvent::HardwareTimeout)
    );
    assert_eq!(
        AggregateTxServiceEvent::classify(WifiTxWake::Interrupt {
            events: EVENT_COLLISION,
        }),
        Ok(AggregateTxServiceEvent::Collision)
    );
}

#[test]
fn conflicting_terminal_events_fail_closed() {
    let events = EVENT_TX_COMPLETE | EVENT_COLLISION;
    assert_eq!(
        AggregateTxServiceEvent::classify(WifiTxWake::Interrupt { events }),
        Err(AggregateTxServiceEventError { events })
    );
}
