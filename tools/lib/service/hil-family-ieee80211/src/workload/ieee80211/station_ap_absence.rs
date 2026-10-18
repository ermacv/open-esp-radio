//! Controlled prolonged AP absence and bounded retry-exhaustion qualification.

use oer_hil_family_ieee80211_fixture::prepared::Prepared;
use oer_hil_workload::context::Context;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use oer_hil_protocol::{
    wifi::StationAttemptFailureReason, wifi::StationDisconnectReason, wifi::StationFailureStage,
    wifi::StationLifecycleEvent,
};

use crate::Result;
use oer_hil_family_ieee80211_fixture::controlled_ap::ControlledAp;
use oer_hil_link::SerialCapture;
use oer_hil_scenario_catalog::link::PhyExpectation;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const QUALIFIED_ATTEMPTS: u16 = 3;

pub struct Config {
    pub timeout: Duration,
    pub initially_absent: bool,
}

pub fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    fixture: &Prepared,
    _phy: PhyExpectation,
) -> Result<()> {
    let options = options.validate()?;
    fs::create_dir_all(output)?;

    let mut ap = fixture.ap()?;
    let result = context.with_capture(output, |capture| {
        let mut cursor = capture.station_lifecycle_cursor();
        qualify(
            capture,
            context,
            &mut cursor,
            &mut ap,
            options.timeout,
            options.initially_absent,
        )
    });
    drop(ap);
    result?;
    eprintln!("station_ap_absence=PASS");
    eprintln!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    context: &Context<'_>,
    cursor: &mut usize,
    ap: &mut ControlledAp,
    timeout: Duration,
    initially_absent: bool,
) -> Result<()> {
    let absence_started = Instant::now();
    let generation = if initially_absent {
        ap.stop()?;
        let (capabilities, handle) = capture.begin_station_attempt(context.target())?;
        validate_service_admission(
            capture
                .wait_command::<oer_hil_protocol::wifi::RoleTransitioned>(handle, timeout)
                .map(|completed| completed.0)?,
        )?;
        if !capabilities.has::<oer_hil_protocol::wifi::StationLifecycleEvents>() {
            return Err("firmware does not advertise reliable station lifecycle events".into());
        }
        0
    } else {
        let capabilities = capture.prepare_station(context.target(), timeout)?;
        if !capabilities.has::<oer_hil_protocol::wifi::StationLifecycleEvents>() {
            return Err("firmware does not advertise reliable station lifecycle events".into());
        }
        expect_event(
            capture,
            cursor,
            timeout,
            StationLifecycleEvent::Connected {
                generation: 0,
                association_bandwidth_mhz: None,
                security: None,
            },
            "initial connection",
        )?;
        ap.stop()?;
        expect_event(
            capture,
            cursor,
            timeout,
            StationLifecycleEvent::Disconnected {
                generation: 0,
                reason: StationDisconnectReason::BeaconLoss,
            },
            "beacon-loss disconnect",
        )?;
        1
    };

    for attempt in 1..QUALIFIED_ATTEMPTS {
        expect_event(
            capture,
            cursor,
            timeout,
            StationLifecycleEvent::AttemptFailed {
                generation,
                attempt,
                stage: StationFailureStage::CandidateSelection,
                reason: StationAttemptFailureReason::NoCandidate,
            },
            "no-candidate attempt",
        )?;
    }
    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::RetryExhausted {
            generation,
            attempts: QUALIFIED_ATTEMPTS,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        "retry exhaustion",
    )?;
    context.measurements.check(
        if initially_absent {
            "wifi.station.initial-retry-exhausted"
        } else {
            "wifi.station.recovery-retry-exhausted"
        },
        true,
    );
    let responsive = capture
        .request(
            0,
            oer_hil_protocol::network::GetStatus,
            Duration::from_secs(3),
        )
        .map(|status| status.0);
    context
        .measurements
        .check("wifi.station.control-responsive", responsive.is_ok());
    responsive?;
    eprintln!(
        "station_ap_absence_exhausted_ms={}",
        absence_started.elapsed().as_millis()
    );
    Ok(())
}

fn validate_service_admission(
    evidence: oer_hil_protocol::wifi::WifiRoleTransitionEvidence,
) -> Result<()> {
    use oer_hil_protocol::wifi::WifiRole;
    if evidence.previous != WifiRole::Idle || evidence.current != WifiRole::Station {
        return Err(format!(
            "initial absence did not admit the requested station service: {evidence:?}"
        )
        .into());
    }
    Ok(())
}

fn expect_event(
    capture: &SerialCapture,
    cursor: &mut usize,
    timeout: Duration,
    expected: StationLifecycleEvent,
    transition: &str,
) -> Result<()> {
    let actual = capture
        .wait_station_lifecycle_event(cursor, timeout)
        .map_err(|error| format!("station {transition}: {error}"))?;
    validate_event(actual, expected, transition)
}

fn validate_event(
    actual: StationLifecycleEvent,
    expected: StationLifecycleEvent,
    transition: &str,
) -> Result<()> {
    let same_connection = matches!(
        (actual, expected),
        (
            StationLifecycleEvent::Connected { generation: observed, .. },
            StationLifecycleEvent::Connected { generation: required, .. }
        ) if observed == required
    );
    if !same_connection && actual != expected {
        return Err(
            format!("station {transition} reported {actual:?}, expected {expected:?}").into(),
        );
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            initially_absent: false,
        }
    }
}

#[cfg(test)]
mod tests;

impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(30)..=Duration::from_secs(300)).contains(&self.timeout) {
            return Err("AP timeout must be in 30..=300 seconds".into());
        }
        Ok(self)
    }
}
