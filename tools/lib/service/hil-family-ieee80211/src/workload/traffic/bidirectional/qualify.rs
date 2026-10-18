use super::*;

pub(super) fn is_qualified_rx_sample(sample: ThroughputSample) -> bool {
    sample.elapsed_us >= MIN_QUALIFIED_SAMPLE.as_micros() as u64
        && sample.throughput_kbps > 0
        && sample.datagrams >= 16
}

fn qualified_rx_samples(report: &DeviceReport) -> Vec<ThroughputSample> {
    report
        .rx
        .iter()
        .copied()
        .filter(|sample| is_qualified_rx_sample(*sample))
        .collect()
}

fn qualify_runtime_marker(report: &DeviceReport) -> Result<()> {
    if report.code_addresses.is_empty()
        || report
            .code_addresses
            .iter()
            .any(|address| !(PSRAM_CODE_START..PSRAM_CODE_END).contains(address))
    {
        return Err("missing psram-code runtime marker".into());
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn qualify_tx_samples(
    report: &DeviceReport,
    required_width: u16,
    minimum_rate: u64,
) -> Result<u64> {
    if report.tx.is_empty()
        || report
            .tx
            .iter()
            .any(|sample| sample.bandwidth_mhz != required_width || sample.rate_kbps < minimum_rate)
    {
        return Err(format!(
            "TX did not remain at {required_width} MHz / at least {minimum_rate} kbit/s"
        )
        .into());
    }
    Ok(report
        .tx
        .iter()
        .map(|sample| sample.throughput_kbps)
        .min()
        .expect("nonempty TX samples"))
}

#[cfg(test)]
fn qualify_ampdu(report: &DeviceReport) -> Result<AmpduEvidence> {
    let ampdu = AmpduEvidence::from_report(report);
    if ampdu.aggregates < MIN_QUALIFIED_AGGREGATES || ampdu.completed == 0 {
        return Err(format!(
            "insufficient A-MPDU evidence: prepared={} completed={} required={MIN_QUALIFIED_AGGREGATES}",
            ampdu.aggregates, ampdu.completed,
        )
        .into());
    }
    if ampdu.maximum > 32 || ampdu.minimum == 0 {
        return Err(format!(
            "invalid A-MPDU size range: min={} max={}",
            ampdu.minimum, ampdu.maximum,
        )
        .into());
    }
    if ampdu.subframes <= ampdu.aggregates {
        return Err("TX never formed a multi-MPDU aggregate".into());
    }
    if ampdu.histogram_total() != ampdu.aggregates {
        return Err(format!(
            "incomplete A-MPDU histogram: buckets={} prepared={}",
            ampdu.histogram_total(),
            ampdu.aggregates,
        )
        .into());
    }
    let build_stop_total = ampdu
        .stop_frame
        .saturating_add(ampdu.stop_capacity)
        .saturating_add(ampdu.stop_empty);
    if build_stop_total != ampdu.aggregates {
        return Err(format!(
            "incomplete A-MPDU build-stop evidence: stops={build_stop_total} prepared={}",
            ampdu.aggregates,
        )
        .into());
    }
    if report.ampdu_timings.is_empty() {
        return Err("missing A-MPDU preparation/publication/exchange timing".into());
    }
    if report.tx_irq_timings.is_empty() {
        return Err("missing TX IRQ-to-service timing".into());
    }
    if report.ampdu_block_acks.is_empty() {
        return Err("missing A-MPDU BlockAck validity evidence".into());
    }
    let classified_block_acks = ampdu
        .full_block_ack
        .saturating_add(ampdu.partial_block_ack)
        .saturating_add(ampdu.empty_block_ack);
    if ampdu.block_ack_samples + ampdu.publications_pending_end
        != ampdu.publications + ampdu.publications_pending_start
        || classified_block_acks != ampdu.block_ack_samples
        // Receipt and bitmap coverage are independent axes. A received
        // BlockAck may contain an empty bitmap; a missing BlockAck is also
        // classified as empty because it acknowledges no subframes.
        || ampdu.block_ack_received > ampdu.block_ack_samples
    {
        return Err(format!(
            "inconsistent BlockAck evidence: samples={} publications={} received={} \
             full={} partial={} empty={}",
            ampdu.block_ack_samples,
            ampdu.publications,
            ampdu.block_ack_received,
            ampdu.full_block_ack,
            ampdu.partial_block_ack,
            ampdu.empty_block_ack,
        )
        .into());
    }
    if ampdu.block_ack_success_without != 0 || ampdu.block_ack_nonzero_control != 0 {
        return Err(format!(
            "invalid BlockAck validity/control evidence: success_without={} nonzero_control={}",
            ampdu.block_ack_success_without, ampdu.block_ack_nonzero_control,
        )
        .into());
    }
    if ampdu.tx_irq_epochs != 0 && ampdu.tx_irq_samples.saturating_add(ampdu.tx_irq_skew) == 0 {
        return Err("TX IRQ timing sampled no service edge".into());
    }
    // Publication-to-IRQ timing is available only in the intrusive MAC-IRQ
    // diagnostic image. The correctness image still owns complete aggregate
    // and BlockAck accounting, so zero samples are valid here.
    if ampdu.timeout != 0 || ampdu.collision != 0 {
        return Err(format!(
            "terminal A-MPDU failure: timeout={} collision={}",
            ampdu.timeout, ampdu.collision,
        )
        .into());
    }
    Ok(ampdu)
}

fn observe_rx_report(report: &DeviceReport, expected_format: u8) -> Result<RxQualification> {
    qualify_runtime_marker(report)?;
    if report.dma_health.is_empty() {
        return Err("missing RX DMA-health interval".into());
    }
    if report.software_health.is_empty() {
        return Err("missing RX software-queue health interval".into());
    }
    let rx = qualified_rx_samples(report);
    if rx.is_empty() {
        return Err("missing complete device-side direct-RX sample".into());
    }
    if report.rx_formats.is_empty()
        || report
            .rx_formats
            .iter()
            .any(|format| *format != expected_format)
    {
        return Err(format!("RX did not remain in baseband format {expected_format}").into());
    }
    if report.rx_service.is_empty()
        || report.rx_dispatch.is_empty()
        || report.rx_frontier.is_empty()
    {
        return Err("missing RX pipeline phase telemetry".into());
    }
    if report.rx_sequences.len() != rx.len() {
        return Err(format!(
            "incomplete UDP sequence evidence: records={} qualified_samples={}",
            report.rx_sequences.len(),
            rx.len(),
        )
        .into());
    }
    if report.rx_reorder.len() != rx.len() {
        return Err(format!(
            "incomplete RX BlockAck reorder evidence: records={} qualified_samples={}",
            report.rx_reorder.len(),
            rx.len(),
        )
        .into());
    }
    if report.rx_s_mpdu.len() != rx.len() {
        return Err(format!(
            "incomplete RX S-MPDU evidence: records={} qualified_samples={}",
            report.rx_s_mpdu.len(),
            rx.len(),
        )
        .into());
    }
    if report.rx_ampdu.len() != rx.len() {
        return Err(format!(
            "incomplete RX A-MPDU evidence: records={} qualified_samples={}",
            report.rx_ampdu.len(),
            rx.len(),
        )
        .into());
    }
    if report.mac_irq.is_empty() {
        return Err("missing MAC interrupt classification telemetry".into());
    }
    let mut pipeline = RxPipelineEvidence::default();
    for sample in report
        .rx_service
        .iter()
        .chain(&report.rx_dispatch)
        .chain(&report.rx_frontier)
    {
        pipeline.merge(*sample);
    }
    let mut irq = MacIrqEvidence::default();
    for sample in &report.mac_irq {
        irq.merge(*sample);
    }
    if pipeline.mac_irq_entries != 0 && irq.classified_entries() != pipeline.mac_irq_entries {
        return Err(format!(
            "incomplete MAC interrupt classification: classified={} hard_entries={}",
            irq.classified_entries(),
            pipeline.mac_irq_entries,
        )
        .into());
    }
    if pipeline.frontier_histogram_total() != pipeline.service_calls {
        return Err(format!(
            "incomplete RX frontier histogram: buckets={} services={}",
            pipeline.frontier_histogram_total(),
            pipeline.service_calls,
        )
        .into());
    }
    let mut he_mcs_histogram = [0_u64; 12];
    let mut other_phy_frames = 0_u64;
    for (histogram, other) in &report.rx_mcs_histograms {
        for (total, sample) in he_mcs_histogram.iter_mut().zip(histogram) {
            *total = total.saturating_add(*sample);
        }
        other_phy_frames = other_phy_frames.saturating_add(*other);
    }
    let mut ht40_long_gi_mcs = [0_u64; 8];
    let mut ht40_short_gi_mcs = [0_u64; 8];
    let mut ht20_mcs = [0_u64; 8];
    let mut ht_other_frames = 0_u64;
    for long_gi in &report.rx_ht40_long_gi_histograms {
        for (total, sample) in ht40_long_gi_mcs.iter_mut().zip(long_gi) {
            *total = total.saturating_add(*sample);
        }
    }
    for short_gi in &report.rx_ht40_short_gi_histograms {
        for (total, sample) in ht40_short_gi_mcs.iter_mut().zip(short_gi) {
            *total = total.saturating_add(*sample);
        }
    }
    for (ht20, other) in &report.rx_ht20_histograms {
        for (total, sample) in ht20_mcs.iter_mut().zip(ht20) {
            *total = total.saturating_add(*sample);
        }
        ht_other_frames = ht_other_frames.saturating_add(*other);
    }
    let mut s_mpdu = RxSmpduEvidence::default();
    for sample in &report.rx_s_mpdu {
        s_mpdu.merge(*sample);
    }
    if s_mpdu.observed_datagrams() == 0 {
        return Err("RX S-MPDU evidence did not observe a benchmark datagram".into());
    }
    if s_mpdu.unavailable_datagrams != 0 {
        return Err(format!(
            "RX S-MPDU provenance unavailable for {} benchmark datagrams",
            s_mpdu.unavailable_datagrams,
        )
        .into());
    }
    if s_mpdu.observed_beacons() == 0 {
        return Err("RX S-MPDU evidence did not observe a connected beacon".into());
    }
    if s_mpdu.unavailable_beacons != 0 {
        return Err(format!(
            "RX S-MPDU provenance unavailable for {} connected beacons",
            s_mpdu.unavailable_beacons,
        )
        .into());
    }
    let mut ampdu = RxAmpduEvidence::default();
    for sample in &report.rx_ampdu {
        ampdu.merge(*sample);
    }
    if ampdu.ampdu_datagrams
        != ampdu
            .hardware_ampdu_datagrams
            .saturating_add(ampdu.protocol_ampdu_datagrams)
        || ampdu.not_ampdu_datagrams
            != ampdu
                .hardware_not_ampdu_datagrams
                .saturating_add(ampdu.protocol_not_ampdu_datagrams)
    {
        return Err("RX A-MPDU totals do not match their provenance classes".into());
    }
    if expected_format == 2 {
        if ampdu.hardware_observed_datagrams() == 0 {
            return Err("HT RX did not carry direct HT-SIG A-MPDU evidence".into());
        }
        if ampdu.unavailable_datagrams != 0 {
            return Err(format!(
                "HT RX A-MPDU provenance unavailable for {} benchmark datagrams",
                ampdu.unavailable_datagrams,
            )
            .into());
        }
        if ampdu.ampdu_datagrams == 0 {
            return Err("HT RX did not observe an aggregated benchmark MPDU".into());
        }
        // Legacy OFDM frames are valid rate-control fallback within an HT
        // association and protocol-decodable as non-A-MPDU. A positive HT
        // aggregation claim still requires the hardware HT-SIG bit.
        if ampdu.protocol_ampdu_datagrams != 0 {
            return Err("HT RX A-MPDU evidence did not remain hardware-sourced".into());
        }
    } else if matches!(expected_format, 4..=7) {
        if ampdu.protocol_ampdu_datagrams == 0 {
            return Err("HE RX did not carry format-validated A-MPDU evidence".into());
        }
        if ampdu.protocol_not_ampdu_datagrams != 0
            || ampdu.hardware_observed_datagrams() != 0
            || ampdu.unavailable_datagrams != 0
        {
            return Err(format!(
                "HE RX A-MPDU provenance was not exclusively format-validated: \
                 protocol_ampdu={} protocol_not_ampdu={} hardware={} unavailable={}",
                ampdu.protocol_ampdu_datagrams,
                ampdu.protocol_not_ampdu_datagrams,
                ampdu.hardware_observed_datagrams(),
                ampdu.unavailable_datagrams,
            )
            .into());
        }
    }
    let mut sequence = UdpSequenceEvidence::default();
    for sample in &report.rx_sequences {
        sequence.merge(*sample);
    }
    let mut order = RxOrderEvidence::default();
    for sample in &report.rx_order {
        order.merge(*sample);
    }
    let mut reorder = RxReorderEvidence::default();
    for sample in &report.rx_reorder {
        reorder.merge(*sample);
    }
    if reorder.window == 0 || reorder.window > 64 || reorder.last_start_tid > 7 {
        return Err(format!(
            "invalid RX BlockAck agreement evidence: tid={} window={}",
            reorder.last_start_tid, reorder.window,
        )
        .into());
    }
    if reorder.occupied > reorder.maximum_occupied || reorder.maximum_occupied >= reorder.window {
        return Err(format!(
            "invalid RX reorder occupancy: current={} maximum={} window={}",
            reorder.occupied, reorder.maximum_occupied, reorder.window,
        )
        .into());
    }
    if reorder.first_samples != 0 {
        let distance = reorder
            .first_frame_sequence
            .wrapping_sub(reorder.first_start_sequence)
            & 0x0fff;
        if reorder.first_tid > 7
            || reorder.first_distance != distance
            // A forward frame outside the initial window is valid: the
            // reorder algorithm advances its window and accounts for the
            // missing prefix. Only the backward half of the 12-bit sequence
            // space is stale. Exact UDP delivery below still rejects any loss
            // inside the measured stream.
            || reorder.first_distance >= 0x0800
        {
            return Err(format!(
                "invalid first RX reorder frame: tid={} start={} sequence={} distance={} window={}",
                reorder.first_tid,
                reorder.first_start_sequence,
                reorder.first_frame_sequence,
                reorder.first_distance,
                reorder.window,
            )
            .into());
        }
    }
    Ok(RxQualification {
        throughput_median_kbps: median(rx.iter().map(|sample| sample.throughput_kbps).collect())
            .expect("nonempty RX samples"),
        sample_count: rx.len(),
        received_datagrams: rx.iter().map(|sample| sample.datagrams).sum(),
        enqueued: report
            .software_health
            .iter()
            .map(|(enqueued, _)| *enqueued)
            .sum(),
        dropped: report
            .software_health
            .iter()
            .map(|(_, dropped)| *dropped)
            .sum(),
        he_mcs_histogram,
        other_phy_frames,
        ht40_long_gi_mcs,
        ht40_short_gi_mcs,
        ht20_mcs,
        ht_other_frames,
        s_mpdu,
        ampdu,
        pipeline,
        irq,
        task_polls: report.task_polls,
        sequence,
        order,
        reorder,
        buffer_full: report.dma_health.iter().map(|(full, _)| *full).sum(),
        fifo_overflow: report
            .dma_health
            .iter()
            .map(|(_, overflow)| *overflow)
            .sum(),
    })
}

fn rx_policy_failure(report: &DeviceReport, rx: &RxQualification) -> Option<String> {
    if rx.buffer_full != 0 || rx.fifo_overflow != 0 {
        return Some(format!(
            "RX DMA starvation: buffer_full={} fifo_overflow={}",
            rx.buffer_full, rx.fifo_overflow,
        ));
    }
    if rx.dropped != 0 {
        return Some(format!("RX software queue dropped {} frames", rx.dropped));
    }
    if let Some(failure) = report.failures.first() {
        return Some(format!("device reported a data-path failure: {failure}"));
    }
    if rx.irq.saturated_entries != 0 {
        return Some(format!(
            "MAC interrupt acknowledgement loop saturated {} times",
            rx.irq.saturated_entries,
        ));
    }
    if rx.irq.unhandled_entries != 0 {
        return Some(format!(
            "MAC interrupt dispatcher observed {} entries with an unhandled cause",
            rx.irq.unhandled_entries,
        ));
    }
    None
}

pub(super) fn assess_rx_report(report: &DeviceReport, expected_format: u8) -> Result<RxAssessment> {
    let rx = observe_rx_report(report, expected_format)?;
    Ok(RxAssessment {
        failure: rx_policy_failure(report, &rx),
        rx,
    })
}

#[cfg(test)]
pub(super) fn qualify_rx_report(
    report: &DeviceReport,
    expected_format: u8,
) -> Result<RxQualification> {
    let assessment = assess_rx_report(report, expected_format)?;
    if let Some(failure) = assessment.failure {
        return Err(failure.into());
    }
    Ok(assessment.rx)
}

pub fn assess_rx_log(log: &str, expected_format: u8) -> Result<RxAssessment> {
    assess_rx_report(&parse_device_report(log), expected_format)
}

#[cfg(test)]
pub(super) fn qualify(
    options: &Config,
    host: HostTransmission,
    report: &DeviceReport,
) -> Result<RxQualification> {
    let rx = qualify_rx_report(report, options.phy.expected_rx_format())?;
    let minimum_bps = options.rate_bps.saturating_mul(9) / 10;
    if host.throughput_bps() < minimum_bps {
        return Err("host failed to offer at least 90% of the requested rate".into());
    }
    if rx.throughput_median_kbps < minimum_bps / 1_000 {
        return Err(format!(
            "device RX {} kbit/s is below the acceptance floor",
            rx.throughput_median_kbps,
        )
        .into());
    }
    validate_exact_rx_delivery(host.datagrams, rx.received_datagrams, rx.sequence, rx.order)?;
    let (required_width, minimum_rate) = options.phy.required_tx();
    qualify_tx_samples(report, required_width, minimum_rate)?;
    qualify_ampdu(report)?;
    Ok(rx)
}

#[cfg(test)]
pub fn validate_exact_rx_delivery(
    host_datagrams: u64,
    received_datagrams: u64,
    sequence: UdpSequenceEvidence,
    order: RxOrderEvidence,
) -> Result<()> {
    let expected_highest = host_datagrams.checked_sub(1);
    if received_datagrams != host_datagrams {
        return Err(format!(
            "device received {received_datagrams}/{host_datagrams} host UDP datagrams"
        )
        .into());
    }
    if sequence.first != Some(0)
        || sequence.highest != expected_highest
        || sequence.next != Some(host_datagrams)
        || sequence.gap_events != 0
        || sequence.forward_missing != 0
        || sequence.backward != 0
        || sequence.adjacent_duplicates != 0
        || sequence.unsequenced != 0
    {
        let localization = rx_order_localization(sequence, order);
        return Err(format!(
            "device RX sequence defects: first={:?} highest={:?} next={:?} gaps={} \
             missing={} backward={} duplicates={} unsequenced={}; localization={localization}",
            sequence.first,
            sequence.highest,
            sequence.next,
            sequence.gap_events,
            sequence.forward_missing,
            sequence.backward,
            sequence.adjacent_duplicates,
            sequence.unsequenced,
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
fn rx_order_localization(sequence: UdpSequenceEvidence, order: RxOrderEvidence) -> &'static str {
    if sequence.backward == 0 {
        return "no recovered late datagram is available for MAC-order correlation";
    }
    if order.intervals == 0 {
        return "RX order telemetry was not enabled";
    }
    if order.backward == 0 {
        return "reordering appeared after the pre-network ConnectedRx observer";
    }
    if order.backward != sequence.backward {
        return "reordering spans more than one observed RX boundary";
    }
    if order.backward_mac_forward == order.backward {
        return "all late UDP datagrams carried forward 802.11 sequence numbers; reordering predates the open driver's per-TID MAC/BlockAck boundary";
    }
    if order.backward_mac_backward != 0 {
        return "at least one late UDP datagram carried a backward 802.11 sequence number; inspect the open driver's per-TID BlockAck reorder path";
    }
    if order.backward_mac_same != 0 {
        return "at least one late UDP datagram shared an MPDU with its predecessor; inspect A-MSDU subframe order";
    }
    "MAC-order evidence is mixed or unavailable"
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some((values.get(middle - 1)? + values.get(middle)?) / 2)
    } else {
        values.get(middle).copied()
    }
}
