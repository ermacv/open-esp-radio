//! HIL commands, lifecycle observations and network workload over the facade.
use super::{Attempt, ObservationStorage, radio};
use core::cell::Cell;
use oer_esp32s31_wifi as wifi;
use oer_esp32s31_wifi::NonceSource;
use oer_ieee80211_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap};
use oer_ieee80211_control::StationServiceEnd;
use oer_memory::{OnceValue, Placed};
pub struct Callbacks(pub Placed<'static, NonceSource>);
impl wifi::Callbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0
            .next_nonce()
            .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control))
    }
}
static ATTEMPT: OnceValue<Attempt> = OnceValue::new();
pub struct ProductEvents(pub &'static oer_ieee80211_control::PendingCommands);
impl wifi::Events for ProductEvents {
    fn event(&mut self, event: wifi::Event) {
        let attempt = ATTEMPT
            .get()
            .unwrap_or_else(|| radio::failure(wifi::InitializationError::Control));
        match event {
            wifi::Event::Connected { bssid, channel } => {
                attempt.publish(oer_ieee80211_control::StationLinkState::Authorized {
                    bssid,
                    channel,
                });
            }
            wifi::Event::Disconnected(reason) => {
                attempt.publish(oer_ieee80211_control::StationLinkState::Disconnected {
                    reason: reason.into(),
                });
            }
            wifi::Event::CommandCompleted(terminal) => {
                let (identity, event) = self.0.finished(
                    terminal.request,
                    oer_ieee80211_control::StationCommandResult::Completed(terminal.outcome.into()),
                    attempt.request,
                );
                super::emit(identity, &event);
            }
            wifi::Event::CommandRefused(refused) => {
                let (identity, event) = self.0.finished(
                    refused.request,
                    oer_ieee80211_control::StationCommandResult::Refused(refused.reason.into()),
                    attempt.request,
                );
                super::emit(identity, &event);
            }
            wifi::Event::CommandInFlight(request) => {
                let (identity, event) = self.0.finished(
                    request,
                    oer_ieee80211_control::StationCommandResult::InFlight,
                    attempt.request,
                );
                super::emit(identity, &event);
            }
            wifi::Event::TxRefused { operation, reason } => super::emit(
                attempt.request,
                &oer_ieee80211_control::StationTransmitRefused {
                    operation: operation.into(),
                    reason: reason.into(),
                },
            ),
            wifi::Event::MsduRefused { packet, reason } => {
                super::emit(
                    attempt.request,
                    &oer_ieee80211_control::StationMsduRefusal::from(reason),
                );
                drop(packet);
            }
            wifi::Event::CommandStarted { request, status }
                if oer_ieee80211_control::StationCommandKind::from(request.command())
                    == oer_ieee80211_control::StationCommandKind::Connect
                    && !oer_ieee80211_control::StationStatus::from(status).linked =>
            {
                attempt.advance();
            }
            _ => {}
        }
    }
}
pub struct Session {
    pub attempt: Attempt,
    pub pending: &'static oer_ieee80211_control::PendingCommands,
    pub observations: &'static ObservationStorage,
}
pub async fn serve(
    runner: wifi::StationRunner<radio::binding::Binding>,
    mut control: wifi::Control<'static>,
    net: wifi::Network<'static>,
    profile: wifi::Profile<radio::binding::Binding>,
    address: [u8; 6],
    seed: u64,
    session: Session,
) {
    let Session {
        attempt,
        pending,
        observations,
    } = session;
    let _ = ATTEMPT.set(attempt);
    let stopped = Cell::new(None);
    let (network_control, network) = super::ip::start(address, seed, net, attempt, observations);
    let mut network = core::pin::pin!(network);
    let mut running = core::pin::pin!(runner.run(profile, SoftwareKeyUnwrap::new(SoftwareAes128)));
    let outcome = {
        let mut controls = core::pin::pin!(commands(&mut control, &stopped, pending, observations));
        let mut workload = core::pin::pin!(embassy_futures::join::join(
            super::future::Boundary(network.as_mut()),
            super::future::Boundary(controls.as_mut())
        ));
        match embassy_futures::select::select(
            super::future::Boundary(running.as_mut()),
            super::future::Boundary(workload.as_mut()),
        )
        .await
        {
            embassy_futures::select::Either::First(outcome) => outcome,
            embassy_futures::select::Either::Second(_) => {
                crate::fail(crate::fatal::Failure::RootInitialization)
            }
        }
    };
    // Join the test application while continuing to poll its socket stack.
    match embassy_futures::select::select(
        super::future::Boundary(network.as_mut()),
        super::udp::stop_for_station(),
    )
    .await
    {
        embassy_futures::select::Either::First(()) => {
            crate::fail(crate::fatal::Failure::RootInitialization)
        }
        embassy_futures::select::Either::Second(()) => {}
    }
    network_control.stop();
    super::future::Boundary(network.as_mut()).await;
    let status = control.status();
    super::ENDED.store(true, core::sync::atomic::Ordering::Release);
    for (identity, event) in pending.finish_stopped(status.into()).into_iter().flatten() {
        super::emit(identity, &event);
    }
    while let Ok((request, command)) = super::COMMANDS.try_receive() {
        let admission = pending.admit(&mut Commands(&mut control), request, command);
        super::emit(request, &admission);
    }
    let driver = super::DRIVER_OBSERVATIONS
        .freeze()
        .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control));
    let capture = observations
        .freeze(&driver, super::DRIVER_OBSERVATIONS.failure())
        .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control));
    match outcome {
        wifi::StationExit::Unavailable(_) => super::end(attempt, StationServiceEnd::Unavailable),
        wifi::StationExit::Stopped => {
            if let Some(request) = stopped.get() {
                super::emit(
                    request,
                    &oer_ieee80211_control::StationStopped {
                        counters: super::counters(observations),
                        capture,
                    },
                );
            }
        }
        wifi::StationExit::Failed(fault) => super::end(
            attempt,
            StationServiceEnd::Failed(
                oer_ieee80211_control::StationServiceFault::encode(&fault)
                    .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control)),
            ),
        ),
        wifi::StationExit::Poisoned(fault) => super::end(
            attempt,
            StationServiceEnd::Poisoned(
                oer_ieee80211_control::StationServiceFault::encode(&fault)
                    .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control)),
            ),
        ),
        wifi::StationExit::Quarantined(fault) => super::end(
            attempt,
            StationServiceEnd::Quarantined(
                oer_ieee80211_control::StationServiceFault::encode(&fault)
                    .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control)),
            ),
        ),
    }
}
async fn commands(
    control: &mut wifi::Control<'static>,
    stopped: &Cell<Option<oer_hil_protocol::RequestIdentity>>,
    pending: &oer_ieee80211_control::PendingCommands,
    observations: &ObservationStorage,
) -> ! {
    use embassy_futures::select::{Either, select};
    loop {
        match select(super::COMMANDS.receive(), super::STOPS.receive()).await {
            Either::First((request, command)) => {
                let admission = pending.admit(&mut Commands(control), request, command);
                super::emit(request, &admission);
            }
            Either::Second(request) => {
                stopped.set(Some(request));
                super::udp::stop_for_station().await;
                observations.departing();
                control.stop();
                core::future::pending::<()>().await;
            }
        }
    }
}

struct Commands<'a>(&'a mut wifi::Control<'static>);
impl oer_ieee80211_control::StationControl for Commands<'_> {
    fn try_command(
        &mut self,
        kind: oer_ieee80211_control::StationCommandKind,
        correlation: u64,
    ) -> Result<(), oer_ieee80211_control::StationControlError> {
        use oer_ieee80211_control::StationCommandKind as K;
        match kind {
            K::Connect => self.0.try_connect(correlation),
            K::Disconnect => self.0.try_disconnect(correlation),
            K::BackgroundScan => self.0.try_scan(correlation),
        }
        .map_err(Into::into)
    }
    fn status(&self) -> oer_ieee80211_control::StationStatus {
        self.0.status().into()
    }
}
