//! Fixture prerequisites shared by doctor, execution and fixture-only checks.

use crate::{
    Result,
    scenario::{Families, Scenario},
};
use oer_hil_lab::config::LabConfig;
use oer_hil_run_bundle_format::run::Failure;
use oer_hil_run_bundle_format::run::FailureKind;
use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_workload::family::Registry as _;

/// The laboratory checks a scenario needs before it may run: its
/// preconditions, then every fixture provider's check of its requirements.
pub(crate) fn check(lab: &LabConfig, scenario: &Scenario) -> Result<()> {
    let plan = scenario.plan();
    let resolved = lab.resolve(plan.wifi);
    let lab = &resolved;
    if let Some(failure) = scenario_precondition(lab, scenario) {
        return Err(oer_hil_lab::Error::new(failure.message).into());
    }
    for provider in Families::FIXTURES {
        provider.check(lab, &plan)?;
    }
    Ok(())
}

/// Refuse a peer board whose receipt (the devices layer's record of what
/// its flash holds, never the stand's journal) names another image than
/// `expected`, or an unfinished write. A board without a receipt passes;
/// the consumer's own handshake decides.
fn require_peer_image(
    lab: &LabConfig,
    peer: &oer_hil_lab::config::PeerBoardConfig,
    expected: &str,
    reflash: &str,
) -> crate::Result<()> {
    let id = oer_device_lock::DeviceId::parse(&peer.mac)?;
    let state = oer_devices::image::Store::open()?.state(&id)?;
    match other_image(state.as_ref(), expected) {
        None => Ok(()),
        Some(what) => Err(format!(
            "board {} {what}, not `{expected}`; {reflash}",
            lab.stand().label(&peer.mac)
        )
        .into()),
    }
}

/// What the board holds when `state` is not a write of `expected`.
fn other_image(state: Option<&oer_devices::image::State>, expected: &str) -> Option<String> {
    use oer_devices::image::State;
    match state? {
        State::Written { receipt } | State::Started { receipt } if receipt.image != expected => {
            Some(format!(
                "carries `{}` written by {}",
                receipt.image, receipt.by
            ))
        }
        State::Writing { by, .. } => Some(format!("holds an unfinished write by {by}")),
        State::Written { .. } | State::Started { .. } => None,
    }
}

/// What a scenario needs of the laboratory before it may run: its family's
/// precondition, the peer image it uses on the peer board, and each fixture
/// provider's precondition of its plan. A failure blocks the scenario.
pub(crate) fn scenario_precondition(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    let precondition = |error: Box<dyn std::error::Error + Send + Sync>| {
        Some(Failure::new(FailureKind::Precondition, error.to_string()))
    };
    if let Err(error) = selected.family.precondition(lab) {
        return precondition(error);
    }
    if let Some(image) = selected.family.peer_image()
        && let Err(error) = lab
            .peer()
            .and_then(|peer| require_peer_image(lab, &peer, image.name, image.reflash))
    {
        return precondition(error);
    }
    let plan = selected.plan();
    Families::FIXTURES
        .iter()
        .find_map(|provider| provider.precondition(lab, &plan).err())
        .and_then(precondition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_is_refused_only_when_its_receipt_is_another_image() {
        use oer_devices::image::{Receipt, State};
        let receipt = |image: &str| Receipt {
            chip: String::from("chip-b"),
            image: image.to_owned(),
            segments: Vec::new(),
            digest: String::from("ab"),
            by: String::from("bluetooth"),
            unix_millis: 1,
        };
        assert_eq!(other_image(None, "peer"), None);
        let started = |image: &str| State::Started {
            receipt: receipt(image),
        };
        assert_eq!(other_image(Some(&started("peer")), "peer"), None);
        assert_eq!(
            other_image(Some(&started("ble-peer")), "peer").as_deref(),
            Some("carries `ble-peer` written by bluetooth")
        );
        let writing = State::Writing {
            by: String::from("cargo fw"),
            unix_millis: 1,
        };
        assert!(other_image(Some(&writing), "peer").is_some());
    }
}
