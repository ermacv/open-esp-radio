//! Explicit selection of the independent lower-MAC run. There is no lookup of
//! a latest PASS or reuse of an image's gate flag.
use crate::evidence::KeyEvidence;
use oer_hil_model::schema::{image::ImageClass, run::RunState};
use oer_hil_protocol::lower_mac::{CaseTrace, Prepared};
use oer_hil_scenario::Result;
use oer_ieee80211_port::conformance::{BarrierCase, Execution, catalog::Case};

pub const INPUT: &str = "OER_W4_KEY_CONFORMANCE_RUN";

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        && id.contains('-')
}
pub fn load(lab: &oer_hil_lab::config::LabConfig) -> Result<(String, KeyEvidence)> {
    let id=std::env::var(INPUT).map_err(|_| "W4 requires OER_W4_KEY_CONFORMANCE_RUN naming its independent s31-a key conformance run")?;
    if !valid_id(&id) {
        return Err("invalid W4 conformance run identity".into());
    }
    let run = oer_hil_runs::store::RunStore::shared()?.open(&id)?;
    run.validate()?;
    if run.manifest().state != RunState::Completed
        || !run
            .manifest()
            .cell
            .device_id
            .eq_ignore_ascii_case(&lab.dut.mac)
    {
        return Err("W4 conformance run is unfinished or belongs to another board".into());
    }
    let scenario = run.scenario_result("lower-mac-conformance")?;
    if scenario.image != ImageClass::DiagnosticLowerMac || scenario.repetitions.len() != 1 {
        return Err("W4 requires one existing lower-MAC conformance repetition".into());
    }
    let messages = run.protocol_messages(
        scenario
            .repetitions
            .first()
            .ok_or("missing conformance repetition")?,
    )?;
    let mut prepared = None;
    let mut hardware = Vec::new();
    let mut channel = None;
    for message in &messages {
        if message.path == "lower-mac/prepared" {
            let value: Prepared = serde_json::from_value(message.body.clone())?;
            if prepared.replace((message.boot_id, value)).is_some() {
                return Err("repeated preparation in key conformance evidence".into());
            }
        } else if message.path == "lower-mac/case/trace" {
            let trace: CaseTrace = serde_json::from_value(message.body.clone())?;
            if trace.case.case().is_none() {
                return Err("unknown key conformance case identity".into());
            }
            let Some((boot, _)) = prepared else {
                return Err("key trace precedes preparation".into());
            };
            if message.boot_id != boot {
                return Err("key traces cross a boot boundary".into());
            }
            if trace.case.case().is_some_and(|case| {
                matches!(case, Case::Group(_) | Case::Barrier(BarrierCase::RemoveKey))
                    && case.execution() == Execution::Hardware
            }) {
                hardware.push(trace);
            }
        }
    }
    // The immutable scenario plan fixes the laboratory channel in its typed
    // provenance; the current board's run must have used that same channel.
    if let Some(provenance) = run.lab_provenance()?
        && let oer_hil_runs::format::lab::FixtureObservation::OpenWrt(fixture) = provenance.fixture
    {
        channel = Some(fixture.channel);
    }
    let (_, prepared) = prepared.ok_or("missing key conformance preparation")?;
    let channel = channel.ok_or("missing key conformance channel")?;
    Ok((
        id,
        KeyEvidence {
            channel,
            prepared,
            hardware,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_names_one_run_and_cannot_be_a_path_or_a_credential() {
        assert!(valid_id("1792798821710-0002806d"));
        for value in [
            "",
            "../secret-passphrase",
            "/tmp/proof",
            "password",
            "a/b-c",
        ] {
            assert!(!valid_id(value));
        }
    }
}
