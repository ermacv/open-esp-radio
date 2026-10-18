//! Controlled real-AP disappearance and station recovery qualification.

use crate::link::WifiCapture as _;
use oer_hil_family_ieee80211_fixture::prepared::Prepared;
use oer_hil_workload::context::Context;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use oer_hil_protocol::{wifi::StationDisconnectReason, wifi::StationLifecycleEvent};

use crate::Result;
use oer_hil_family_ieee80211_fixture::controlled_ap::ControlledAp;
use oer_hil_link::SerialCapture;
use oer_hil_scenario_catalog::link::PhyExpectation;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Config {
    pub timeout: Duration,
    pub require_recovery_echo: bool,
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
        qualify(capture, context, &mut cursor, &mut ap, options.timeout)?;
        if options.require_recovery_echo {
            // The existing lease may survive link loss. Observe its current-boot
            // address without issuing Initialize or StartStation again; fresh
            // replies, not cached readiness, prove the recovered data path.
            let address = capture.wait_for_network_ready_after(
                0,
                oer_hil_protocol::wifi::WifiNetworkInterface::Station,
                options.timeout,
            )?;
            let result = oer_hil_net_traffic::icmp::fresh_echo(address);
            context.results.observe(
                "recovery-echo",
                &serde_json::json!({
                    "target": address, "requested_replies": 3,
                    "after": "generation-one-reconnection",
                    "failure": result.as_ref().err().map(ToString::to_string),
                }),
            );
            context
                .measurements
                .check("wifi.station.recovered-ip-exchange", result.is_ok());
            result?;
            capture.require_station_unchanged_since(cursor)?;
        }
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
        responsive.map(|_| ())
    });
    drop(ap);
    result?;
    eprintln!("station_ap_loss=PASS");
    eprintln!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    context: &Context<'_>,
    cursor: &mut usize,
    ap: &mut ControlledAp,
    timeout: Duration,
) -> Result<()> {
    let capabilities = capture.prepare_station(context.target(), timeout)?;
    if !capabilities.has::<oer_hil_protocol::wifi::StationLifecycleEvents>() {
        return Err("firmware does not advertise reliable station lifecycle events".into());
    }

    let initial_started = Instant::now();
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
    eprintln!(
        "station_ap_loss_initial_connected_ms={}",
        initial_started.elapsed().as_millis()
    );

    let loss_started = Instant::now();
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
    eprintln!(
        "station_ap_loss_detected_ms={}",
        loss_started.elapsed().as_millis()
    );

    let recovery_started = Instant::now();
    ap.restart()?;
    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::Connected {
            generation: 1,
            association_bandwidth_mhz: None,
            security: None,
        },
        "generation-one recovery",
    )?;
    context
        .measurements
        .check("wifi.station.ap-loss-reconnected", true);
    eprintln!(
        "station_ap_loss_recovered_ms={}",
        recovery_started.elapsed().as_millis()
    );
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
            require_recovery_echo: false,
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
