use crate::{evidence::*, keys};
use oer_hil_link::{CommandHandle, SerialCapture};
use oer_hil_scenario::{
    Result,
    workload::{context::Context, require_keys},
};
use oer_hil_session::{
    air::{
        AirCapture, TAIL_BOUND,
        station::{Direction, StationFrame},
    },
    config::StationFixtureConfig,
};
use oer_ieee80211_control::*;
use oer_lab_router::{
    credentials,
    mac::Mac,
    router,
    ssh::{COMMAND_TIMEOUT, CommandLog, Ssh},
    station::{
        self,
        stream::{Capture, Departure, DepartureOutcome},
    },
    station_wpa2,
};
use oer_network_control::{Ready, WifiNetworkInterface};
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

/// A failed workload still owns a running station until StopStation completes.
/// A station whose service already ended needs no stop; its end joins the error.
pub(super) fn stop_on_error<T>(
    work: Result<T>,
    stop: impl FnOnce() -> Result<Option<StationServiceEnded>>,
) -> Result<T> {
    match work {
        Err(error) => match stop() {
            Err(_) => Err(format!("{error}; Wi-Fi station StopStation cleanup failed").into()),
            Ok(Some(end)) => Err(format!("{error}; {}", ended(&end)).into()),
            Ok(None) => Err(error),
        },
        ok => ok,
    }
}

pub(super) fn ended(end: &StationServiceEnded) -> String {
    format!(
        "station service ended in epoch {}: {:?}",
        end.epoch, end.end
    )
}

/// How a stop ended.
#[derive(Debug, PartialEq)]
pub(super) enum StopEnd {
    Stopped(StationCounters),
    /// The service had ended without the stop; nothing will stop it.
    Ended(StationServiceEnded),
}

/// The station's side of a stop, apart from the serial capture.
pub(super) trait StationStop {
    type Pending;
    /// The service end the station published since its start, if any.
    fn ended(&self) -> Result<Option<StationServiceEnded>>;
    /// Send StopStation; a refusal is an error.
    fn stop(&self) -> Result<Self::Pending>;
    /// `StationStopped` of the stop or the service end, whichever comes
    /// first, within a bound.
    fn stopped(&self, pending: Self::Pending) -> Result<StopEnd>;
}

/// Stop the station unless its service already ended. A service that ends
/// before the stop is admitted makes the station refuse it; either way the
/// end returns at once instead of waiting out the stop's bound.
pub(super) fn stop_station(station: &impl StationStop) -> Result<StopEnd> {
    if let Some(end) = station.ended()? {
        return Ok(StopEnd::Ended(end));
    }
    match station.stop() {
        Ok(pending) => station.stopped(pending),
        Err(error) => match station.ended()? {
            Some(end) => Ok(StopEnd::Ended(end)),
            None => Err(error),
        },
    }
}

/// The station started by `start` on `capture`.
pub(super) struct Station<'a> {
    pub capture: &'a SerialCapture,
    pub start: CommandHandle,
}
impl StationStop for Station<'_> {
    type Pending = (usize, CommandHandle);
    fn ended(&self) -> Result<Option<StationServiceEnded>> {
        self.capture
            .messages_since(self.start.first_event(), |messages| {
                messages
                    .iter()
                    .filter(|message| self.start.correlates(message))
                    .find_map(|message| message.decode::<StationServiceEnded>())
            })
    }
    fn stop(&self) -> Result<(usize, CommandHandle)> {
        let cursor = self.capture.event_cursor();
        Ok((cursor, self.capture.command(StopStation, SHORT)?))
    }
    fn stopped(&self, (cursor, stop): (usize, CommandHandle)) -> Result<StopEnd> {
        let message = self
            .capture
            .wait_for_message_after(cursor, SHORT, |message| {
                (stop.correlates(message) && message.is::<StationStopped>())
                    || (self.start.correlates(message) && message.is::<StationServiceEnded>())
            })?
            .ok_or("device did not complete wifi/station/stop with wifi/station/stopped")?;
        if let Some(StationStopped(counters)) = message.decode() {
            return Ok(StopEnd::Stopped(counters));
        }
        Ok(StopEnd::Ended(
            message
                .decode::<StationServiceEnded>()
                .ok_or("the accepted stop message does not decode")?,
        ))
    }
}

#[derive(Clone, Debug, serde::Serialize)]
struct Link {
    at_us: u64,
    body: StationLink,
}
pub(super) struct Connected {
    epoch: u32,
    at_us: u64,
    sequence: u32,
    router_authorized: bool,
    associated_us: u64,
}
pub(super) fn authorized(
    capture: &SerialCapture,
    start: CommandHandle,
    epoch: u32,
) -> Result<Connected> {
    let (message,link)=capture.wait_for_after::<StationLink>(start.first_event(),Duration::from_secs(45),|message,link|
        start.correlates(message) && link.epoch>epoch && matches!(link.state,StationLinkState::Authorized{..}))?
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
pub(super) fn network(
    capture: &SerialCapture,
    start: CommandHandle,
    connected: &Connected,
) -> Result<oer_network_control::NetworkInfo> {
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
        return Err("Wi-Fi station requires managed OpenWrt".into());
    };
    let (key_run, keys) = keys::load(context.lab)?;
    if !keys.complete() {
        return Err(
            "independent key conformance evidence is incomplete or differs from the model".into(),
        );
    }
    context
        .results
        .observe("wifi-station-key-conformance-run", &key_run);
    let mut evidence = Evidence {
        keys: Some(keys),
        ..Default::default()
    };
    let directory = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router = oer_lab_router::lock::router(&directory, &config.ssh_target)?;
    let ssid = credentials::ssid("oer-w4")?;
    let passphrase = credentials::passphrase()?;
    let target = Mac::parse(&context.lab.dut.mac).ok_or("invalid Wi-Fi station identity")?;
    let target_text = target.octets().map(|byte| format!("{byte:02x}")).join(":");
    let results = context.results.clone();
    let log = CommandLog::new(
        &[
            ssid.expose(),
            passphrase.expose(),
            &config.ssh_target,
            &config.wireless_interface,
            &target_text,
        ],
        move |record| results.observe("wifi-station-router-command", &record),
    );
    let mut ssh = Ssh::new(&config.ssh_target)?.with_log(log);
    station_wpa2::verify_observation_tools(&mut ssh)?;
    let profile = router::Profile::station_wpa2(
        &config.radio,
        &config.ap_section,
        &config.wireless_interface,
        config.channel,
        ssid.expose(),
        passphrase.expose(),
    )?;
    let applied = router::apply(&mut ssh, &profile)?;
    station_wpa2::verify_runtime(&mut ssh, &config.radio, &config.wireless_interface)?;
    let interval =
        station_wpa2::group_rekey_seconds(&mut ssh, &config.radio, &config.wireless_interface)?;
    context
        .results
        .observe("wifi-station-rekey-seconds", &interval);
    if interval != station_wpa2::GROUP_REKEY_SECONDS {
        return Err("running Wi-Fi station BSS has the wrong group rekey interval".into());
    }
    drop(profile);
    let before = station::station(&mut ssh, &config.wireless_interface, target)?;
    if before.present {
        return Err("Wi-Fi station was already associated before StartStation".into());
    }
    let hostapd = super::observation::hostapd(&ssh, target)?;
    let dhcp = super::observation::Observer::start(
        &ssh,
        &format!(
            "exec timeout 240 tcpdump -n -e -tt -vvv -l -i {} 'udp port 67 or 68'",
            router::quote(&config.wireless_interface)?
        ),
    )?;
    let air = AirCapture::start_in_run(applied.channel)?;
    let mut links = Vec::new();
    let mut connected = Vec::new();
    let mut stop_at = 0;
    // When the host had StationStopped; the air capture must pass it.
    let mut stopped_us = None;
    let result = context.with_capture(output, |capture| {
        require_keys::<StartStation>(capture)?;
        require_keys::<GetStationStatus>(capture)?;
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
        let mut station_stopped = false;
        let work: Result<_> = (|| {
            connected.push(authorized(capture, start, 0)?);
            let info = network(capture, start, &connected[0])?;
            let address = Ipv4Addr::from(info.address);
            let gateway = Ipv4Addr::from(
                info.gateway
                    .ok_or("station DHCP observation has no gateway")?,
            );
            let joined = station::station(&mut ssh, &config.wireless_interface, target)?;
            let lease = station::lease(&mut ssh, target)?
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
                after: station::Station::default(),
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
            station_wpa2::deauthenticate(
                |script| ssh.output(script, COMMAND_TIMEOUT),
                &config.wireless_interface,
                target,
            )?;
            evidence.forced_deauth_us = Some(forced);
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
            match departure.finish()? {
                DepartureOutcome::Unique => {}
                outcome => {
                    return Err(format!(
                        "router has no unique fresh forced-departure event: {outcome}"
                    )
                    .into());
                }
            }
            if !station::station(&mut ssh, &config.wireless_interface, target)?.joined() {
                return Err("router did not authorize the reconnected station".into());
            }
            connected[1].router_authorized = true;
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
            let stopped = match stop_station(&Station { capture, start })? {
                StopEnd::Stopped(counters) => {
                    stopped_us = Some(now());
                    counters
                }
                StopEnd::Ended(end) => {
                    station_stopped = true;
                    context.results.observe("wifi-station-service-ended", &end);
                    return Err(ended(&end).into());
                }
            };
            station_stopped = true;
            context
                .results
                .observe("wifi-station-final-counters", &stopped);
            let deadline = Instant::now() + SHORT;
            let after = loop {
                let after = station::station(&mut ssh, &config.wireless_interface, target)?;
                if !after.present || Instant::now() >= deadline {
                    break after;
                }
                oer_os_process::sleep(Duration::from_millis(100))?;
            };
            match departure.finish()? {
                DepartureOutcome::Unique => {}
                outcome => {
                    return Err(format!(
                        "router has no unique fresh final-departure event: {outcome}"
                    )
                    .into());
                }
            }
            let b = evidence
                .baseline
                .as_mut()
                .expect("initial observations retained");
            b.tx_timeouts = stopped.tx_timeouts;
            b.queue_collisions = stopped.queue_collisions;
            b.deauth_transmissions = stopped.deauth_transmissions;
            b.after = after;
            Ok(())
        })();
        let work = stop_on_error(work, || {
            if station_stopped {
                return Ok(None);
            }
            stop_at = now();
            match stop_station(&Station { capture, start })? {
                StopEnd::Stopped(counters) => {
                    stopped_us = Some(now());
                    context
                        .results
                        .observe("wifi-station-failure-stopped", &counters);
                    Ok(None)
                }
                StopEnd::Ended(end) => {
                    context.results.observe("wifi-station-service-ended", &end);
                    Ok(Some(end))
                }
            }
        });
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
    if let Some(stopped_us) = stopped_us {
        let tail = air.await_after(stopped_us, TAIL_BOUND)?;
        context.results.observe("wifi-station-air-tail", &tail);
        if !tail.covered {
            eprintln!(
                "wifi-station air capture has no frame after StationStopped within {} s",
                TAIL_BOUND.as_secs()
            );
        }
    }
    let projected = air
        .finish_station(target.octets(), applied.bssid.octets())
        .and_then(|(frames, drops, stats)| {
            context.results.observe("wifi-station-air-health", &stats);
            context.results.observe("wifi-station-air", &frames);
            project(&mut evidence, &frames, drops, &connected, stop_at)?;
            Ok(())
        });
    context
        .results
        .observe("wifi-station-hostapd", &hostapd.finish()?);
    context.results.observe(
        "wifi-station-dhcp",
        &super::observation::dhcp(&dhcp.finish()?),
    );
    context.results.observe("wifi-station-link-events", &links);
    context.results.observe("wifi-station-evidence", &evidence);
    let gates = evaluate(&evidence);
    context.results.observe("wifi-station-gates", &gates);
    for gate in &gates {
        eprintln!(
            "wifi-station gate {}: {}{}",
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
            "Wi-Fi station gate {}: {}",
            gate.id,
            gate.reason.expect("failed gate has a reason")
        )
        .into());
    }
    context
        .results
        .claim("wifi-station", &["PTK rekey", "WPA3-SAE"]);
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
        return Err("Wi-Fi station requires OpenWrt".into());
    };
    let status = capture.request(0, GetStationStatus, SHORT)?;
    if !status.linked || status.stopped {
        return Err("station Control is not linked for the traffic window".into());
    }
    context.results.observe("wifi-station-status", &status);
    let observed = Capture::start_peer(ssh, &config.wireless_interface, address, count)?;
    let cursor = capture.event_cursor();
    let ping_ssh = ssh.parallel();
    let window = if initial.is_some() {
        super::ping::Window::GroupKeyChange
    } else {
        super::ping::Window::AfterReconnect
    };
    let (ping, rekey) = std::thread::scope(|scope| -> Result<_> {
        let ping = scope.spawn(|| station::ping(&ping_ssh, address, count));
        let work: Result<_> = (|| {
            let rekey = if let Some(initial) = initial {
                let (message, key) = capture
                    .wait_for_after::<StationGroupKey>(
                        cursor,
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
                let cursor = capture.event_cursor();
                let triggered_us = now();
                let neighbor = station_wpa2::refresh_neighbor(
                    |script| ssh.output(script, COMMAND_TIMEOUT),
                    station::network::BRIDGE,
                    address,
                    |state| {
                        context
                            .results
                            .observe("wifi-station-neighbor-state", state)
                    },
                )?;
                context
                    .results
                    .observe("wifi-station-neighbor-refresh", &neighbor);
                let deadline = Instant::now() + Duration::from_secs(5);
                let observe = |transmitted: bool| -> Result<ArpObservation> {
                    let (message, frame) = capture
                        .wait_for_after::<StationArpFrame>(
                            cursor,
                            deadline.saturating_duration_since(Instant::now()),
                            |message, frame| {
                                station.correlates(message)
                                    && frame.epoch == epoch
                                    && frame.generation == key.generation
                                    && frame.transmitted == transmitted
                                    && frame.group_addressed != transmitted
                                    && frame.operation == if transmitted { 2 } else { 1 }
                                    && frame.sender
                                        == if transmitted {
                                            address.octets()
                                        } else {
                                            router.octets()
                                        }
                                    && frame.target
                                        == if transmitted {
                                            router.octets()
                                        } else {
                                            address.octets()
                                        }
                            },
                        )?
                        .ok_or("post-GTK broadcast ARP request/station reply missing")?;
                    Ok(ArpObservation {
                        received_us: capture
                            .received_at_micros(&message)
                            .ok_or("ARP observation has no timestamp")?,
                        frame,
                        air_us: None,
                    })
                };
                let request = observe(false)?;
                let reply = observe(true)?;
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
                    epoch,
                    previous_generation: initial.group_generation,
                    generation: key.generation,
                    triggered_us,
                    request: Some(request),
                    reply: Some(reply),
                    arp_frames: after
                        .group_arp_received
                        .saturating_sub(before.group_arp_received),
                })
            } else {
                None
            };
            Ok(rekey)
        })();
        let ping = ping.join().map_err(|_| "router ping worker panicked")??;
        super::ping::record(&context.results, &context.measurements, window, &ping);
        let rekey = work?;
        Ok((ping, rekey))
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
        for (observation, direction) in [
            (&mut rekey.request, Direction::GroupFromAp),
            (&mut rekey.reply, Direction::FromStation),
        ] {
            if let Some(observation) = observation {
                observation.air_us = frames
                    .iter()
                    .find(|frame| {
                        frame.at_us >= rekey.triggered_us
                            && frame.at_us.saturating_sub(rekey.triggered_us) <= 5_000_000
                            && frame.direction == direction
                            && frame.protected
                            && !frame.eapol
                            && (0x20..=0x2f).contains(&frame.subtype)
                            && frame.key_id == Some(observation.frame.key_id)
                            && frame.sequence == Some(observation.frame.sequence)
                            && frame.packet_number == Some(observation.frame.packet_number)
                    })
                    .map(|frame| frame.at_us);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
