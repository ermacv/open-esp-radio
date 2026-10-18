//! Role-neutral aggregate-TX scheduler and transaction facts.

use oer_esp32s31_ieee80211_mac::irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};

use crate::datapath::WifiTxWake;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AggregateTxServiceEvent {
    Pending,
    Completion,
    HardwareTimeout,
    Collision,
    ExecutorDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AggregateTxServiceEventError {
    pub(crate) events: u32,
}

impl AggregateTxServiceEvent {
    pub(crate) fn classify(wake: WifiTxWake) -> Result<Self, AggregateTxServiceEventError> {
        let WifiTxWake::Interrupt { events } = wake else {
            return Ok(Self::ExecutorDeadline);
        };
        let tx_events = events & (EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT | EVENT_COLLISION);
        if tx_events.count_ones() > 1 {
            return Err(AggregateTxServiceEventError { events: tx_events });
        }
        Ok(match tx_events {
            EVENT_TX_COMPLETE => Self::Completion,
            EVENT_TX_TIMEOUT => Self::HardwareTimeout,
            EVENT_COLLISION => Self::Collision,
            _ => Self::Pending,
        })
    }
}

#[cfg(test)]
mod tests;
