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
use oer_espressif_ieee802154_mac_values::{
    Ieee802154DebugCounter, Ieee802154Event, Ieee802154EventMask,
};

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

/// The total whose next increment is not representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatisticsCounter {
    /// Started transmissions.
    Transmit,
    /// Immediately deferred transmissions.
    Deferred,
    /// Lone TX_DONE samples.
    TxDone,
    /// Lone RX_DONE samples.
    RxDone,
    /// Software-counted transmit coexistence breaks.
    TxCoexistenceBreak,
    /// A drained hardware diagnostic counter.
    Hardware(Ieee802154DebugCounter),
}

/// Original counter values and the already-drained hardware prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatisticsOverflow {
    /// Counter that refused this increment.
    pub counter: StatisticsCounter,
    /// Previously accumulated total, unchanged on error.
    pub accumulated: u64,
    /// Observed increment, not consumed on error.
    pub increment: u64,
    /// Number of preceding hardware counters drained in vendor order.
    pub drained: u8,
}

pub(super) fn add(
    total: &mut u64,
    increment: u64,
    counter: StatisticsCounter,
    drained: u8,
) -> Result<(), StatisticsOverflow> {
    *total = total.checked_add(increment).ok_or(StatisticsOverflow {
        counter,
        accumulated: *total,
        increment,
        drained,
    })?;
    Ok(())
}

impl Ieee802154TxRxStatistics {
    /// `ieee802154_txrx_statistic`: count a lone `TX_DONE` or `RX_DONE`
    /// interrupt - the vendor compares the whole event image - and drain
    /// the MAC's diagnostic counters into the totals.
    pub(crate) fn record<L: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut L,
        events: Ieee802154EventMask,
    ) -> Result<(), StatisticsOverflow> {
        if events == Ieee802154Event::TxDone.mask() {
            add(&mut self.tx.done_nums, 1, StatisticsCounter::TxDone, 0)?;
        } else if events == Ieee802154Event::RxDone.mask() {
            add(&mut self.rx.done_nums, 1, StatisticsCounter::RxDone, 0)?;
        }
        use Ieee802154DebugCounter as Counter;
        let count = u64::from(ll.debug_counter(Counter::CcaBusy));
        add(
            &mut self.tx.abort.cca_busy,
            count,
            StatisticsCounter::Hardware(Counter::CcaBusy),
            0,
        )?;
        ll.clear_debug_counter(Counter::CcaBusy);
        let count = u64::from(ll.debug_counter(Counter::TxSecurityError));
        add(
            &mut self.tx.abort.tx_security_error,
            count,
            StatisticsCounter::Hardware(Counter::TxSecurityError),
            1,
        )?;
        ll.clear_debug_counter(Counter::TxSecurityError);
        let count = u64::from(ll.debug_counter(Counter::RxAckTimeout));
        add(
            &mut self.tx.abort.rx_ack_timeout,
            count,
            StatisticsCounter::Hardware(Counter::RxAckTimeout),
            2,
        )?;
        ll.clear_debug_counter(Counter::RxAckTimeout);
        let count = u64::from(ll.debug_counter(Counter::RxAckAbortCoex));
        add(
            &mut self.tx.abort.rx_ack_coex_break,
            count,
            StatisticsCounter::Hardware(Counter::RxAckAbortCoex),
            3,
        )?;
        ll.clear_debug_counter(Counter::RxAckAbortCoex);
        let count = u64::from(ll.debug_counter(Counter::CcaFail));
        add(
            &mut self.tx.abort.cca_failed,
            count,
            StatisticsCounter::Hardware(Counter::CcaFail),
            4,
        )?;
        ll.clear_debug_counter(Counter::CcaFail);
        let count = u64::from(ll.debug_counter(Counter::TxAckAbortCoex));
        add(
            &mut self.rx.abort.tx_ack_coex_break,
            count,
            StatisticsCounter::Hardware(Counter::TxAckAbortCoex),
            5,
        )?;
        ll.clear_debug_counter(Counter::TxAckAbortCoex);
        let count = u64::from(ll.debug_counter(Counter::RxRestart));
        add(
            &mut self.rx.abort.rx_restart,
            count,
            StatisticsCounter::Hardware(Counter::RxRestart),
            6,
        )?;
        ll.clear_debug_counter(Counter::RxRestart);
        let count = u64::from(ll.debug_counter(Counter::RxAbortCoex));
        add(
            &mut self.rx.abort.rx_coex_break,
            count,
            StatisticsCounter::Hardware(Counter::RxAbortCoex),
            7,
        )?;
        ll.clear_debug_counter(Counter::RxAbortCoex);
        let count = u64::from(ll.debug_counter(Counter::NoRssDetect));
        add(
            &mut self.rx.abort.no_rss,
            count,
            StatisticsCounter::Hardware(Counter::NoRssDetect),
            8,
        )?;
        ll.clear_debug_counter(Counter::NoRssDetect);
        let count = u64::from(ll.debug_counter(Counter::RxFilterFail));
        add(
            &mut self.rx.abort.filter_fail,
            count,
            StatisticsCounter::Hardware(Counter::RxFilterFail),
            9,
        )?;
        ll.clear_debug_counter(Counter::RxFilterFail);
        let count = u64::from(ll.debug_counter(Counter::EdAbort));
        add(
            &mut self.rx.abort.ed_abort,
            count,
            StatisticsCounter::Hardware(Counter::EdAbort),
            10,
        )?;
        ll.clear_debug_counter(Counter::EdAbort);
        let count = u64::from(ll.debug_counter(Counter::CrcError));
        add(
            &mut self.rx.abort.crc_error,
            count,
            StatisticsCounter::Hardware(Counter::CrcError),
            11,
        )?;
        ll.clear_debug_counter(Counter::CrcError);
        let count = u64::from(ll.debug_counter(Counter::SfdTimeout));
        add(
            &mut self.rx.abort.sfd_timeout,
            count,
            StatisticsCounter::Hardware(Counter::SfdTimeout),
            12,
        )?;
        ll.clear_debug_counter(Counter::SfdTimeout);
        Ok(())
    }
}
