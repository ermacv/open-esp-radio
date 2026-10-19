//! TX/RX statistics of the vendor driver's debug build
//! (`CONFIG_IEEE802154_TXRX_STATISTIC`, `esp_ieee802154_debug.c` at ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`): operation counts the driver
//! keeps and the MAC diagnostic counters it drains at every interrupt.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver/esp_ieee802154_debug.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

use crate::ll::Ieee802154LowLevel;
use crate::types::{Ieee802154DebugCounter, Ieee802154Event, Ieee802154EventMask};

/// Transmissions that ended in an abort, by reason.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154TxAbortStatistics {
    /// Coexistence broke the ACK reception (`RX_ACK_ABORT_COEX_CNT`).
    pub rx_ack_coex_break: u64,
    /// No ACK arrived in time (`RX_ACK_TIMEOUT_CNT`).
    pub rx_ack_timeout: u64,
    /// Coexistence broke the transmission, counted by the driver: the
    /// vendor does not read `TX_BREAK_COEX_CNT` (ZB-105).
    pub tx_coex_break: u64,
    /// Transmit security failed (`TX_SECURITY_ERROR_CNT`).
    pub tx_security_error: u64,
    /// The CCA failed (`CCA_FAIL_CNT`).
    pub cca_failed: u64,
    /// The CCA found the channel busy (`CCA_BUSY_CNT`).
    pub cca_busy: u64,
}

/// Transmit statistics (`ieee802154_txrx_statistic_t.tx`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154TxStatistics {
    /// Transmissions started (`tx_init`).
    pub nums: u64,
    /// Transmissions refused at once because a reception or ACK was in
    /// progress.
    pub deferred_nums: u64,
    /// `TX_DONE` interrupts.
    pub done_nums: u64,
    /// Aborts by reason.
    pub abort: Ieee802154TxAbortStatistics,
}

/// Receptions that ended in an abort, by reason.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154RxAbortStatistics {
    /// No SFD in time (`SFD_TIMEOUT_CNT`).
    pub sfd_timeout: u64,
    /// The FCS was wrong (`CRC_ERROR_CNT`).
    pub crc_error: u64,
    /// The address filter dropped the frame (`RX_FILTER_FAIL_CNT`).
    pub filter_fail: u64,
    /// No signal was detected (`NO_RSS_DETECT_CNT`).
    pub no_rss: u64,
    /// Coexistence broke the reception (`RX_ABORT_COEX_CNT`).
    pub rx_coex_break: u64,
    /// The receiver restarted (`RX_RESTART_CNT`).
    pub rx_restart: u64,
    /// Coexistence broke the ACK transmission (`TX_ACK_ABORT_COEX_CNT`).
    pub tx_ack_coex_break: u64,
    /// An energy detection was aborted (`ED_ABORT_CNT`).
    pub ed_abort: u64,
}

/// Receive statistics (`ieee802154_txrx_statistic_t.rx`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154RxStatistics {
    /// `RX_DONE` interrupts.
    pub done_nums: u64,
    /// Aborts by reason.
    pub abort: Ieee802154RxAbortStatistics,
}

/// The vendor's TX/RX statistics (`ieee802154_txrx_statistic_t`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154TxRxStatistics {
    /// Transmit statistics.
    pub tx: Ieee802154TxStatistics,
    /// Receive statistics.
    pub rx: Ieee802154RxStatistics,
}

impl Ieee802154TxRxStatistics {
    /// `ieee802154_txrx_statistic`: count a lone `TX_DONE` or `RX_DONE`
    /// interrupt - the vendor compares the whole event image - and drain
    /// the MAC's diagnostic counters into the totals.
    pub(crate) fn record<L: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut L,
        events: Ieee802154EventMask,
    ) {
        if events == Ieee802154Event::TxDone.mask() {
            self.tx.done_nums += 1;
        } else if events == Ieee802154Event::RxDone.mask() {
            self.rx.done_nums += 1;
        }
        let mut drain = |counter| {
            let count = u64::from(ll.debug_counter(counter));
            ll.clear_debug_counter(counter);
            count
        };
        use Ieee802154DebugCounter as Counter;
        self.tx.abort.cca_busy += drain(Counter::CcaBusy);
        self.tx.abort.tx_security_error += drain(Counter::TxSecurityError);
        self.tx.abort.rx_ack_timeout += drain(Counter::RxAckTimeout);
        self.tx.abort.rx_ack_coex_break += drain(Counter::RxAckAbortCoex);
        self.tx.abort.cca_failed += drain(Counter::CcaFail);
        self.rx.abort.tx_ack_coex_break += drain(Counter::TxAckAbortCoex);
        self.rx.abort.rx_restart += drain(Counter::RxRestart);
        self.rx.abort.rx_coex_break += drain(Counter::RxAbortCoex);
        self.rx.abort.no_rss += drain(Counter::NoRssDetect);
        self.rx.abort.filter_fail += drain(Counter::RxFilterFail);
        self.rx.abort.ed_abort += drain(Counter::EdAbort);
        self.rx.abort.crc_error += drain(Counter::CrcError);
        self.rx.abort.sfd_timeout += drain(Counter::SfdTimeout);
    }
}
