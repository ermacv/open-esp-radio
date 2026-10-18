//! Single-device on-air check of the composed IEEE 802.15.4 client.
//!
//! The check splits the radio root concurrently and runs every cycle through
//! `oer-esp32s31-i-system`: start on the shared radio arbiter, then
//! a port body that enables the radio and runs one energy scan, one
//! clear-channel assessment, one direct transmit without an acknowledgement
//! request, two transmits scheduled through the MAC timer and ETM route, one
//! promiscuous receive window and one receive window scheduled through the
//! MAC timer, then disables the radio; then stop. The evidence records
//! outcomes and target-monotonic times; the host judges them.

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer, with_timeout};
use oer_esp32s31_ieee802154_system::Esp32s31Ieee802154;
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_espressif_ieee802154_runtime::{Ieee802154PortEvent, Ieee802154RadioEvent};
use oer_hil_protocol::{
    ieee802154::Ieee802154AirCcaOutcome, ieee802154::Ieee802154AirCheckEvidence,
    ieee802154::Ieee802154AirCheckRequest, ieee802154::Ieee802154AirCheckStop,
    ieee802154::Ieee802154AirCycle, ieee802154::Ieee802154AirEnergyOutcome,
    ieee802154::Ieee802154AirTransmit, ieee802154::Ieee802154AirWindow,
};
use oer_ieee802154::{
    Channel, EventsLost, FrameView, Ieee802154Instant, TxMode, TxSecurity,
    owner::{
        Has, Ieee802154Client, Ieee802154Clients, Ieee802154Setup, Operation, RadioSettings,
        SupportedCca, SupportedEnergyScan, SupportedScheduledReceive, SupportedTransmit,
        Transmission,
    },
};
use oer_ieee802154_port::{Disabled, Ended, Owner, PortBody, RadioPortEvents, run_port};
use oer_radio_port::Seat;

use super::client::{Backend, Client, now_micros, tx_outcome};

/// Bound on one terminal event after its command started.
const EVENT_TIMEOUT: Duration = Duration::from_millis(500);

/// A data frame with PAN ID compression and short addresses, without an
/// acknowledgement request: broadcast destination 0xffff on PAN 0x4f45 from
/// short address 0x0154, payload `OER-154`. The FCS is appended by the MAC.
const FRAME: [u8; 16] = [
    0x41, 0x88, 0x00, 0x45, 0x4f, 0xff, 0xff, 0x54, 0x01, b'O', b'E', b'R', b'-', b'1', b'5', b'4',
];

/// Why one cycle stopped early.
struct Stop(Ieee802154AirCheckStop);

/// Run the air check. The image is terminal: the radio stays split.
pub(in crate::product_hil) async fn run_air_check(
    spawner: embassy_executor::Spawner,
    platform: EspHalRadioPlatform,
    request: Ieee802154AirCheckRequest,
) -> Ieee802154AirCheckEvidence {
    let mut evidence = Ieee802154AirCheckEvidence::default();
    let Ok(channel) = Channel::new(request.channel) else {
        return evidence;
    };
    let Some((mut client, mut parked)) = Client::claim(spawner, platform) else {
        return evidence;
    };
    for index in 0..usize::from(request.cycles) {
        let started = {
            let started = core::pin::pin!(client.start(parked));
            started.await
        };
        let Some((system, backend)) = started else {
            evidence.stop = Ieee802154AirCheckStop::StartFailed;
            return evidence;
        };
        let mut outcome = Ok(());
        let ended = {
            let ended = core::pin::pin!(run_port(
                backend,
                CycleBody {
                    channel,
                    request,
                    cycle: &mut evidence.cycles[index],
                    outcome: &mut outcome,
                },
            ));
            ended.await
        };
        let Some(parts) = ended else {
            evidence.stop = Ieee802154AirCheckStop::StopFailed;
            return evidence;
        };
        let stopped = {
            let stopped = core::pin::pin!(client.stop(system, parts));
            stopped.await
        };
        let Some(stopped) = stopped else {
            evidence.stop = Ieee802154AirCheckStop::StopFailed;
            return evidence;
        };
        parked = stopped;
        if let Err(Stop(stop)) = outcome {
            evidence.stop = stop;
            return evidence;
        }
        evidence.completed_cycles += 1;
    }
    evidence.stop = Ieee802154AirCheckStop::Complete;
    evidence
}

/// One cycle's port body.
struct CycleBody<'c> {
    channel: Channel,
    request: Ieee802154AirCheckRequest,
    cycle: &'c mut Ieee802154AirCycle,
    outcome: &'c mut Result<(), Stop>,
}

impl PortBody<Backend> for CycleBody<'_> {
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, Backend, Disabled>,
        resources: <Backend as oer_ieee802154_port::PortBackend>::Resources<'id>,
    ) -> Ended<'id, Backend> {
        let zero = owner.setup().configure(resources.interfaces.interface);
        let [mut seat, ..] = resources.operation;
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => {
                *self.outcome = Err(Stop(Ieee802154AirCheckStop::CommandRejected));
                return failed.fail_stop();
            }
        };
        *self.outcome = {
            let parts = owner.split();
            let mut events = parts.events;
            let mut control = parts.control;
            let client = parts.clients.client(&zero);
            run_cycle(
                &mut *events,
                &mut *control,
                &client,
                &mut seat,
                self.channel,
                self.request,
                self.cycle,
            )
            .await
        };
        match owner.disable(&mut |_| {}).await {
            Ok(owner) => owner.uninstall(()),
            Err(poisoned) => poisoned.uninstall(()),
        }
    }
}

async fn run_cycle<'id, E, S, C>(
    events: &mut E,
    control: &mut S,
    client: &C,
    seat: &mut Seat<'id, Operation>,
    channel: Channel,
    request: Ieee802154AirCheckRequest,
    cycle: &mut Ieee802154AirCycle,
) -> Result<(), Stop>
where
    E: RadioPortEvents<Event = Ieee802154PortEvent>,
    S: RadioSettings<Caps = Esp32s31Ieee802154>,
    C: Ieee802154Client<'id, 0, Caps = Esp32s31Ieee802154>,
{
    let rejected = |_| Stop(Ieee802154AirCheckStop::CommandRejected);
    let unsupported = || Stop(Ieee802154AirCheckStop::UnsupportedSetup);

    let scan =
        SupportedEnergyScan::new(channel, request.energy_scan_micros).ok_or_else(unsupported)?;
    let permit = client.operation(seat).await;
    client.energy_scan(permit, scan).map_err(rejected)?.id();
    cycle.energy = match next_event(events).await? {
        Ieee802154RadioEvent::EnergyScanDone { energy_dbm, .. } => {
            Ieee802154AirEnergyOutcome::Energy(energy_dbm)
        }
        Ieee802154RadioEvent::EnergyScanFailed { .. } => Ieee802154AirEnergyOutcome::Failed,
        _ => return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent)),
    };

    let cca = SupportedCca::new(channel).ok_or_else(unsupported)?;
    let permit = client.operation(seat).await;
    client
        .clear_channel_assessment(permit, cca)
        .map_err(rejected)?
        .id();
    cycle.cca = match next_event(events).await? {
        Ieee802154RadioEvent::ClearChannelAssessmentDone { idle: true, .. } => {
            Ieee802154AirCcaOutcome::Clear
        }
        Ieee802154RadioEvent::ClearChannelAssessmentDone { idle: false, .. } => {
            Ieee802154AirCcaOutcome::Busy
        }
        Ieee802154RadioEvent::ClearChannelAssessmentFailed { .. } => {
            Ieee802154AirCcaOutcome::Failed
        }
        _ => return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent)),
    };

    let frame = FrameView::new(&FRAME).map_err(|_| unsupported())?;
    let transmission = |mode| {
        SupportedTransmit::new(Transmission {
            frame,
            channel,
            mode,
            power: None,
            max_frame_retries: 0,
            security: TxSecurity::Radio,
            time_sync: None,
        })
        .map_err(|_| unsupported())
    };
    let requested_at_micros = now_micros();
    let permit = client.operation(seat).await;
    client
        .transmit(permit, transmission(TxMode::Direct)?)
        .map_err(rejected)?
        .id();
    cycle.direct = transmitted(events, requested_at_micros).await?;

    for scheduled in &mut cycle.scheduled {
        let at = now_micros() + u64::from(request.scheduled_lead_micros);
        let mode = TxMode::Scheduled {
            at: Ieee802154Instant::from_micros(at),
            cca: false,
        };
        let permit = client.operation(seat).await;
        client
            .transmit(permit, transmission(mode)?)
            .map_err(rejected)?
            .id();
        *scheduled = transmitted(events, at).await?;
    }

    let promiscuous = Has::new().ok_or_else(unsupported)?;
    control.set_promiscuous(promiscuous, true);
    client.receive(channel).await.map_err(rejected)?;
    let window = Timer::after(Duration::from_millis(u64::from(
        request.receive_window_millis,
    )));
    let mut window = core::pin::pin!(window);
    loop {
        match select(window.as_mut(), taken(events)).await {
            Either::First(()) => break,
            Either::Second(Ok(Ieee802154RadioEvent::Received(frame))) => {
                cycle.received_frames = cycle.received_frames.saturating_add(1);
                let rssi = frame.metadata.rssi_dbm;
                cycle.strongest_rssi_dbm = Some(match cycle.strongest_rssi_dbm {
                    Some(strongest) => strongest.max(rssi),
                    None => rssi,
                });
            }
            Either::Second(Ok(_)) => {
                return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent));
            }
            Either::Second(Err(stop)) => return Err(stop),
        }
    }
    // Leave the receive window; frames it flushed are not the scheduled
    // window's.
    client.sleep();
    loop {
        match with_timeout(Duration::from_millis(1), taken(events)).await {
            Ok(Ok(Ieee802154RadioEvent::Received(_))) => {}
            Ok(Ok(_)) => return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent)),
            Ok(Err(stop)) => return Err(stop),
            Err(_) => break,
        }
    }

    let start_micros = now_micros() + u64::from(request.scheduled_lead_micros);
    let window = &mut cycle.scheduled_window;
    *window = Ieee802154AirWindow {
        start_micros,
        end_micros: start_micros + u64::from(request.scheduled_window_micros),
        ..Ieee802154AirWindow::default()
    };
    let scheduled = SupportedScheduledReceive::new(
        channel,
        Ieee802154Instant::from_micros(start_micros),
        request.scheduled_window_micros,
    )
    .ok_or_else(unsupported)?;
    client.scheduled_receive(scheduled);
    let bound = Duration::from_micros(
        u64::from(request.scheduled_lead_micros) + u64::from(request.scheduled_window_micros),
    ) + EVENT_TIMEOUT;
    let window_end = Timer::after(bound);
    let mut window_end = core::pin::pin!(window_end);
    loop {
        let ended = client.window_ended();
        let ended = core::pin::pin!(ended);
        match select(window_end.as_mut(), select(ended, taken(events))).await {
            Either::First(()) => {
                return Err(Stop(Ieee802154AirCheckStop::EventTimeout));
            }
            Either::Second(Either::First(Ok(()))) => {
                window.done_at_micros = now_micros();
                window.ended = true;
                return Ok(());
            }
            Either::Second(Either::First(Err(_))) => {
                return Err(Stop(Ieee802154AirCheckStop::CommandRejected));
            }
            Either::Second(Either::Second(Ok(Ieee802154RadioEvent::Received(frame)))) => {
                window.received_frames = window.received_frames.saturating_add(1);
                if window.first_frame_at_micros.is_none() {
                    window.first_frame_at_micros =
                        frame.metadata.timestamp.map(Ieee802154Instant::as_micros);
                }
            }
            Either::Second(Either::Second(Ok(_))) => {
                return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent));
            }
            Either::Second(Either::Second(Err(stop))) => return Err(stop),
        }
    }
}

/// The port's next event: a lost frame or a poisoned port stops the cycle.
async fn taken<E>(events: &mut E) -> Result<Ieee802154RadioEvent, Stop>
where
    E: RadioPortEvents<Event = Ieee802154PortEvent>,
{
    match events.next_event().await {
        Ok(Ok(event)) => Ok(event.event),
        Ok(Err(EventsLost)) => Err(Stop(Ieee802154AirCheckStop::EventsLost)),
        Err(_) => Err(Stop(Ieee802154AirCheckStop::CommandRejected)),
    }
}

async fn next_event<E>(events: &mut E) -> Result<Ieee802154RadioEvent, Stop>
where
    E: RadioPortEvents<Event = Ieee802154PortEvent>,
{
    match with_timeout(EVENT_TIMEOUT, taken(events)).await {
        Ok(event) => event,
        Err(_) => Err(Stop(Ieee802154AirCheckStop::EventTimeout)),
    }
}

async fn transmitted<E>(
    events: &mut E,
    requested_at_micros: u64,
) -> Result<Ieee802154AirTransmit, Stop>
where
    E: RadioPortEvents<Event = Ieee802154PortEvent>,
{
    let Ieee802154RadioEvent::TransmitDone { status, .. } = next_event(events).await? else {
        return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent));
    };
    Ok(Ieee802154AirTransmit {
        outcome: tx_outcome(status),
        requested_at_micros,
        done_at_micros: now_micros(),
    })
}
