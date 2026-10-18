//! Host-driven IEEE 802.15.4 peer session.
//!
//! The session starts the composed client once, applies the host's identity
//! and filter, and then serves the host's commands until it stops: transmit
//! and report the outcome with any acknowledgement, enter receive mode,
//! report and forget the frames received so far, and change the automatic
//! frame-pending decision, and measure and assess any channel. The host may
//! ask for enhanced ACKs of 2015 frames. Received frames accumulate between collections,
//! so the host can drive the peer while the device listens. Each install of
//! the radio runs as one port body; a restart ends the body, stops and
//! starts the client, and runs a new body, whose pending-table entries
//! start empty.

mod radio;
mod serve;

use core::cell::Cell;

use embassy_futures::join::join;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use heapless::Vec;
use oer_esp32s31_ieee802154_system::Ieee802154System;
use oer_esp32s31_radio_esp_hal::EspHalRadioClocks;
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_esp32s31_radio_runtime::RadioSystem;
use oer_hil_protocol::{
    ieee802154::Ieee802154SessionCoexistence, ieee802154::Ieee802154SessionConfig,
    ieee802154::Ieee802154SessionMaintenanceCounts, ieee802154::Ieee802154SessionRestartEvidence,
    ieee802154::Ieee802154SessionResult, ieee802154::Ieee802154SessionStopEvidence,
};
use oer_ieee802154::{
    Channel,
    owner::{Ieee802154Client, Ieee802154Clients, Ieee802154Setup},
};
use oer_ieee802154_port::{Disabled, Ended, Owner, PortBackend, PortBody, run_port};

use super::client::{Backend, Client};
use crate::console::publish_event_reliably;

use radio::{Received, Session};
use serve::{Served, serve_session};

type Radio = RadioSystem<EspHalRadioPlatform, EspHalRadioClocks, oer_time_embassy::EmbassyClock>;

/// What the body publishes once the radio is configured: the session's
/// start, or a restart's evidence.
enum Opening {
    Started(u32),
    Restarted(u32, Ieee802154SessionRestartEvidence),
}

/// One install's port body: configure interface 0, enable the radio,
/// apply the host's configuration, publish the opening, then serve the
/// host's commands until it stops the session or restarts the radio.
struct SessionBody<'b> {
    config: Ieee802154SessionConfig,
    channel: Channel,
    opening: Opening,
    system: &'b mut Ieee802154System,
    client: &'b Client,
    counts: &'b Cell<Ieee802154SessionMaintenanceCounts>,
    received: &'b mut Received,
    served: &'b mut Option<Served>,
}

impl PortBody<Backend> for SessionBody<'_> {
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, Backend, Disabled>,
        resources: <Backend as PortBackend>::Resources<'id>,
    ) -> Ended<'id, Backend> {
        let interface = resources.interfaces;
        let zero = owner.setup().configure(interface.interface);
        let [mut seat, ..] = resources.operation;
        let rssi = resources.rssi;
        let enabled = owner.enable().await;
        let mut owner = match enabled {
            Ok(owner) => owner,
            Err(failed) => {
                open(self.opening, Ieee802154SessionResult::StartFailed).await;
                return failed.fail_stop();
            }
        };
        {
            let parts = owner.split();
            let mut events = parts.events;
            let mut control = parts.control;
            let client = parts.clients.client(&zero);
            let mut session = Session {
                events: &mut *events,
                control: &mut *control,
                client: &client,
                seat: &mut seat,
                rssi: &rssi,
                short_free: Vec::from_array(interface.short),
                short: Vec::new(),
                channel: self.channel,
                received: self.received,
                lost: false,
            };
            let mut result = match session.configure(self.config) {
                Ok(()) => Ieee802154SessionResult::Done,
                Err(result) => result,
            };
            // A restarted radio receives again, as before its restart.
            if matches!(self.opening, Opening::Restarted(..))
                && result == Ieee802154SessionResult::Done
                && let Err(failed) = session.receive().await
            {
                result = failed;
            }
            open(self.opening, result).await;
            let served = {
                let served = core::pin::pin!(serve_session(
                    &mut session,
                    self.system,
                    self.client,
                    self.config,
                    self.counts,
                ));
                served.await
            };
            session.clear_pending();
            session.client.sleep();
            *self.served = Some(served);
        }
        match owner.disable(&mut |_| {}).await {
            Ok(owner) => owner.uninstall(()),
            Err(poisoned) => poisoned.uninstall(()),
        }
    }
}

/// Publish what the body opened with.
async fn open(opening: Opening, result: Ieee802154SessionResult) {
    match opening {
        Opening::Started(request_id) => {
            publish_event_reliably(
                0,
                request_id,
                oer_hil_protocol::ieee802154::SessionStarted(result),
            )
            .await;
        }
        Opening::Restarted(request_id, mut evidence) => {
            evidence.result = result;
            publish_event_reliably(
                0,
                request_id,
                oer_hil_protocol::ieee802154::SessionRadioRestarted(evidence),
            )
            .await;
        }
    }
}

/// Run one peer session until the host stops it. The image is terminal.
pub(in crate::product_hil) async fn run_session(
    spawner: embassy_executor::Spawner,
    platform: EspHalRadioPlatform,
    request_id: u32,
    config: Ieee802154SessionConfig,
) {
    let Some(channel) = Channel::new(config.channel).ok() else {
        publish_event_reliably(
            0,
            request_id,
            oer_hil_protocol::ieee802154::SessionStarted(Ieee802154SessionResult::UnsupportedSetup),
        )
        .await;
        return;
    };
    let Some((mut client, parked)) = Client::claim(spawner, platform) else {
        publish_event_reliably(
            0,
            request_id,
            oer_hil_protocol::ieee802154::SessionStarted(Ieee802154SessionResult::UnsupportedSetup),
        )
        .await;
        return;
    };
    let started = {
        let started = core::pin::pin!(client.start(parked));
        started.await
    };
    let Some((mut system, mut backend)) = started else {
        publish_event_reliably(
            0,
            request_id,
            oer_hil_protocol::ieee802154::SessionStarted(Ieee802154SessionResult::StartFailed),
        )
        .await;
        return;
    };
    report_power_sequence_once(&system);
    // Coexistence with Wi-Fi, as a Thread border router enters it.
    let mut coexistence = Ieee802154SessionCoexistence::default();
    if config.wifi_coexistence {
        system = system.enable_wifi_coexistence(client.radio).await;
        coexistence.enabled = true;
    }
    let counts = Cell::new(Ieee802154SessionMaintenanceCounts::default());
    let mut received = Received::default();
    let mut opening = Opening::Started(request_id);

    // The coexistence schedule runs beside the session while it takes part.
    let schedule_stop = Signal::<CriticalSectionRawMutex, ()>::new();
    let schedule = async {
        if coexistence.enabled {
            client
                .radio
                .run_coex_schedule_until(schedule_stop.wait())
                .await;
        }
    };
    let serving = async {
        loop {
            let mut served = None;
            let ended = {
                let ended = core::pin::pin!(run_port(
                    backend,
                    SessionBody {
                        config,
                        channel,
                        opening,
                        system: &mut system,
                        client: &client,
                        counts: &counts,
                        received: &mut received,
                        served: &mut served,
                    },
                ));
                ended.await
            };
            let Some(parts) = ended else {
                // The port was poisoned and fail-stopped: the image ends
                // without its client.
                schedule_stop.signal(());
                return None;
            };
            match served {
                Some(Served::Restart(request_id)) => {
                    // Stop the client and start it again, as the air check
                    // does between cycles: the stop leaves the shared PHY
                    // and closes RF after the last client, the start joins
                    // and wakes it.
                    let mut evidence = Ieee802154SessionRestartEvidence::default();
                    let stopped = {
                        let stopped = core::pin::pin!(client.stop(system, parts));
                        stopped.await
                    };
                    let Some(parked) = stopped else {
                        evidence.result = Ieee802154SessionResult::StopFailed;
                        open(Opening::Restarted(request_id, evidence), evidence.result).await;
                        schedule_stop.signal(());
                        return None;
                    };
                    evidence.rf_closed = client.radio.lock().await.lease().attachment().rf_closed();
                    let started = {
                        let started = core::pin::pin!(client.start(parked));
                        started.await
                    };
                    let Some((restarted, restarted_backend)) = started else {
                        evidence.result = Ieee802154SessionResult::StartFailed;
                        open(Opening::Restarted(request_id, evidence), evidence.result).await;
                        schedule_stop.signal(());
                        return None;
                    };
                    system = restarted;
                    backend = restarted_backend;
                    opening = Opening::Restarted(request_id, evidence);
                }
                Some(Served::Stop(stop_request)) => {
                    schedule_stop.signal(());
                    return Some((stop_request, system, parts));
                }
                // The body ended before it served: its start failed.
                None => {
                    schedule_stop.signal(());
                    return None;
                }
            }
        }
    };
    let ((), served) = core::pin::pin!(join(schedule, serving)).await;
    let Some((stop_request, mut system, parts)) = served else {
        return;
    };
    if coexistence.enabled {
        system = match system.disable_wifi_coexistence(client.radio).await {
            Ok(system) => system,
            Err((system, _)) => {
                coexistence.disable_failed = true;
                system
            }
        };
    }
    let stopped = {
        let stopped = core::pin::pin!(client.stop(system, parts));
        stopped.await
    };
    let stopped = match stopped {
        Some(_parked) => Ieee802154SessionResult::Done,
        None => Ieee802154SessionResult::StopFailed,
    };
    publish_event_reliably(
        0,
        stop_request,
        oer_hil_protocol::ieee802154::SessionStopped(Ieee802154SessionStopEvidence {
            result: stopped,
            maintenance: counts.get(),
            coexistence,
        }),
    )
    .await;
}

/// Print the MAC power-sequencing words once per boot, before the first
/// transmission, read under the runtime's lock.
fn report_power_sequence_once(system: &Ieee802154System) {
    static REPORTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if REPORTED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    crate::console::ieee802154_power_sequence_report(system.power_sequence());
}
