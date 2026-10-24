use crate::{evidence::*, keys};
use oer_hil_lab::{
    air::{
        AirCapture,
        station::{Direction, StationFrame},
    },
    config::StationFixtureConfig,
};
use oer_hil_link::{CommandHandle, SerialCapture};
use oer_hil_protocol::{
    network::{Ready, WifiNetworkInterface},
    wifi::*,
};
use oer_hil_scenario::{
    Result,
    workload::{context::Context, require_keys},
};
use oer_lab_router::{
    credentials,
    mac::Mac,
    router,
    ssh::Ssh,
    w3::{
        self,
        stream::{Capture, Departure},
    },
    w4,
};
use std::{
    net::Ipv4Addr,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("host epoch")
        .as_micros()
        .try_into()
        .expect("host epoch fits u64")
}
const SHORT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, serde::Serialize)]
struct Link {
    at_us: u64,
    body: StationLink,
}
struct Connected {
    epoch: u32,
    at_us: u64,
    sequence: u32,
    router_authorized: bool,
    associated_us: u64,
}
fn authorized(capture: &SerialCapture, start: CommandHandle, epoch: u32) -> Result<Connected> {
    let (message,link)=capture.wait_for_after::<StationLink>(start.first_event(),Duration::from_secs(45),|message,link|
        start.correlates(message) && link.epoch>epoch && link.state==StationLinkState::Authorized)?
        .ok_or("station did not report Connected/Authorized; inspect the handshake trace before continuing")?;
    let associated = capture
        .messages_since(start.first_event(), |messages| {
            messages
                .iter()
                .rev()
                .find(|candidate| {
                    start.correlates(candidate)
                        && candidate.message_sequence < message.message_sequence
                        && candidate.decode::<StationLink>().is_some_and(|event| {
                            event.epoch == link.epoch && event.state == StationLinkState::Associated
                        })
                })
                .cloned()
        })?
        .ok_or("Authorized event has no association in its station epoch")?;
    Ok(Connected {
        epoch: link.epoch,
        at_us: capture
            .received_at_micros(&message)
            .ok_or("link event has no host timestamp")?,
        sequence: message.message_sequence,
        router_authorized: false,
        associated_us: capture
            .received_at_micros(&associated)
            .ok_or("association has no host timestamp")?,
    })
}
fn network(
    capture: &SerialCapture,
    start: CommandHandle,
    connected: &Connected,
) -> Result<oer_hil_protocol::network::NetworkInfo> {
    let (_, Ready(info)) = capture
        .wait_for_after::<Ready>(
            start.first_event(),
            Duration::from_secs(35),
            |message, Ready(info)| {
                start.correlates(message)
                    && message.message_sequence > connected.sequence
                    && info.network_interface == WifiNetworkInterface::Station
            },
        )?
        .ok_or("authorized station has no DHCP observation")?;
    Ok(info)
}

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let StationFixtureConfig::OpenWrt(config) = &context.lab.station_fixture else {
        return Err("W4 requires managed OpenWrt".into());
    };
    let (key_run, keys) = keys::load(context.lab)?;
    if !keys.complete() {
        return Err(
            "independent key conformance evidence is incomplete or differs from the model".into(),
        );
    }
    context.results.observe("w4-key-conformance-run", &key_run);
    let mut evidence = Evidence {
        keys: Some(keys),
        ..Default::default()
    };
    let directory = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router = oer_lab_router::lock::router(&directory, &config.ssh_target)?;
    let mut ssh = Ssh::new(&config.ssh_target)?;
    let ssid = credentials::ssid("oer-w4")?;
    let passphrase = credentials::passphrase()?;
    let profile = router::Profile::w4(
        &config.radio,
        &config.ap_section,
        &config.wireless_interface,
        config.channel,
        ssid.expose(),
        passphrase.expose(),
    )?;
    let applied = router::apply(&mut ssh, &profile)?;
    w4::verify_runtime(&mut ssh, &config.radio, &config.wireless_interface)?;
    drop(profile);
    let target = Mac::parse(&context.lab.dut.mac).ok_or("invalid W4 station identity")?;
    let before = w3::station(&mut ssh, &config.wireless_interface, target)?;
    if before.present {
        return Err("W4 station was already associated before StartStation".into());
    }
    let air = AirCapture::start_in_run(applied.channel)?;
    let mut links = Vec::new();
    let mut connected = Vec::new();
    let mut stop_at = 0;
    let result = context.with_capture(output, |capture| {
        require_keys::<StartStation>(capture)?;
        require_keys::<ProbeStation>(capture)?;
        require_keys::<StopStation>(capture)?;
        let credentials =
            NetworkCredentials::try_new(ssid.expose().as_bytes(), passphrase.expose().as_bytes())?;
        let start = capture.command(
            StartStation(StationStart {
                credentials,
                power_save: StationPowerSave::Off,
            }),
            SHORT,
        )?;
        let work: Result<_> = (|| {
            connected.push(authorized(capture, start, 0)?);
            let info = network(capture, start, &connected[0])?;
            let address = Ipv4Addr::from(info.address);
            let gateway = Ipv4Addr::from(
                info.gateway
                    .ok_or("station DHCP observation has no gateway")?,
            );
            let joined = w3::station(&mut ssh, &config.wireless_interface, target)?;
            let lease = w3::lease(&mut ssh, target)?
                .ok_or("router has no unique live station DHCP lease")?;
            connected[0].router_authorized = joined.joined();
            evidence.baseline = Some(Baseline {
                channel: applied.channel,
                wpa2_ccmp: true,
                ht20: true,
                he_disabled: true,
                pmf_disabled: true,
                before,
                joined,
                lease,
                target_address: address,
                target_gateway: gateway,
                target_prefix: info.prefix_length,
                tx_timeouts: 0,
                queue_collisions: 0,
                deauth_transmissions: 0,
                final_departure_us: 0,
                after: w3::Station::default(),
            });
            let initial_key = capture.request(0, GetStationCounters, SHORT)?;
            let (initial_traffic, rekey) = traffic(
                capture,
                context,
                &mut ssh,
                100,
                address,
                gateway,
                connected[0].epoch,
                start,
                Some(initial_key),
            )?;
            evidence.traffic = Some(initial_traffic);
            evidence.rekey = rekey;
            let departure = Departure::start(&ssh, target)?;
            let cursor = capture.event_cursor();
            let forced = now();
            w4::deauthenticate(&mut ssh, &config.wireless_interface, target)?;
            capture
                .wait_for_after::<StationLink>(cursor, SHORT, |message, link| {
                    start.correlates(message)
                        && link.epoch == connected[0].epoch
                        && matches!(
                            link.state,
                            StationLinkState::Disconnected {
                                reason: StationDisconnectReason::Peer { .. }
                            }
                        )
                })?
                .ok_or("station did not observe the router deauthentication")?;
            connected.push(authorized(capture, start, connected[0].epoch)?);
            let _ = network(capture, start, &connected[1])?;
            if !departure.finish()? {
                return Err("router has no unique fresh forced-departure event".into());
            }
            if !w3::station(&mut ssh, &config.wireless_interface, target)?.joined() {
                return Err("router did not authorize the reconnected station".into());
            }
            connected[1].router_authorized = true;
            evidence.forced_deauth_us = Some(forced);
            let (resumed, _) = traffic(
                capture,
                context,
                &mut ssh,
                10,
                address,
                gateway,
                connected[1].epoch,
                start,
                None,
            )?;
            evidence.resumed = Some(resumed);
            let departure = Departure::start(&ssh, target)?;
            stop_at = now();
            let stop = capture.command(StopStation, SHORT)?;
            let stopped: StationStopped = capture.wait_command(stop, SHORT)?;
            context.results.observe("w4-final-counters", &stopped.0);
            let deadline = Instant::now() + SHORT;
            let after = loop {
                let after = w3::station(&mut ssh, &config.wireless_interface, target)?;
                if !after.present || Instant::now() >= deadline {
                    break after;
                }
                oer_os_process::sleep(Duration::from_millis(100))?;
            };
            if !departure.finish()? {
                return Err("router has no unique fresh final-departure event".into());
            }
            let b = evidence
                .baseline
                .as_mut()
                .expect("initial observations retained");
            b.tx_timeouts = stopped.0.tx_timeouts;
            b.queue_collisions = stopped.0.queue_collisions;
            b.deauth_transmissions = stopped.0.deauth_transmissions;
            b.after = after;
            Ok(())
        })();
        let events = capture.messages_since(start.first_event(), |messages| {
            messages
                .iter()
                .filter(|message| start.correlates(message))
                .filter_map(|message| {
                    message
                        .decode::<StationLink>()
                        .map(|body| (message.clone(), body))
                })
                .collect::<Vec<_>>()
        })?;
        for (message, body) in events {
            links.push(Link {
                at_us: capture
                    .received_at_micros(&message)
                    .ok_or("link event has no host timestamp")?,
                body,
            });
        }
        work
    });
    let projected = air
        .finish_station(target.octets(), applied.bssid.octets())
        .and_then(|(frames, drops)| {
            context.results.observe("w4-station-air", &frames);
            project(&mut evidence, &frames, drops, &connected, stop_at)?;
            Ok(())
        });
    context.results.observe("w4-link-events", &links);
    context.results.observe("w4-evidence", &evidence);
    let gates = evaluate(&evidence);
    context.results.observe("w4-gates", &gates);
    for gate in &gates {
        eprintln!(
            "w4 gate {}: {}{}",
            gate.id,
            if gate.passed() { "PASS" } else { "FAIL" },
            gate.reason
                .map(|reason| format!(": {reason}"))
                .unwrap_or_default()
        );
    }
    result?;
    projected?;
    if let Some(gate) = gates.iter().find(|gate| !gate.passed()) {
        return Err(format!(
            "W4 gate {}: {}",
            gate.id,
            gate.reason.expect("failed gate has a reason")
        )
        .into());
    }
    context.results.claim("w4", &["PTK rekey", "WPA3-SAE"]);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn traffic(
    capture: &SerialCapture,
    context: &Context<'_>,
    ssh: &mut Ssh,
    count: u16,
    address: Ipv4Addr,
    router: Ipv4Addr,
    epoch: u32,
    station: CommandHandle,
    initial: Option<StationCounters>,
) -> Result<(Traffic, Option<Rekey>)> {
    let StationFixtureConfig::OpenWrt(config) = &context.lab.station_fixture else {
        return Err("W4 requires OpenWrt".into());
    };
    let observed = Capture::start(ssh, &config.wireless_interface, address, count)?;
    let probe = capture.command(
        ProbeStation {
            count,
            identifier: if count == 100 { 0x4331 } else { 0x4332 },
        },
        SHORT,
    )?;
    let ready: StationProbeReady = capture.wait_command(probe, SHORT)?;
    if ready.count != count
        || ready.epoch != epoch
        || ready.identifier != if count == 100 { 0x4331 } else { 0x4332 }
    {
        return Err("probe readiness belongs to another traffic window".into());
    }
    let ping_ssh = Ssh::new(&config.ssh_target)?;
    let (finished, ping, rekey) = std::thread::scope(|scope| -> Result<_> {
        let ping = scope.spawn(|| w3::ping(&ping_ssh, address, count));
        let work: Result<_> = (|| {
            let rekey = if let Some(initial) = initial {
                let (message, key) = capture
                    .wait_for_after::<StationGroupKey>(
                        probe.first_event(),
                        Duration::from_secs(45),
                        |message, key| {
                            station.correlates(message)
                                && key.epoch == epoch
                                && key.replaced
                                && key.generation > initial.group_generation
                        },
                    )?
                    .ok_or("no observed GTK replacement during the traffic window")?;
                let completed_us = capture
                    .received_at_micros(&message)
                    .ok_or("GTK observation has no timestamp")?;
                let before = capture.request(0, GetStationCounters, SHORT)?;
                w4::broadcast_arp(ssh, w3::network::BRIDGE, address)?;
                let after = capture.request(0, GetStationCounters, SHORT)?;
                if before.group_generation != key.generation
                    || after.group_generation != key.generation
                {
                    return Err("GTK epoch changed during the ARP observation".into());
                }
                Some(Rekey {
                    completed_us,
                    old_key_id: initial.group_key_id,
                    new_key_id: key.key_id,
                    arp_received_us: now(),
                    arp_frames: after
                        .group_arp_received
                        .saturating_sub(before.group_arp_received),
                })
            } else {
                None
            };
            let finished: StationProbeFinished =
                capture.wait_command(probe, Duration::from_secs(u64::from(count) + 20))?;
            context
                .results
                .observe(format!("w4-probe-{count}"), &finished);
            if finished.epoch != epoch || finished.identifier != ready.identifier {
                return Err("probe result belongs to another traffic window".into());
            }
            Ok((finished, rekey))
        })();
        let ping = ping.join().map_err(|_| "router ping worker panicked")??;
        let (finished, rekey) = work?;
        Ok((finished, ping, rekey))
    })?;
    let (echoes, complete) = observed.finish()?;
    if !complete {
        return Err("router ICMP capture is incomplete".into());
    }
    Ok((
        Traffic {
            count,
            address,
            router,
            target_sent: finished.sent,
            target_received: finished.received,
            target_peer_requests: finished.peer_requests,
            target_peer_replies: finished.peer_replies,
            router_sent: ping.sent.try_into()?,
            router_received: ping.received.try_into()?,
            echoes,
        },
        rekey,
    ))
}

fn project(
    e: &mut Evidence,
    frames: &[StationFrame],
    drops: u32,
    connected: &[Connected],
    stop_at: u64,
) -> Result<()> {
    let mut handshakes = Vec::new();
    let mut current = Handshake::default();
    let mut replay = [None, None];
    for frame in frames {
        if frame.subtype == 1 && frame.direction == Direction::ToStation && frame.status == Some(0)
        {
            if current.association_us > 0 {
                handshakes.push(current.clone());
            }
            current = Handshake {
                association_us: frame.at_us,
                ..Default::default()
            };
            replay = [None, None];
        }
        match (frame.handshake_message, frame.direction) {
            (Some(1), Direction::ToStation) if current.association_us > 0 && current.m1_us == 0 => {
                current.m1_us = frame.at_us;
                replay[0] = frame.replay_counter;
            }
            (Some(2), Direction::FromStation)
                if current.m1_us > 0
                    && current.m2_us == 0
                    && replay[0].is_some()
                    && replay[0] == frame.replay_counter =>
            {
                current.m2_us = frame.at_us
            }
            (Some(3), Direction::ToStation)
                if current.m2_us > 0
                    && current.m3_us == 0
                    && frame
                        .replay_counter
                        .zip(replay[0])
                        .is_some_and(|(new, old)| new > old) =>
            {
                current.m3_us = frame.at_us;
                replay[1] = frame.replay_counter;
            }
            (Some(4), Direction::FromStation)
                if current.m3_us > 0
                    && current.m4_us == 0
                    && replay[1].is_some()
                    && replay[1] == frame.replay_counter =>
            {
                current.m4_us = frame.at_us;
            }
            _ => {}
        }
    }
    if current.association_us > 0 {
        handshakes.push(current);
    }
    let correlated = connected
        .iter()
        .filter_map(|link| {
            let mut wire = handshakes
                .iter()
                .rev()
                .find(|wire| wire.association_us <= link.associated_us)?
                .clone();
            wire.epoch = link.epoch;
            wire.connected_us = link.at_us;
            wire.router_authorized = link.router_authorized;
            Some(wire)
        })
        .collect::<Vec<_>>();
    e.handshake = correlated.first().cloned();
    e.reconnect = correlated.get(1).cloned();
    let departure = frames
        .iter()
        .find(|frame| {
            frame.at_us >= stop_at
                && stop_at > 0
                && frame.subtype == 12
                && frame.direction == Direction::FromStation
        })
        .map(|frame| frame.at_us);
    if let Some(b) = &mut e.baseline {
        b.final_departure_us = departure.unwrap_or(0);
    }
    let authorized = e.handshake.as_ref().map(|h| h.m4_us).unwrap_or(u64::MAX);
    let data = frames
        .iter()
        .filter(|frame| {
            frame.at_us >= authorized
                && (0x20..=0x2f).contains(&frame.subtype)
                && frame.direction != Direction::GroupFromAp
                && !frame.eapol
        })
        .collect::<Vec<_>>();
    e.air = Some(Air {
        protected_station_frames: data
            .iter()
            .filter(|frame| frame.protected)
            .count()
            .try_into()?,
        plaintext_station_frames: data
            .iter()
            .filter(|frame| !frame.protected)
            .count()
            .try_into()?,
        first_us: frames.first().map_or(0, |f| f.at_us),
        last_us: frames.last().map_or(0, |f| f.at_us),
        captured_frames: frames.len().try_into()?,
        dropped_frames: drops,
    });
    if let Some(rekey) = &mut e.rekey {
        if !frames.iter().any(|frame| {
            frame.at_us >= rekey.completed_us
                && frame.at_us <= rekey.arp_received_us
                && frame.direction == Direction::GroupFromAp
                && frame.protected
                && frame.key_id == Some(rekey.new_key_id)
        }) {
            rekey.arp_frames = 0;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
