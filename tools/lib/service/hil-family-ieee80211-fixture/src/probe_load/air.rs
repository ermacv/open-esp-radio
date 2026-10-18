//! Pcap-based checks are independent of successful raw-socket send calls.
use super::model;
use crate::Result;
use crate::air::{self, AirFrame, FrameKind, MacAddress};
use serde::Serialize;
use std::{collections::BTreeSet, fs, path::Path};

#[derive(Debug, Default, Serialize)]
struct Evidence {
    fixed_source_requests: usize,
    rotating_source_requests: usize,
    responses: usize,
    fixed_source_responses: usize,
    rotating_source_responses: usize,
    retry_responses: usize,
    maximum_responses_in_one_second: usize,
}
const FIXED_SOURCE: MacAddress = MacAddress([0x02, 0x4f, 0x45, 0x52, 0x00, 0x00]);

fn fixture_mac(mac: MacAddress) -> bool {
    mac.0[..4] == FIXED_SOURCE.0[..4]
}

fn analyze(frames: &[AirFrame], bssid: MacAddress) -> Result<Evidence> {
    let mut evidence = Evidence::default();
    let mut fixed = BTreeSet::new();
    let mut rotating = BTreeSet::new();
    let mut times = Vec::new();
    let mut response_sequences = BTreeSet::new();
    for frame in frames {
        let (Some(source), Some(receiver)) = (frame.transmitter, frame.receiver) else {
            return Err("probe frame lacks an address".into());
        };
        let sequence = frame
            .sequence
            .ok_or("probe frame lacks a sequence number")?;
        if frame.kind == FrameKind::PROBE_REQUEST && fixture_mac(source) {
            let expected = model::request(sequence).ok_or("unexpected probe sequence")?;
            if source != MacAddress(expected.source) {
                return Err("probe source/sequence does not match the workload".into());
            }
            if source == FIXED_SOURCE {
                fixed.insert((source, sequence));
            } else {
                rotating.insert((source, sequence));
            }
        }
        if frame.kind == FrameKind::PROBE_RESPONSE && source == bssid && fixture_mac(receiver) {
            evidence.responses += 1;
            if receiver == FIXED_SOURCE {
                evidence.fixed_source_responses += 1;
            } else {
                evidence.rotating_source_responses += 1;
            }
            match frame.retry {
                Some(true) => evidence.retry_responses += 1,
                Some(false) => {}
                None => return Err("missing response retry flag".into()),
            }
            if !response_sequences.insert((receiver, sequence)) {
                return Err("duplicate probe response sequence".into());
            }
            times.push(frame.time_micros);
        }
    }
    evidence.fixed_source_requests = fixed.len();
    evidence.rotating_source_requests = rotating.len();
    times.sort_unstable();
    let mut start = 0;
    for end in 0..times.len() {
        while times[end] - times[start] >= 1_000_000 {
            start += 1;
        }
        evidence.maximum_responses_in_one_second = evidence
            .maximum_responses_in_one_second
            .max(end - start + 1);
    }
    Ok(evidence)
}
impl Evidence {
    fn validate(&self) -> Result<()> {
        if self.fixed_source_requests < 201 || self.rotating_source_requests < 200 {
            return Err(
                "probe air evidence incomplete: the observer did not see the full source workload"
                    .into(),
            );
        }
        if self.fixed_source_responses == 0 || self.rotating_source_responses == 0 {
            return Err("probe air evidence lacks responses for both source modes".into());
        }
        if self.retry_responses != 0 || self.maximum_responses_in_one_second > 105 {
            return Err("probe responses exceed the retry/admission budget".into());
        }
        Ok(())
    }
}
pub fn verify(output: &Path) -> Result<()> {
    let report: model::Report =
        serde_json::from_slice(&fs::read(output.join("probe-source.json"))?)?;
    report.validate()?;
    let capture: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("management.json"))?)?;
    if capture["kernel_dropped"].as_u64() != Some(0) {
        return Err("probe observer dropped capture frames".into());
    }
    let frames = air::decode(
        &output.join("management.pcap"),
        "wlan.fc.type_subtype == 4 || wlan.fc.type_subtype == 5",
        air::Payload::Omit,
    )?;
    let evidence = analyze(&frames, MacAddress(report.bssid))?;
    fs::write(
        output.join("probe-air.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    evidence.validate()
}
#[cfg(test)]
mod tests;
