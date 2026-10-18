//! Delivery continuity is independent of average-throughput and radio gates.
use crate::Result;
use oer_hil_protocol::network::TransportEvidence;

pub(super) fn record_rx_silence(
    recorder: &oer_hil_workload::measurements::Recorder,
    limit_ms: Option<u32>,
    evidence: TransportEvidence,
) {
    use oer_hil_run_bundle_format::run::Comparison;
    use oer_hil_run_bundle_format::run::Measurement;
    use oer_hil_run_bundle_format::run::MeasurementUnit;
    if let Some(observed) = evidence.rx_maximum_silence_micros {
        let measurement = Measurement::observed(
            "udp.rx.maximum-silence",
            observed,
            MeasurementUnit::Microseconds,
        );
        recorder.record([match limit_ms {
            Some(limit) => measurement.evaluated(Comparison::AtMost, u64::from(limit) * 1_000),
            None => measurement,
        }]);
    }
}

pub(super) fn require_rx_silence(limit_ms: Option<u32>, evidence: TransportEvidence) -> Result<()> {
    let Some(limit_ms) = limit_ms else {
        return Ok(());
    };
    let observed = evidence
        .rx_maximum_silence_micros
        .ok_or("missing typed complete-window RX silence evidence")?;
    if observed > evidence.elapsed_micros {
        return Err("RX silence exceeds its measured observation window".into());
    }
    if observed > u64::from(limit_ms) * 1_000 {
        return Err(format!(
            "RX continuity failed: maximum silence {observed} us exceeds {limit_ms} ms"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn throughput_cannot_hide_missing_or_overlong_silence() {
        let mut evidence = TransportEvidence {
            rx_maximum_silence_micros: Some(2_995_274),
            rx_bytes: 58_000_000,
            tx_bytes: 78_000_000,
            rx_units: 48_333,
            tx_units: 65_000,
            rx_late_bytes: 0,
            rx_late_units: 0,
            elapsed_micros: 12_000_000,
            transport_errors: 0,
        };
        assert!(require_rx_silence(Some(250), evidence).is_err());
        evidence.rx_maximum_silence_micros = Some(250_000);
        assert!(require_rx_silence(Some(250), evidence).is_ok());
        evidence.rx_maximum_silence_micros = None;
        assert!(require_rx_silence(Some(250), evidence).is_err());
        assert!(require_rx_silence(None, evidence).is_ok());
        evidence.rx_maximum_silence_micros = Some(12_000_001);
        assert!(require_rx_silence(Some(20_000), evidence).is_err());
    }

    #[test]
    fn recorder_preserves_missing_silence_and_the_absolute_bound() {
        let mut evidence = TransportEvidence {
            rx_maximum_silence_micros: None,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_units: 0,
            tx_units: 0,
            rx_late_bytes: 0,
            rx_late_units: 0,
            elapsed_micros: 100_000,
            transport_errors: 0,
        };
        let recorder = oer_hil_workload::measurements::Recorder::default();
        record_rx_silence(&recorder, Some(50), evidence);
        assert!(recorder.snapshot().is_empty());
        evidence.rx_maximum_silence_micros = Some(50_001);
        record_rx_silence(&recorder, Some(50), evidence);
        let values = recorder.snapshot();
        assert_eq!(values[0].name, "udp.rx.maximum-silence");
        assert_eq!(values[0].threshold.unwrap().value, 50_000);
        assert_eq!(
            values[0].verdict,
            Some(oer_hil_run_bundle_format::run::MeasurementVerdict::Failed)
        );
    }
}
