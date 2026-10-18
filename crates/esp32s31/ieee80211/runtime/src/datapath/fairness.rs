//! Deficit round robin between the RX frontier and network TX.
//!
//! One signed balance counts RX frames serviced minus network TX frames
//! admitted. While both sides are backlogged, an RX turn spends the balance
//! and a network transaction repays it, and RX yields one network
//! transaction once it is a quantum ahead. As in deficit round robin, a side
//! with nothing to serve carries no debt: RX serviced while no network TX
//! waits, or TX admitted while no RX work waits, resets the balance. Without
//! that reset an RX-only period, such as a TCP download between its ACKs,
//! accumulates a debt that throttles RX to one frame per turn for as long as
//! any ACK is queued.

/// RX frames one turn may service before a waiting network transaction.
const RX_TX_FAIRNESS_QUANTUM_FRAMES: u32 = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct RxTxFairness {
    /// Signed RX-minus-TX frame balance. A negative value is retained across
    /// transactions so a large aggregate cannot erase the RX credit it
    /// consumed.
    rx_frame_deficit: i64,
}

impl RxTxFairness {
    pub(super) const fn new() -> Self {
        Self {
            rx_frame_deficit: 0,
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(super) const fn deficit(self) -> i64 {
        self.rx_frame_deficit
    }

    /// Whether RX is a quantum ahead of a waiting network transaction.
    pub(super) const fn network_turn_owed(self) -> bool {
        self.rx_frame_deficit >= RX_TX_FAIRNESS_QUANTUM_FRAMES as i64
    }

    /// The RX frames one protocol turn may service; `None` when no network
    /// TX waits and the role may use its own bounded batch.
    pub(super) fn rx_protocol_frame_budget(self, network_tx_pending: bool) -> Option<usize> {
        if !network_tx_pending {
            return None;
        }
        let remaining = i64::from(RX_TX_FAIRNESS_QUANTUM_FRAMES)
            .saturating_sub(self.rx_frame_deficit)
            .max(1);
        Some(usize::try_from(remaining).unwrap_or(usize::MAX))
    }

    /// Charge `frames` serviced RX frames. `network_tx_pending` is whether
    /// network TX waited while they were serviced.
    pub(super) fn charge_rx(&mut self, frames: u64, network_tx_pending: bool) {
        self.rx_frame_deficit = if network_tx_pending {
            self.rx_frame_deficit
                .saturating_add(i64::try_from(frames).unwrap_or(i64::MAX))
        } else {
            0
        };
    }

    /// Repay `frames` admitted network TX frames, at least one per
    /// transaction. `rx_pending` is whether RX work waited meanwhile.
    pub(super) fn charge_tx(&mut self, frames: usize, rx_pending: bool) {
        self.rx_frame_deficit = if rx_pending {
            self.rx_frame_deficit
                .saturating_sub(i64::try_from(frames.max(1)).unwrap_or(i64::MAX))
        } else {
            0
        };
    }
}

#[cfg(test)]
mod tests;
