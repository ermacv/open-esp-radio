//! HIL commands, lifecycle observations and network workload over the facade.
use super::{Attempt, observation, radio};
use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_wifi as wifi;
use oer_ieee80211_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap};
use oer_ieee80211_control::{StationLinkState, StationServiceEnd};
use oer_ieee80211_rsn::nonce::SNonceSource;
use oer_memory::{OnceValue, Placed};
use oer_time::{Duration, Timer};
pub struct Callbacks(pub Placed<'static, SNonceSource>);
impl oer_ieee80211_sta::machine::StaCallbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0
            .next_nonce()
            .expect("SNonce stream exhausted; reset required")
    }
    fn he_packet_padding(&self, _: &[u8]) -> oer_ieee80211_upper_mac::protection::HePacketPadding {
        oer_ieee80211_upper_mac::protection::HePacketPadding::None
    }
}
static ATTEMPT: OnceValue<Attempt> = OnceValue::new();
pub static LINK: AtomicBool = AtomicBool::new(false);
static RETRY: AtomicBool = AtomicBool::new(true);
static READY: AtomicBool = AtomicBool::new(true);
pub struct ProductEvents;
impl wifi::Events for ProductEvents {
    fn event(&mut self, event: wifi::Event) {
        let attempt = ATTEMPT
            .get()
            .unwrap_or_else(|| radio::failure(wifi::InitializationError::Control));
        match event {
            wifi::Event::Connected => {
                LINK.store(true, Relaxed);
                attempt.publish(StationLinkState::Authorized);
            }
            wifi::Event::Disconnected(reason) => {
                LINK.store(false, Relaxed);
                RETRY.store(true, Relaxed);
                attempt.publish(StationLinkState::Disconnected {
                    reason: super::report::disconnect(reason),
                });
            }
            wifi::Event::CommandCompleted(_) => READY.store(true, Relaxed),
            wifi::Event::CommandRefused(_) => {
                READY.store(true, Relaxed);
                RETRY.store(true, Relaxed);
            }
            _ => {}
        }
    }
}
pub async fn serve(
    runner: wifi::StationRunner<radio::binding::Binding>,
    mut control: wifi::Control<'static>,
    net: wifi::Network<'static>,
    profile: wifi::Profile<radio::binding::Binding>,
    address: [u8; 6],
    seed: u64,
    attempt: Attempt,
) {
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
            let mut controls = core::pin::pin!(reconnect(control, attempt, &stopped));
            let mut workload = core::pin::pin!(embassy_futures::join::join(
                super::network::run(address, seed, net, &LINK, attempt),
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
    if outcome.kind == wifi::EndKind::Unavailable {
        attempt.publish(StationLinkState::Disconnected {
            reason: oer_ieee80211_control::StationDisconnectReason::RadioFailure,
        });
        super::end(attempt, StationServiceEnd::Unavailable);
    }
    if outcome.kind == wifi::EndKind::Stopped {
        if let Some(request) = stopped.get() {
            super::emit(
                request,
                &oer_ieee80211_control::StationStopped(observation::counters()),
            );
        }
    } else if let Some(fault) = outcome.fault {
        let fault = super::report::service(fault);
        super::end(
            attempt,
            match outcome.kind {
                wifi::EndKind::Failed => StationServiceEnd::Failed(fault),
                wifi::EndKind::Poisoned => StationServiceEnd::Poisoned(fault),
                _ => StationServiceEnd::Quarantined(fault),
            },
        );
    }
}
async fn reconnect(
    mut commands: wifi::Control<'static>,
    attempt: Attempt,
    stopped: &Cell<Option<oer_hil_protocol::RequestIdentity>>,
) -> ! {
    let mut first = true;
    let mut disconnect = true;
    loop {
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
