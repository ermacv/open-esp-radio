//! Projection of decoded protocol values into the host measurement vocabulary.

use oer_hil_link::Received;
use oer_hil_protocol::{
    base::LinkHealth, network::EvidenceRecord, network::TransportEvidence, system::StackUsage,
};
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit as Unit;
use std::collections::BTreeMap;

pub(super) fn observations(
    prefix: &str,
    messages: &[Received],
    received_bytes: u64,
) -> Vec<Measurement> {
    let mut records = BTreeMap::new();
    add(
        &mut records,
        prefix,
        "capture.received-bytes",
        received_bytes,
        Unit::Bytes,
    );
    add(
        &mut records,
        prefix,
        "capture.events",
        messages.len() as u64,
        Unit::Count,
    );
    for message in messages {
        let request = format!("{prefix}.request-{}", message.request_id);
        let session = format!("{prefix}.session-{}", message.session_id);
        if let Some(value) = message.decode::<LinkHealth>() {
            link(&mut records, &request, value);
        }
        if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::Transport(value))) =
            message.decode()
        {
            transport(&mut records, &session, value)
        } else if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::FlowTransport(
            value,
        ))) = message.decode()
        {
            transport(
                &mut records,
                &format!("{session}.flow-{}", value.flow_id),
                value.as_session_total(),
            );
        } else if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::Stack(value))) =
            message.decode()
        {
            stack(&mut records, &session, value);
        } else if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::Link(value))) =
            message.decode()
        {
            link(&mut records, &session, value);
        }
        // Qualifying raw register images, metadata or a radio feature is
        // outside this numeric projection. Those typed events remain in
        // protocol.jsonl and keep their workload-specific validator.
    }
    records.into_values().collect()
}

fn add(
    records: &mut BTreeMap<String, Measurement>,
    prefix: &str,
    name: &str,
    value: u64,
    unit: Unit,
) {
    let name = format!("{prefix}.{name}");
    // Retained traffic evidence is replayed under a new request ID. The first
    // publication is the observation; replay must not duplicate or replace it.
    records
        .entry(name.clone())
        .or_insert_with(|| Measurement::observed(name, value, unit));
}

fn transport(records: &mut BTreeMap<String, Measurement>, prefix: &str, value: TransportEvidence) {
    for (name, count) in [
        ("rx.units", value.rx_units),
        ("tx.units", value.tx_units),
        ("errors", value.transport_errors.into()),
    ] {
        add(
            records,
            prefix,
            &format!("transport.{name}"),
            count,
            Unit::Count,
        );
    }
    add(
        records,
        prefix,
        "transport.elapsed",
        value.elapsed_micros,
        Unit::Microseconds,
    );
    for (direction, bytes) in [("rx", value.rx_bytes), ("tx", value.tx_bytes)] {
        add(
            records,
            prefix,
            &format!("transport.{direction}.bytes"),
            bytes,
            Unit::Bytes,
        );
        if value.elapsed_micros != 0 {
            let bps = (u128::from(bytes) * 8_000_000 / u128::from(value.elapsed_micros))
                .min(u128::from(u64::MAX)) as u64;
            add(
                records,
                prefix,
                &format!("transport.{direction}.rate"),
                bps,
                Unit::BitsPerSecond,
            );
        }
    }
}

fn stack(records: &mut BTreeMap<String, Measurement>, prefix: &str, value: StackUsage) {
    stack_values(
        records,
        prefix,
        [
            ("cpu0", Some(value.cpu0)),
            ("cpu1", Some(value.cpu1)),
            ("cpu0-irq", value.cpu0_irq),
            ("cpu1-irq", value.cpu1_irq),
        ],
    );
}

fn stack_values<const N: usize>(
    records: &mut BTreeMap<String, Measurement>,
    prefix: &str,
    values: [(&str, Option<oer_hil_protocol::system::StackWatermark>); N],
) {
    for (core, watermark) in values {
        let Some(watermark) = watermark else { continue };
        for (name, bytes) in [
            ("capacity", watermark.capacity_bytes),
            ("free", watermark.free_bytes),
            ("minimum-free", watermark.minimum_free_bytes),
        ] {
            add(
                records,
                prefix,
                &format!("stack.{core}.{name}"),
                bytes.into(),
                Unit::Bytes,
            );
        }
    }
}

fn link(records: &mut BTreeMap<String, Measurement>, prefix: &str, value: LinkHealth) {
    for (name, count) in [
        ("rx.frames", value.rx_frames),
        ("rx.cobs-errors", value.rx_cobs_errors),
        ("rx.checksum-errors", value.rx_checksum_errors),
        ("rx.decode-errors", value.rx_decode_errors),
        ("rx.overflows", value.rx_overflows),
        ("tx.frames", value.tx_frames),
        ("tx.dropped", value.tx_dropped),
        ("text.dropped", value.text_dropped),
        ("text.truncated", value.text_truncated),
    ] {
        add(
            records,
            prefix,
            &format!("link.lifetime.{name}"),
            count.into(),
            Unit::Count,
        );
    }
}
