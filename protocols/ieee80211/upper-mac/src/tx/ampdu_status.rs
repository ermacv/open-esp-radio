//! The terminal status of one logical A-MPDU exchange: aggregate attempts
//! and the individual retries of MPDUs taken out of it.

use oer_ieee80211_port::{MacTxResult, MacTxStatus};
use oer_ieee80211_wire::phy::PhyRate;

/// Terminal result of one logical A-MPDU exchange.
///
/// A successful aggregate may include several aggregate publications followed
/// by individual retries of MPDUs taken out of it.  `Delivered` therefore describes
/// the complete HMAC-visible exchange, not just receipt of one BlockAck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacAmpduTxResult {
    /// Every original MPDU was acknowledged, by BlockAck or by an individual
    /// retry.
    Delivered,
    /// The retry policy ended while at least one original MPDU was unacknowledged.
    Incomplete,
    /// The aggregate publication reached its terminal hardware timeout edge.
    HardwareTimeout,
    /// The aggregate publication reached its terminal collision limit.
    CollisionLimit,
}

/// Normalized terminal status for one logical A-MPDU exchange.
///
/// The aggregate rate is kept separately from the individual retries' final
/// rate.  Collapsing those into one `final_rate` would lose which part of the
/// exchange used which PHY policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacAmpduTxStatus<Rate = PhyRate> {
    pub result: MacAmpduTxResult,
    pub original_subframes: u16,
    /// Number of A-MPDU hardware publications, including the first one.
    pub aggregate_attempts: u8,
    pub aggregate_rate: Rate,
    /// Original MPDUs acknowledged by one or more BlockAck responses.
    pub block_acknowledged_subframes: u16,
    /// Signed ACK SNR sample of the last BlockAck, when the hardware
    /// reports a valid one.
    pub ack_snr_db: Option<i8>,
    /// MPDUs taken out of the aggregate after its last publication and
    /// sent as individual frames.
    pub individual_retries: MacIndividualRetries<Rate>,
}

/// Individual transmissions of MPDUs that left an A-MPDU exchange: a single
/// missing HT MPDU, or every live missing MPDU once the BlockAck agreement
/// ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacIndividualRetries<Rate = PhyRate> {
    /// MPDUs whose individual transmission was acknowledged.
    pub transmitted: u16,
    /// MPDUs whose individual transmission ended unacknowledged.
    pub failed: u16,
    /// Hardware publications across every individual transmission.
    pub attempts: u16,
    /// Rate of the last individual transmission, if any was made.
    pub final_rate: Option<Rate>,
}

impl<Rate> MacIndividualRetries<Rate> {
    pub const NONE: Self = Self {
        transmitted: 0,
        failed: 0,
        attempts: 0,
        final_rate: None,
    };

    /// Account one finished individual transmission.
    pub fn record(&mut self, status: MacTxStatus<Rate>) {
        if matches!(status.result, MacTxResult::Transmitted) {
            self.transmitted = self.transmitted.saturating_add(1);
        } else {
            self.failed = self.failed.saturating_add(1);
        }
        self.attempts = self.attempts.saturating_add(u16::from(status.attempts));
        self.final_rate = Some(status.final_rate);
    }
}

impl<Rate: Copy> MacAmpduTxStatus<Rate> {
    /// Original MPDUs proved delivered by BlockAck or by an acknowledged
    /// individual retry.
    pub const fn delivered_subframes(&self) -> u16 {
        self.block_acknowledged_subframes
            .saturating_add(self.individual_retries.transmitted)
    }

    pub const fn fully_delivered(&self) -> bool {
        matches!(self.result, MacAmpduTxResult::Delivered)
            && self.delivered_subframes() == self.original_subframes
    }

    /// Aggregate and ordinary hardware publications made for the complete
    /// exchange.
    pub const fn total_publication_attempts(&self) -> u16 {
        (self.aggregate_attempts as u16).saturating_add(self.individual_retries.attempts)
    }
}

#[cfg(test)]
mod tests;
