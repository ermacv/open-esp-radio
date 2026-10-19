//! Checks of a session's typed evidence: RX and TX radio health, aggregate
//! timing, zero-copy holding and the stack watermarks the target reports.

use oer_hil_link::Result;
use oer_hil_protocol::{
    network::{RxRadioEvidence, RxZeroCopyEvidence, TxAggregateTimingEvidence, TxRadioEvidence},
    system::StackUsage,
};

use crate::SessionEvidence;

impl SessionEvidence {
    pub fn require_rx_radio_health(self, expected_format: u8) -> Result<RxRadioEvidence> {
        let rx = self
            .radio
            .and_then(|evidence| evidence.rx)
            .ok_or("session did not publish typed RX radio evidence")?;
        if rx.phy_format != expected_format {
            return Err(format!(
                "RX did not remain in baseband format {expected_format}: observed {}",
                rx.phy_format
            )
            .into());
        }
        if rx.dma_buffer_full != 0
            || rx.dma_fifo_overflow != 0
            || rx.network_dropped != 0
            || rx.irq_drain_saturated != 0
            || rx.unhandled_irq_entries != 0
        {
            return Err(format!("typed RX radio health failed: {rx:?}").into());
        }
        let s_mpdu_datagrams = rx
            .s_mpdu_datagrams
            .saturating_add(rx.not_s_mpdu_datagrams)
            .saturating_add(rx.s_mpdu_unavailable_datagrams);
        let s_mpdu_beacons = rx
            .s_mpdu_beacons
            .saturating_add(rx.not_s_mpdu_beacons)
            .saturating_add(rx.s_mpdu_unavailable_beacons);
        if s_mpdu_datagrams == 0
            || rx.s_mpdu_unavailable_datagrams != 0
            || s_mpdu_beacons == 0
            || rx.s_mpdu_unavailable_beacons != 0
        {
            return Err(format!("incomplete typed RX S-MPDU provenance: {rx:?}").into());
        }
        if rx.ampdu_datagrams
            != rx
                .hardware_ampdu_datagrams
                .saturating_add(rx.protocol_ampdu_datagrams)
            || rx.not_ampdu_datagrams
                != rx
                    .hardware_not_ampdu_datagrams
                    .saturating_add(rx.protocol_not_ampdu_datagrams)
        {
            return Err(format!("inconsistent typed RX A-MPDU provenance: {rx:?}").into());
        }
        if expected_format == 2 {
            // An HT association may briefly receive legacy OFDM data while
            // the AP rate controller recovers after a radio pause. Legacy
            // containment is protocol-proven false; only a positive A-MPDU
            // claim must remain sourced from the HT-SIG aggregation bit.
            if rx.hardware_ampdu_datagrams == 0
                || rx.ampdu_datagrams == 0
                || rx.protocol_ampdu_datagrams != 0
                || rx.ampdu_unavailable_datagrams != 0
            {
                return Err(format!("invalid typed HT A-MPDU provenance: {rx:?}").into());
            }
        } else if matches!(expected_format, 4..=7)
            && (rx.protocol_ampdu_datagrams == 0
                || rx.protocol_not_ampdu_datagrams != 0
                || rx.hardware_ampdu_datagrams != 0
                || rx.hardware_not_ampdu_datagrams != 0
                || rx.ampdu_unavailable_datagrams != 0)
        {
            return Err(format!("invalid typed HE A-MPDU provenance: {rx:?}").into());
        }
        if rx.reorder_window == 0
            || rx.reorder_window > 64
            || rx.reorder_tid > 7
            || rx.reorder_current_occupied > rx.reorder_maximum_occupied
            || rx.reorder_maximum_occupied >= u32::from(rx.reorder_window)
        {
            return Err(format!("invalid typed RX reorder agreement: {rx:?}").into());
        }
        if rx.reorder_first_samples != 0 {
            let distance = rx
                .reorder_first_sequence
                .wrapping_sub(rx.reorder_first_start)
                & 0x0fff;
            if rx.reorder_first_tid > 7
                || rx.reorder_first_distance != distance
                || rx.reorder_first_distance >= 0x0800
            {
                return Err(format!("invalid typed first RX reorder frame: {rx:?}").into());
            }
        }
        if rx.rx_frontier_histogram_samples != rx.rx_service_calls
            || (rx.mac_irq_entries != 0 && rx.mac_irq_classified_entries != rx.mac_irq_entries)
        {
            return Err(format!("incomplete typed RX accounting: {rx:?}").into());
        }
        Ok(rx)
    }

    pub fn require_rx_radio(
        self,
        expected_format: u8,
        expected_datagrams: u64,
    ) -> Result<RxRadioEvidence> {
        let rx = self.require_rx_radio_health(expected_format)?;
        let expected_datagrams = u32::try_from(expected_datagrams).map_err(
            |_| "RX qualification sent more datagrams than typed evidence can represent",
        )?;
        if rx.sequence_first != Some(0)
            || rx.sequence_highest != expected_datagrams.checked_sub(1)
            || rx.sequence_gap_events != 0
            || rx.sequence_forward_missing != 0
            || rx.sequence_backward != 0
            || rx.sequence_duplicates != 0
            || rx.sequence_unsequenced != 0
        {
            return Err(format!("typed RX sequence evidence is not exact: {rx:?}").into());
        }
        Ok(rx)
    }

    pub fn require_tx_radio(
        self,
        required_width: u16,
        minimum_rate_kbps: u64,
        minimum_aggregates: u32,
    ) -> Result<(TxRadioEvidence, TxAggregateTimingEvidence)> {
        let tx = self
            .radio
            .and_then(|evidence| evidence.tx)
            .ok_or("session did not publish typed TX radio evidence")?;
        if tx.bandwidth_mhz != required_width
            || u64::from(tx.aggregate_rate_kbps) < minimum_rate_kbps
        {
            return Err(format!(
                "TX did not remain at {required_width} MHz / at least {minimum_rate_kbps} kbit/s: {tx:?}"
            )
            .into());
        }
        if tx.aggregates_prepared < minimum_aggregates || tx.aggregates_completed == 0 {
            return Err(format!("insufficient typed A-MPDU evidence: {tx:?}").into());
        }
        if tx.minimum_subframes == 0
            || tx.maximum_subframes > 32
            || tx.subframes_prepared <= tx.aggregates_prepared
        {
            return Err(format!("invalid typed A-MPDU size evidence: {tx:?}").into());
        }
        let histogram_total = tx
            .prepared_histogram
            .iter()
            .copied()
            .fold(0_u32, u32::saturating_add);
        let stop_total = tx
            .stopped_at_frame_limit
            .saturating_add(tx.stopped_at_capacity_limit)
            .saturating_add(tx.stopped_on_empty_queue);
        if histogram_total != tx.aggregates_prepared || stop_total != tx.aggregates_prepared {
            return Err(format!("incomplete typed A-MPDU accounting: {tx:?}").into());
        }
        let classified = tx
            .full_block_ack
            .saturating_add(tx.partial_block_ack)
            .saturating_add(tx.empty_block_ack);
        if u64::from(tx.block_ack_samples) + u64::from(tx.publications_pending_end)
            != u64::from(tx.aggregate_publications) + u64::from(tx.publications_pending_start)
            || classified != tx.block_ack_samples
            // Receipt and bitmap coverage are independent axes. A received
            // BlockAck may contain an empty bitmap; a missing BlockAck is also
            // classified as empty because it acknowledges no subframes.
            || tx.block_ack_received > tx.block_ack_samples
            || tx.success_without_block_ack != 0
            || tx.nonzero_block_ack_control != 0
        {
            return Err(format!("inconsistent typed BlockAck evidence: {tx:?}").into());
        }
        if tx.tx_irq_epochs != 0
            && tx
                .tx_irq_service_samples
                .saturating_add(tx.tx_irq_clock_skew_samples)
                == 0
        {
            return Err("typed TX IRQ evidence contains no service edge".into());
        }
        // Hard-IRQ timing is an intrusive diagnostic overlay, not part of an
        // ordinary image. Validate it when present, but do not make an
        // otherwise complete TX/BlockAck observation depend on that overlay.
        if tx.hardware_timeouts != 0 || tx.collisions != 0 {
            return Err(format!("terminal typed A-MPDU failure: {tx:?}").into());
        }
        let timing = self
            .tx_timing
            .ok_or("session did not publish typed aggregate-TX timing evidence")?;
        validate_tx_timing(tx, timing)?;
        Ok((tx, timing))
    }
}

fn validate_tx_timing(tx: TxRadioEvidence, timing: TxAggregateTimingEvidence) -> Result<()> {
    for (name, samples, total, maximum) in [
        (
            "preparation",
            tx.aggregates_prepared,
            timing.preparation_micros,
            timing.preparation_max_micros,
        ),
        (
            "publication",
            tx.aggregate_publications,
            timing.publication_micros,
            timing.publication_max_micros,
        ),
        (
            "exchange",
            tx.aggregates_completed,
            timing.exchange_micros,
            timing.exchange_max_micros,
        ),
        (
            "first exchange",
            timing.first_exchanges,
            timing.first_exchange_micros,
            timing.first_exchange_max_micros,
        ),
        (
            "retried exchange",
            timing.retried_exchanges,
            timing.retry_exchange_micros,
            timing.retry_exchange_max_micros,
        ),
        (
            "IRQ-to-service",
            timing.tx_irq_service_samples,
            timing.tx_irq_service_micros,
            timing.tx_irq_service_max_micros,
        ),
        (
            "publication-to-IRQ",
            timing.tx_publication_to_irq_samples,
            timing.tx_publication_to_irq_micros,
            timing.tx_publication_to_irq_max_micros,
        ),
    ] {
        if samples != 0 && (total == 0 || maximum == 0 || maximum > total) {
            return Err(format!(
                "inconsistent typed {name} timing: samples={samples} total={total} max={maximum}"
            )
            .into());
        }
        if samples == 0 && (total != 0 || maximum != 0) {
            return Err(format!(
                "typed {name} timing has values without samples: total={total} max={maximum}"
            )
            .into());
        }
    }
    if timing
        .first_exchanges
        .saturating_add(timing.retried_exchanges)
        != tx.aggregates_completed
        || timing.tx_irq_epochs != tx.tx_irq_epochs
        || timing.tx_irq_service_samples != tx.tx_irq_service_samples
        || timing.tx_irq_clock_skew_samples != tx.tx_irq_clock_skew_samples
        || timing.tx_publication_to_irq_samples != tx.tx_publication_to_irq_samples
        || u64::from(timing.standby_prepared) + u64::from(timing.standby_pending_start)
            != u64::from(timing.standby_published)
                + u64::from(timing.standby_cancelled)
                + u64::from(timing.standby_pending_end)
    {
        return Err(format!(
            "typed aggregate-TX timing does not match radio ownership: radio={tx:?} timing={timing:?}"
        )
        .into());
    }
    Ok(())
}

/// Every stack watermark is consistent and keeps its required headroom.
pub fn validate_stack_usage(usage: StackUsage) -> Result<()> {
    if usage.cpu0_irq.is_some() != usage.cpu1_irq.is_some() {
        return Err("two-hart stack evidence omitted one dedicated IRQ stack".into());
    }
    for (name, watermark) in [
        ("cpu0", Some(usage.cpu0)),
        ("cpu1", Some(usage.cpu1)),
        ("cpu0-irq", usage.cpu0_irq),
        ("cpu1-irq", usage.cpu1_irq),
    ] {
        let Some(watermark) = watermark else { continue };
        if watermark.capacity_bytes == 0
            || watermark.free_bytes > watermark.capacity_bytes
            || watermark.used_bytes > watermark.capacity_bytes
            || watermark.free_bytes.checked_add(watermark.used_bytes)
                != Some(watermark.capacity_bytes)
            || watermark.minimum_free_bytes == 0
            || watermark.minimum_free_bytes > watermark.capacity_bytes
        {
            return Err(format!("device reported inconsistent {name} stack watermark").into());
        }
        if !watermark.has_required_headroom() {
            return Err(format!(
                "{name} stack headroom is below policy: free={} capacity={} required={} bytes",
                watermark.free_bytes, watermark.capacity_bytes, watermark.minimum_free_bytes
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// A zero-copy RX session never holds more DMA slots than its credit cap,
/// and its peak covers its final holding.
pub(crate) fn validate_rx_zero_copy(zero_copy: RxZeroCopyEvidence) -> Result<()> {
    if zero_copy.cap == 0
        || zero_copy.held_at_end > zero_copy.peak_held
        || zero_copy.peak_held > zero_copy.cap
    {
        return Err(format!("zero-copy RX exceeded its credit cap: {zero_copy:?}").into());
    }
    Ok(())
}
