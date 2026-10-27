//! Interrupt-side MAC operations: `EVENT_STATUS`, sampled and acknowledged,
//! and the RX/TX status and ED result words it observes.
//!
//! SOURCE: esp-idf-7b9cc1ac-ieee802154-common-ll.
//! SOURCE: hw8-20261026-radio-local-05, ieee-timers.json, PR #689:
//! masked timer status is hidden; a saved positive sample clears under mask.

use oer_esp32s31_pac::ieee802154::{Ieee802154MacIrq, event_status};
use oer_espressif_ieee802154_engine::types::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
    Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReasonObservation,
};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154RxStateCode, Ieee802154StateSnapshot, Ieee802154TxStateCode,
};
use oer_mmio::Placement;

use super::{Ieee802154InterruptOwner, Ieee802154InterruptSetupOwner, task::byte};

/// The named events of one `EVENT_STATUS` read, and whether one of the two
/// unnamed event bits was set.
fn observation(status: &event_status::Snapshot) -> Ieee802154EventObservation {
    let named = [
        (status.tx_done(), Ieee802154Event::TxDone),
        (status.rx_done(), Ieee802154Event::RxDone),
        (status.ack_tx_done(), Ieee802154Event::AckTxDone),
        (status.ack_rx_done(), Ieee802154Event::AckRxDone),
        (status.rx_abort(), Ieee802154Event::RxAbort),
        (status.tx_abort(), Ieee802154Event::TxAbort),
        (status.ed_done(), Ieee802154Event::EdDone),
        (status.timer0_overflow(), Ieee802154Event::Timer0Overflow),
        (status.timer1_overflow(), Ieee802154Event::Timer1Overflow),
        (status.clock_count_match(), Ieee802154Event::ClockCountMatch),
        (status.tx_sfd_done(), Ieee802154Event::TxSfdDone),
        (status.rx_sfd_done(), Ieee802154Event::RxSfdDone),
    ];
    let events = named
        .into_iter()
        .filter(|(asserted, _)| *asserted)
        .fold(Ieee802154EventMask::NONE, |events, (_, event)| {
            events.with(event)
        });
    Ieee802154EventObservation::new(events, status.events_7() || status.events_13())
}

/// One affine event sample, retained across changes to the enable mask.
/// Observations are copyable; only this saved sample grants acknowledgement.
#[derive(Debug)]
#[must_use = "retain the sample until its selected events are acknowledged"]
pub struct Ieee802154EventSnapshot {
    status: event_status::Snapshot,
}

impl Ieee802154EventSnapshot {
    /// Classifiable events from the same read that grants acknowledgement.
    pub fn observation(&self) -> Ieee802154EventObservation {
        observation(&self.status)
    }
}

/// The sampled events `mask` selects; the two unnamed events are never
/// acknowledged.
fn retain(snapshot: event_status::Snapshot, mask: Ieee802154EventMask) -> event_status::Snapshot {
    snapshot
        .retain_tx_done(mask.contains(Ieee802154Event::TxDone))
        .retain_rx_done(mask.contains(Ieee802154Event::RxDone))
        .retain_ack_tx_done(mask.contains(Ieee802154Event::AckTxDone))
        .retain_ack_rx_done(mask.contains(Ieee802154Event::AckRxDone))
        .retain_rx_abort(mask.contains(Ieee802154Event::RxAbort))
        .retain_tx_abort(mask.contains(Ieee802154Event::TxAbort))
        .retain_ed_done(mask.contains(Ieee802154Event::EdDone))
        .retain_events_7(false)
        .retain_timer0_overflow(mask.contains(Ieee802154Event::Timer0Overflow))
        .retain_timer1_overflow(mask.contains(Ieee802154Event::Timer1Overflow))
        .retain_clock_count_match(mask.contains(Ieee802154Event::ClockCountMatch))
        .retain_tx_sfd_done(mask.contains(Ieee802154Event::TxSfdDone))
        .retain_rx_sfd_done(mask.contains(Ieee802154Event::RxSfdDone))
        .retain_events_13(false)
}

/// The receive and transmit state fields, each read once.
fn state_snapshot<P: Placement>(mac: &Ieee802154MacIrq<P>) -> Ieee802154StateSnapshot {
    Ieee802154StateSnapshot::new(
        Ieee802154RxStateCode::from_field(byte(mac.read_rx_status().state())),
        Ieee802154TxStateCode::from_field(byte(mac.read_tx_status().state())),
    )
}

impl<P: Placement> Ieee802154InterruptSetupOwner<P> {
    /// Sample the receive and transmit state fields once each, while no
    /// handler holds the interrupt side.
    pub(crate) fn state_snapshot(&self) -> Ieee802154StateSnapshot {
        state_snapshot(&self.mac)
    }
}

impl<P: Placement> Ieee802154InterruptOwner<P> {
    /// `ieee802154_ll_get_events`: one read of the event field, without
    /// acknowledging it.
    pub fn events(&self) -> Ieee802154EventSnapshot {
        Ieee802154EventSnapshot {
            status: self.mac.sample_event_status(),
        }
    }

    /// Acknowledge only the saved sample's events selected by `mask`.
    /// Stop timer producers before sampling and masking. Masked EVENT_STATUS
    /// reads hide retained timer pending and the tested RX_ABORT for
    /// ED_COEX_REJECT (reason 26); they do not expose raw pending state.
    /// No status resample grants this write.
    /// SOURCE: HW8_S31_IEEE_MASKED_STATUS_20261026;
    /// HW26_S31_IEEE_ABORT_20261027 (RX_ABORT reason 26 only).
    /// Other lanes stay latched; recurrence of the same lane is not counted.
    pub fn clear_events(&mut self, snapshot: Ieee802154EventSnapshot, mask: Ieee802154EventMask) {
        let snapshot = retain(snapshot.status, mask);
        self.mac.acknowledge_event_status(snapshot);
    }

    /// `ieee802154_ll_get_rx_abort_reason`.
    pub(crate) fn rx_abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        Ieee802154RxAbortReasonObservation::from_field(byte(
            self.mac.read_rx_status().abort_reason(),
        ))
    }

    /// `ieee802154_ll_get_tx_abort_reason`.
    pub(crate) fn tx_abort_reason(&self) -> Ieee802154TxAbortReasonObservation {
        Ieee802154TxAbortReasonObservation::from_field(byte(
            self.mac.read_tx_status().abort_reason(),
        ))
    }

    /// `ieee802154_ll_get_ed_rss`: the signed energy-detection result.
    pub(crate) fn ed_rss(&self) -> i8 {
        self.mac.read_ed_config().ed_rss_code().cast_signed()
    }

    /// `ieee802154_ll_is_cca_busy`.
    pub(crate) fn cca_busy(&self) -> bool {
        self.mac.read_ed_config().cca_busy()
    }
}
