//! Controlled upstream loss and explicit same-channel STA+AP restart.

use oer_hil_family_ieee80211_fixture::prepared::Prepared;
use oer_hil_workload::context::Context;
use std::{fs, path::Path, time::Duration};

use oer_hil_protocol::{
    wifi::StationDisconnectReason, wifi::StationLifecycleEvent, wifi::WifiNetworkInterface,
    wifi::WifiRole, wifi::WifiStationAccessPointRequest,
};

use crate::{
    Result, workload::ieee80211::control::report_stack,
    workload::ieee80211::control::require_transition, workload::ieee80211::control::stop_station,
    workload::ieee80211::station_access_point::wait_for_endpoints,
};
use oer_hil_family_ieee80211_fixture::{
    controlled_ap::ControlledAp,
    local::client::{ClientNetwork, ClientPhy, ControlledClient},
};
use oer_hil_link::SerialCapture;
use oer_hil_net_traffic::probe_udp_rx_ready_via;
use oer_hil_scenario_catalog::link::PhyExpectation;

const TARGET_RX_PORT: u16 = 4_323;
const BEACON_LOSS_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run(
    timeout: Duration,
    output: &Path,
    context: &Context<'_>,
    fixture: &Prepared,
    _phy: PhyExpectation,
) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut upstream = fixture.ap()?;
    context.with_capture(output, |capture| {
        qualify(capture, timeout, context, &mut upstream, output)
    })?;
    eprintln!("wifi_sta_ap_reconnect=PASS");
    eprintln!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    timeout: Duration,
    context: &Context<'_>,
    upstream: &mut ControlledAp,
    output: &Path,
) -> Result<()> {
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let capabilities = capture.prepare_station(context.target(), timeout)?;
    if !capabilities.has::<oer_hil_protocol::wifi::StationAccessPoint>()
        || !capabilities.has::<oer_hil_protocol::wifi::StationLifecycleEvents>()
    {
        return Err("firmware lacks paired-role or lifecycle evidence".into());
    }
    let initial_generation = expect_connected(capture, &mut lifecycle_cursor, timeout)?;
    let _ = stop_station(capture, timeout)?;
    expect_event(
        capture,
        &mut lifecycle_cursor,
        timeout,
        StationLifecycleEvent::Disconnected {
            generation: initial_generation,
            reason: StationDisconnectReason::LinkPolicy,
        },
        "initial station stop",
    )?;

    let first = start_pair(capture, timeout, context)?;
    let first_generation = expect_connected(capture, &mut lifecycle_cursor, timeout)?;
    let first_client = ControlledClient::connect(
        &ClientNetwork::target_access_point(&context.lab.access_point, ClientPhy::Ht),
        &output.join("linux-client-first"),
    )?;
    require_both_endpoints(capture, timeout)?;

    upstream.stop()?;
    let expected_loss = StationLifecycleEvent::Disconnected {
        generation: first_generation,
        reason: StationDisconnectReason::BeaconLoss,
    };
    let observed_loss = capture.wait_station_lifecycle_event_optional(
        &mut lifecycle_cursor,
        timeout.min(BEACON_LOSS_TIMEOUT),
    )?;
    if observed_loss != Some(expected_loss) {
        // Preserve the paired control state which caused the timeout. An
        // isolation reset would erase whether the beacon monitor was unarmed,
        // expired but unserviced, or refreshed by a wrongly routed frame.
        let stopped = capture
            .wait_command::<oer_hil_protocol::wifi::StationAccessPointStopped>(
                capture.command(
                    oer_hil_protocol::wifi::StopStationAccessPoint,
                    oer_hil_link::PROTOCOL_READY_TIMEOUT,
                )?,
                timeout,
            )
            .map(|completed| completed.0)?;
        first_client.restore()?;
        return Err(format!(
            "paired upstream beacon loss reported {observed_loss:?}, expected {expected_loss:?}; explicit diagnostic teardown: {:?}",
            stopped.transition
        )
        .into());
    }

    // The public paired handle remains the explicit capability that joins the
    // already-triggered rollback. Consuming it must return the complete idle
    // graph; no AP-only survivor or hidden station retry is permitted.
    let first_stopped = capture
        .wait_command::<oer_hil_protocol::wifi::StationAccessPointStopped>(
            capture.command(
                oer_hil_protocol::wifi::StopStationAccessPoint,
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            timeout,
        )
        .map(|completed| completed.0)?;
    require_transition(
        first_stopped.transition,
        WifiRole::StationAccessPoint,
        WifiRole::Idle,
    )?;
    if first_stopped.transition.generation != first.generation {
        return Err("beacon-loss teardown returned a different paired generation".into());
    }
    first_client.restore()?;

    upstream.restart()?;
    let second = start_pair(capture, timeout, context)?;
    if second.generation == first.generation {
        return Err("explicit paired restart reused the previous generation".into());
    }
    let second_generation = expect_connected(capture, &mut lifecycle_cursor, timeout)?;
    if second_generation != first_generation.wrapping_add(1) {
        return Err(format!(
            "paired lifecycle generation did not advance after beacon loss: first={first_generation} second={second_generation}"
        )
        .into());
    }
    let second_client = ControlledClient::connect(
        &ClientNetwork::target_access_point(&context.lab.access_point, ClientPhy::Ht),
        &output.join("linux-client-second"),
    )?;
    require_both_endpoints(capture, timeout)?;

    let second_stopped = capture
        .wait_command::<oer_hil_protocol::wifi::StationAccessPointStopped>(
            capture.command(
                oer_hil_protocol::wifi::StopStationAccessPoint,
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            timeout,
        )
        .map(|completed| completed.0)?;
    require_transition(
        second_stopped.transition,
        WifiRole::StationAccessPoint,
        WifiRole::Idle,
    )?;
    if second_stopped.transition.generation != second.generation {
        return Err("explicit restart teardown returned a different generation".into());
    }
    second_client.restore()?;
    report_stack(capture, timeout, "sta-ap-reconnect-stopped")?;
    Ok(())
}

fn start_pair(
    capture: &SerialCapture,
    timeout: Duration,
    context: &Context<'_>,
) -> Result<oer_hil_protocol::wifi::WifiRoleTransitionEvidence> {
    let transition = capture
        .wait_command::<oer_hil_protocol::wifi::RoleTransitioned>(
            capture.command(
                oer_hil_protocol::wifi::StartStationAccessPoint(WifiStationAccessPointRequest {
                    station_credentials: context.lab.station.protocol_credentials()?,
                    access_point: context.lab.access_point.protocol_request(
                        oer_hil_protocol::wifi::WifiAccessPointSecurity::Wpa2Personal,
                    )?,
                }),
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            timeout,
        )
        .map(|completed| completed.0)?;
    require_transition(transition, WifiRole::Idle, WifiRole::StationAccessPoint)?;
    Ok(transition)
}

fn require_both_endpoints(capture: &SerialCapture, timeout: Duration) -> Result<()> {
    let (station, access_point) = wait_for_endpoints(capture, timeout)?;
    probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::Station,
        station,
        None,
        TARGET_RX_PORT,
        timeout,
    )?;
    probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::AccessPoint,
        access_point,
        None,
        TARGET_RX_PORT,
        timeout,
    )?;
    Ok(())
}

fn expect_connected(capture: &SerialCapture, cursor: &mut usize, timeout: Duration) -> Result<u32> {
    let event = capture.wait_station_lifecycle_event(cursor, timeout)?;
    match event {
        StationLifecycleEvent::Connected { generation, .. } => Ok(generation),
        other => Err(format!("paired start reported {other:?} instead of Connected").into()),
    }
}

fn expect_event(
    capture: &SerialCapture,
    cursor: &mut usize,
    timeout: Duration,
    expected: StationLifecycleEvent,
    edge: &str,
) -> Result<()> {
    let actual = capture.wait_station_lifecycle_event(cursor, timeout)?;
    if actual != expected {
        return Err(format!("{edge} reported {actual:?}, expected {expected:?}").into());
    }
    Ok(())
}
