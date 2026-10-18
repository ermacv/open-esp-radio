//! Independent RF observations with silence controls in both directions.

pub mod coexistence;
pub mod directed;
pub mod dtm_peer;
mod establish;
pub mod gatt;
mod hci;
pub mod scannable;
pub mod secure_gatt;

use crate::{Result, fixture::bluetooth};
use oer_hil_link::SerialCapture;
use oer_hil_protocol::{
    bluetooth::BluetoothDtmOperation as Operation, bluetooth::BluetoothDtmResult,
};
use oer_hil_workload::context::Context;
use std::{path::Path, time::Duration};

/// How the adapter's receiver ended while both sides listened.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum PcSilence {
    /// LE Test End reported this count.
    Count(u16),
    /// HCI Reset ended the receiver test, which yields no count.
    RxEndedByReset,
}

#[derive(Default, serde::Serialize)]
struct Counts {
    pc_silence: Option<PcSilence>,
    esp_silence: Option<u16>,
    esp_to_pc: Option<u16>,
    pc_to_esp: Option<u16>,
    quiet_end_counts: Vec<u16>,
    quiet_reset_restarts: u16,
}

pub fn run(
    boots: u8,
    minimum_packets: u16,
    quiet_cycles: Option<u16>,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("set [bluetooth] adapter in the stand file")?;
    for boot in 1..=boots {
        let directory = output.join(format!("boot-{boot:03}"));
        context.with_capture(&directory, |capture| {
            let mut counts = Counts::default();
            let result = probe(capture, adapter, &directory, &mut counts, quiet_cycles)
                .and_then(|()| validate(&counts, minimum_packets, quiet_cycles));
            let cleanup = oer_process::cleanup(|| crate::link::dtm(capture, Operation::Reset));
            context.results.observe(format!("boot-{boot:03}"), &serde_json::json!({
                "schema": 2, "quiet_cycles": quiet_cycles, "adapter": adapter.to_string(), "phy": "LE-1M", "channel": 0,
                "payload": "PRBS9", "payload_bytes": 37, "minimum_packets": minimum_packets,
                "counts": counts, "passed": result.is_ok() && cleanup.is_ok(),
                "error": result.as_ref().err().map(ToString::to_string),
                "esp_restored": cleanup.is_ok(), "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
            }));
            match (result, cleanup) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(error), Ok(_)) => Err(error),
                (Ok(()), Err(error)) => Err(error),
                (Err(error), Err(cleanup)) => Err(format!("{error}; ESP cleanup: {cleanup}").into()),
            }
        })?;
    }
    Ok(())
}

fn probe(
    capture: &SerialCapture,
    adapter: bluetooth::model::Adapter,
    output: &Path,
    counts: &mut Counts,
    quiet_cycles: Option<u16>,
) -> Result<()> {
    let caps = capture.request_image_keys(Duration::from_secs(10))?;
    if !caps.has::<oer_hil_protocol::bluetooth::Dtm>() {
        return Err("firmware lacks Bluetooth DTM control".into());
    }
    crate::link::dtm(capture, Operation::Reset)?;
    crate::link::dtm(capture, Operation::Transmit)?;
    let forward = bluetooth::run_in(
        &output.join("esp-to-pc"),
        adapter,
        bluetooth::model::DtmProfile::ReceiveTransmit,
    )?;
    counts.esp_to_pc = forward.rx_packets;
    if end(capture)? != 0 {
        return Err("ESP transmitter returned a receiver count".into());
    }

    // Every cycle proves both terminal paths and a post-Reset restart without
    // a board reset. Retain partial counts so a failed cycle remains visible.
    for _ in 0..quiet_cycles.unwrap_or(1) {
        // An omitted count preserves the original three End controls and
        // one Reset restart, including archived scenario snapshots.
        for _ in 0..if quiet_cycles.is_some() { 1 } else { 3 } {
            crate::link::dtm(capture, Operation::Receive)?;
            oer_process::sleep(Duration::from_millis(300))?;
            counts.quiet_end_counts.push(end(capture)?);
            counts.esp_silence = counts.quiet_end_counts.first().copied();
        }
        crate::link::dtm(capture, Operation::Receive)?;
        oer_process::sleep(Duration::from_millis(300))?;
        crate::link::dtm(capture, Operation::Reset)?;
        crate::link::dtm(capture, Operation::Receive)?;
        oer_process::sleep(Duration::from_millis(300))?;
        counts.quiet_end_counts.push(end(capture)?);
        counts.quiet_reset_restarts += 1;
    }

    // The adapter first receives while the ESP also receives (the PC silence
    // control), then transmits for 100 ms into the still armed ESP receiver.
    crate::link::dtm(capture, Operation::Receive)?;
    let silence = bluetooth::run_in(
        &output.join("pc-silence"),
        adapter,
        bluetooth::model::DtmProfile::ReceiveSilence,
    )?;
    counts.pc_silence = Some(match silence.rx_packets {
        Some(count) => PcSilence::Count(count),
        None => PcSilence::RxEndedByReset,
    });
    let reverse = bluetooth::run_in(
        &output.join("pc-to-esp"),
        adapter,
        bluetooth::model::DtmProfile::Transmit,
    )?;
    counts.pc_to_esp = Some(end(capture)?);
    if !same_peer(&forward, &silence) || !same_peer(&forward, &reverse) {
        return Err("Bluetooth adapter identity changed between directions".into());
    }
    Ok(())
}

fn same_peer(a: &bluetooth::model::Check, b: &bluetooth::model::Check) -> bool {
    a.address.is_some() && a.address == b.address && a.version.is_some() && a.version == b.version
}

fn end(capture: &SerialCapture) -> Result<u16> {
    match crate::link::dtm(capture, Operation::End)?.result {
        BluetoothDtmResult::Complete {
            received_packets: Some(count),
        } => Ok(count),
        _ => Err("DTM Test End lacks a receiver count".into()),
    }
}

fn validate(counts: &Counts, minimum: u16, quiet_cycles: Option<u16>) -> Result<()> {
    let (ends, resets) = quiet_cycles.map_or((4, 1), |cycles| (usize::from(cycles) * 2, cycles));
    if resets == 0
        || counts.quiet_end_counts.len() != ends
        || counts.quiet_end_counts.iter().any(|&count| count != 0)
        || counts.quiet_reset_restarts != resets
    {
        return Err("DTM quiet Test End/Reset restart controls failed or are incomplete".into());
    }
    // An adapter that cannot report a zero-packet count ends its silent
    // receiver with HCI Reset; that typed outcome still proves a quiet start.
    let pc_quiet = matches!(
        counts.pc_silence,
        Some(PcSilence::Count(0) | PcSilence::RxEndedByReset)
    );
    if !pc_quiet || counts.esp_silence != Some(0) {
        return Err("DTM silence controls failed or are incomplete".into());
    }
    if !counts.esp_to_pc.is_some_and(|count| count >= minimum)
        || !counts.pc_to_esp.is_some_and(|count| count >= minimum)
    {
        return Err(format!(
            "DTM RF delivery below {minimum}: ESP→PC={:?}, PC→ESP={:?}",
            counts.esp_to_pc, counts.pc_to_esp
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_replacement_is_not_a_bidirectional_check_of_one_peer() {
        let mut a = bluetooth::model::Check::new(
            bluetooth::model::Adapter(0),
            bluetooth::model::DtmVersion::V2,
            bluetooth::model::DtmProfile::ReceiveTransmit,
        );
        let mut b = bluetooth::model::Check::new(
            bluetooth::model::Adapter(0),
            bluetooth::model::DtmVersion::V2,
            bluetooth::model::DtmProfile::ReceiveTransmit,
        );
        assert!(!same_peer(&a, &b));
        a.address = Some("peer-a".into());
        a.version = Some("firmware-a".into());
        b.address = a.address.clone();
        b.version = a.version.clone();
        assert!(same_peer(&a, &b));
        b.address = Some("peer-b".into());
        assert!(!same_peer(&a, &b));
    }
    #[test]
    fn both_receivers_and_both_silence_controls_are_required() {
        let mut counts = Counts {
            pc_silence: Some(PcSilence::RxEndedByReset),
            esp_silence: Some(0),
            esp_to_pc: Some(87),
            pc_to_esp: Some(42),
            quiet_end_counts: vec![0; 4],
            quiet_reset_restarts: 2,
        };
        assert!(validate(&counts, 10, Some(2)).is_ok());
        counts.pc_to_esp = Some(0);
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.pc_to_esp = Some(42);
        counts.esp_silence = Some(1);
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.esp_silence = None;
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.esp_silence = Some(0);
        counts.pc_silence = Some(PcSilence::Count(3));
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.pc_silence = Some(PcSilence::Count(0));
        assert!(validate(&counts, 10, Some(2)).is_ok());
        counts.pc_silence = None;
        assert!(validate(&counts, 10, Some(2)).is_err());
    }
    #[test]
    fn rf_counts_cannot_hide_missing_quiet_restart_controls() {
        let mut counts = Counts {
            pc_silence: Some(PcSilence::RxEndedByReset),
            esp_silence: Some(0),
            esp_to_pc: Some(87),
            pc_to_esp: Some(42),
            quiet_end_counts: vec![0; 4],
            quiet_reset_restarts: 2,
        };
        assert!(validate(&counts, 10, Some(2)).is_ok());
        counts.quiet_reset_restarts = 1;
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.quiet_reset_restarts = 2;
        counts.quiet_end_counts.pop();
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.quiet_end_counts.push(1);
        assert!(validate(&counts, 10, Some(2)).is_err());
    }

    #[test]
    fn stress_gate_requires_every_requested_cycle() {
        let mut counts = Counts {
            pc_silence: Some(PcSilence::RxEndedByReset),
            esp_silence: Some(0),
            esp_to_pc: Some(87),
            pc_to_esp: Some(42),
            quiet_end_counts: vec![0; 200],
            quiet_reset_restarts: 100,
        };
        assert!(validate(&counts, 10, Some(100)).is_ok());
        counts.quiet_reset_restarts = 99;
        assert!(validate(&counts, 10, Some(100)).is_err());
        counts.quiet_end_counts = vec![0; 4];
        counts.quiet_reset_restarts = 1;
        assert!(validate(&counts, 10, None).is_ok());
        assert!(validate(&counts, 10, Some(100)).is_err());
        assert!(validate(&counts, 10, Some(0)).is_err());
    }
}
