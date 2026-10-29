//! HIL commands, lifecycle observations and network workload over the facade.
use super::{Attempt, observation, radio};
use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_wifi as wifi;
use oer_esp32s31_wifi::NonceSource;
use oer_ieee80211_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap};
use oer_ieee80211_control::StationServiceEnd;
use oer_memory::{OnceValue, Placed};
use oer_time::{Duration, Timer};
pub struct Callbacks(pub Placed<'static, NonceSource>);
impl wifi::Callbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0
            .next_nonce()
            .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control))
    }
}
static ATTEMPT: OnceValue<Attempt> = OnceValue::new();
pub static LINK: AtomicBool = AtomicBool::new(false);
static AUTOCONNECT: AtomicBool = AtomicBool::new(true);
static RETRY: AtomicBool = AtomicBool::new(true);
static READY: AtomicBool = AtomicBool::new(true);
pub struct ProductEvents(pub &'static oer_ieee80211_control::PendingCommands);
impl wifi::Events for ProductEvents {
    fn event(&mut self, event: wifi::Event) {
        let attempt = ATTEMPT
            .get()
            .unwrap_or_else(|| radio::failure(wifi::InitializationError::Control));
        match event {
            wifi::Event::Connected { bssid, channel } => {
                LINK.store(true, Relaxed);
                attempt.publish(
                    oer_ieee80211_runtime::station::contract::Notice::Connected { bssid, channel }
                        .into(),
                );
            }
            wifi::Event::Disconnected(reason) => {
                LINK.store(false, Relaxed);
                RETRY.store(AUTOCONNECT.load(Relaxed), Relaxed);
                attempt.publish(
                    oer_ieee80211_runtime::station::contract::Notice::Disconnected(reason).into(),
                );
            }
            wifi::Event::CommandCompleted(terminal) => {
                if terminal.request.command()
                    == oer_ieee80211_runtime::station::contract::Command::Connect
                {
                    READY.store(true, Relaxed);
                }
                publish_command(
                    self.0,
                    attempt,
                    terminal.request,
                    oer_ieee80211_control::StationCommandResult::Completed(terminal.outcome.into()),
                );
            }
            wifi::Event::CommandRefused(refused) => {
                publish_command(
                    self.0,
                    attempt,
                    refused.request,
                    oer_ieee80211_control::StationCommandResult::Refused(refused.reason.into()),
                );
                if refused.request.command()
                    == oer_ieee80211_runtime::station::contract::Command::Connect
                {
                    READY.store(true, Relaxed);
                    RETRY.store(AUTOCONNECT.load(Relaxed) && !LINK.load(Relaxed), Relaxed);
                }
            }
            wifi::Event::CommandInFlight(request) => {
                publish_command(
                    self.0,
                    attempt,
                    request,
                    oer_ieee80211_control::StationCommandResult::InFlight,
                );
            }
            wifi::Event::TxRefused { operation, reason } => super::emit(
                attempt.request,
                &oer_ieee80211_control::StationTransmitRefused {
                    operation: operation.into(),
                    reason: reason.into(),
                },
            ),
            wifi::Event::Other(oer_ieee80211_runtime::station::ServiceOutput::MsduRefused {
                packet,
                reason,
            }) => {
                super::emit(
                    attempt.request,
                    &oer_ieee80211_control::StationMsduRefusal::from(reason),
                );
                drop(packet);
            }
            _ => {}
        }
    }
}
pub struct Session {
    pub attempt: Attempt,
    pub pending: &'static oer_ieee80211_control::PendingCommands,
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
    let Session { attempt, pending } = session;
    let _ = ATTEMPT.set(attempt);
    let stopped = Cell::new(None);
    let mut running = core::pin::pin!(runner.run(profile, SoftwareKeyUnwrap::new(SoftwareAes128)));
    let early = {
        let mut started = core::pin::pin!(control.wait_started());
        match embassy_futures::select::select(
            super::future::Boundary(running.as_mut()),
            started.as_mut(),
        )
        .await
        {
            embassy_futures::select::Either::First(outcome) => Some(outcome),
            embassy_futures::select::Either::Second(Ok(())) => None,
            embassy_futures::select::Either::Second(Err(_)) => {
                Some(super::future::Boundary(running.as_mut()).await)
            }
        }
    };
    let outcome = match early {
        Some(outcome) => outcome,
        None => {
            let mut controls = core::pin::pin!(reconnect(&mut control, attempt, &stopped, pending));
            let mut workload = core::pin::pin!(embassy_futures::join::join(
                super::network::run(address, seed, net, attempt),
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
        }
    };
    let status = control.status();
    super::ENDED.store(true, core::sync::atomic::Ordering::Release);
    for (identity, event) in pending.finish_stopped(status).into_iter().flatten() {
        super::emit(identity, &event);
    }
    while let Ok((request, command)) = super::COMMANDS.try_receive() {
        let admission = pending.admit(&mut Commands(&mut control), request, command);
        super::emit(request, &admission);
    }
    match outcome {
        wifi::StationExit::Unavailable(_) => super::end(attempt, StationServiceEnd::Unavailable),
        wifi::StationExit::Stopped => {
            if let Some(request) = stopped.get() {
                super::emit(
                    request,
                    &oer_ieee80211_control::StationStopped(observation::counters()),
                );
            }
        }
        wifi::StationExit::Failed(fault) => super::end(
            attempt,
            StationServiceEnd::Failed(super::report::service(fault)),
        ),
        wifi::StationExit::Poisoned(fault) => super::end(
            attempt,
            StationServiceEnd::Poisoned(super::report::service(fault)),
        ),
        wifi::StationExit::Quarantined(fault) => super::end(
            attempt,
            StationServiceEnd::Quarantined(super::report::service(fault)),
        ),
    }
}
async fn reconnect(
    commands: &mut wifi::Control<'static>,
    attempt: Attempt,
    stopped: &Cell<Option<oer_hil_protocol::RequestIdentity>>,
    pending: &oer_ieee80211_control::PendingCommands,
) -> ! {
    let mut first = true;
    let mut disconnect = true;
    loop {
        if let Ok((request, command)) = super::COMMANDS.try_receive() {
            let admission = pending.admit(&mut Commands(commands), request, command);
            if admission == oer_ieee80211_control::StationAdmission::Accepted {
                use oer_ieee80211_control::StationCommandKind as K;
                match command {
                    K::Connect => {
                        if !LINK.load(Relaxed) && READY.load(Relaxed) {
                            if !first {
                                attempt.advance();
                            }
                            first = false;
                        }
                        READY.store(false, Relaxed);
                        AUTOCONNECT.store(true, Relaxed);
                        RETRY.store(false, Relaxed);
                    }
                    K::Disconnect => {
                        AUTOCONNECT.store(false, Relaxed);
                        RETRY.store(false, Relaxed);
                    }
                    K::BackgroundScan => {}
                }
            }
            super::emit(request, &admission);
        }
        if let Ok(request) = super::STOPS.try_receive()
            && disconnect
        {
            disconnect = false;
            stopped.set(Some(request));
            observation::DEPARTING.store(true, Relaxed);
            RETRY.store(false, Relaxed);
            let _refused = commands.disconnect(0xffff).await;
        }
        if stopped.get().is_some() {
            // Station and NetDevice event producers run before this control
            // future in the same task; link loss needs no periodic sampler.
            core::future::poll_fn(|_| {
                if LINK.load(Relaxed) {
                    core::task::Poll::Pending
                } else {
                    core::task::Poll::Ready(())
                }
            })
            .await;
            commands.stop();
            core::future::pending::<()>().await;
        }
        if RETRY.load(Relaxed) && READY.swap(false, Relaxed) {
            if !first {
                attempt.advance();
            }
            first = false;
            RETRY.store(false, Relaxed);
            let _refused = commands.connect(1).await;
        }
        let _ = EmbassyClock.wait_for(Duration::from_secs(1)).await;
    }
}

fn publish_command(
    pending: &oer_ieee80211_control::PendingCommands,
    attempt: Attempt,
    request: oer_ieee80211_runtime::station::Request,
    result: oer_ieee80211_control::StationCommandResult,
) {
    let (identity, event) = match pending.finish(request, result) {
        Some(matched) => matched,
        None => (
            attempt.request,
            oer_ieee80211_control::StationCommandFinished {
                correlation: request.correlation(),
                origin: if request.correlation() < (1u64 << 32) {
                    oer_ieee80211_control::StationCommandOrigin::Policy
                } else {
                    oer_ieee80211_control::StationCommandOrigin::Unmatched
                },
                command: request.command().into(),
                result,
            },
        ),
    };
    super::emit(identity, &event);
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
    fn status(&self) -> oer_ieee80211_runtime::station::Status {
        self.0.status()
    }
}
