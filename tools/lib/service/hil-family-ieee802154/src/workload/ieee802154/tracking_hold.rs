//! The RF hold of a shared-PHY tracking tick with the IEEE 802.15.4 client
//! ().
//!
//! One session receives on the channel, so the domain serves the
//! Bluetooth/IEEE 802.15.4 tracking class. The host times forced ticks of
//! every branch set through `oer_hil_workload::phy_tracking_hold`. The device
//! serves one console request at a time, so a transmission it is asked for
//! cannot overlap a timed tick: the ticks run on the receiving, idle radio.
//! The hold under traffic with this class comes from the joint Wi-Fi and
//! Bluetooth LE arm, whose traffic arrives over the air.

use std::{path::Path, time::Duration, time::Instant};

use oer_hil_link::SerialCapture;
use oer_hil_protocol::ieee802154::Ieee802154SessionConfig;
use oer_hil_workload::phy_tracking_hold::{self as tracking_hold, Load};
use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};
use serde::Serialize;

use super::peer_exchange::{DEVICE_SHORT, expect_session};
use crate::Result;

const START_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    /// Timed ticks of each branch set per boot.
    pub ticks: u16,
}

#[derive(Serialize)]
struct BootReport {
    ticks: usize,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| {
            let samples = session(capture, &config)?;
            tracking_hold::record(&context.results, &samples)?;
            Ok(BootReport {
                ticks: samples.len(),
            })
        },
        |_, _| Ok(()),
    )?;
    println!(
        "ieee802154_tracking_hold=PASS boots={} ticks={}",
        config.boots, config.ticks
    );
    Ok(())
}

fn session(capture: &SerialCapture, config: &Config) -> Result<Vec<tracking_hold::Sample>> {
    require_keys::<oer_hil_protocol::ieee802154::Session>(capture)?;
    require_keys::<oer_hil_protocol::phy::TrackingHoldProbe>(capture)?;
    expect_session(
        "start",
        capture
            .request(
                0,
                oer_hil_protocol::ieee802154::StartSession(Ieee802154SessionConfig {
                    channel: config.channel,
                    pan_id: super::peer_exchange::PAN_ID,
                    short_address: DEVICE_SHORT,
                    extended_address: [0; 8],
                    promiscuous: false,
                    background_maintenance: false,
                    enhanced_ack: false,
                    wifi_coexistence: false,
                }),
                START_TIMEOUT,
            )
            .map(|response| response.0)?,
    )?;
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::ReceiveSession,
            COMMAND_TIMEOUT,
        )
        .map(drop)?;
    tracking_hold::suspend(capture)?;
    let deadline = Instant::now() + tracking_hold::window(config.ticks);
    let samples = tracking_hold::measure(capture, config.ticks, Load::Idle, deadline);
    let resumed = tracking_hold::resume(capture);
    let stopped = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::StopSession,
            COMMAND_TIMEOUT,
        )
        .map(|response| response.0)
        .and_then(|evidence| expect_session("stop", evidence.result));
    let samples = samples?;
    resumed?;
    stopped?;
    Ok(samples)
}
