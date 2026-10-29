//! Image collectors and spawning over the public station facade.
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use oer_esp32s31_wifi as wifi;
use oer_ieee80211_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap};
pub static AUTHENTICATED: AtomicBool = AtomicBool::new(false);
pub static LAST_BEACON_MS: AtomicU32 = AtomicU32::new(0);
pub static DEPARTING: AtomicBool = AtomicBool::new(false);
pub static DEAUTH: AtomicBool = AtomicBool::new(false);
static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static COLLISIONS: AtomicU32 = AtomicU32::new(0);
static LINK: AtomicBool = AtomicBool::new(false);
static DISCONNECTED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
static SERVICE_TALLY: oer_firmware_apps_w3::collector::ServiceTally =
    oer_firmware_apps_w3::collector::ServiceTally::new();
#[derive(Clone, Copy, Default)]
pub struct Observed;
impl oer_trace::Observer for Observed {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        oer_trace::Observer::event(&SERVICE_TALLY, event);
        use wifi::observation::Observation;
        match Observation::of(&event) {
            Some(Observation::Authenticated) => AUTHENTICATED.store(true, Relaxed),
            Some(Observation::Beacon(t)) => LAST_BEACON_MS.store(t, Relaxed),
            Some(Observation::Indeterminate) => {
                TIMEOUTS.fetch_add(1, Relaxed);
            }
            Some(Observation::QueueCollision) => {
                COLLISIONS.fetch_add(1, Relaxed);
            }
            Some(Observation::ControlDelivered) if DEPARTING.load(Relaxed) => {
                DEAUTH.store(true, Relaxed)
            }
            _ => {}
        }
    }
}
pub struct ProductEvents;
impl wifi::Events for ProductEvents {
    fn event(&mut self, event: wifi::Event) {
        match event {
            wifi::Event::Connected => LINK.store(true, Relaxed),
            wifi::Event::Disconnected(_) => {
                LINK.store(false, Relaxed);
                DISCONNECTED.store(true, Relaxed);
            }
            wifi::Event::CommandRefused(_)
            | wifi::Event::CommandInFlight(_)
            | wifi::Event::TxRefused => FAILED.store(true, Relaxed),
            _ => {}
        }
    }
}
pub fn failure(error: wifi::InitializationError) -> ! {
    match error {
        wifi::InitializationError::MembershipUnavailable => {
            crate::fail(oer_diagnostics::name!("station-membership"; 32))
        }
        wifi::InitializationError::Request(request) => {
            oer_espressif_staged_runtime::panic::reset_request::<oer_esp32s31_hal::platform::Port, _>(
                request,
            )
        }
        wifi::InitializationError::Phy(fault) => oer_esp32s31_compose::phy_fault::reset(fault),
        _ => crate::fail(oer_diagnostics::name!("station-initialization";32)),
    }
}

wifi::bind_station! {pub mod binding{wifi:crate::WifiToken,radio:crate::RadioToken,callbacks:wifi::OpenCallbacks,observer:super::Observed,driver_observer:super::Observed,engine:SoftwareKeyUnwrap,events:super::ProductEvents,fail:super::failure}}
pub async fn drive(driver: wifi::Driver<binding::Binding>) {
    driver.run().await;
}
pub async fn serve(
    runner: wifi::StationRunner<binding::Binding>,
    mut control: wifi::Control<'static>,
    net: wifi::Network<'static>,
    config: crate::W3Config,
    address: [u8; 6],
    seed: u64,
) {
    let setup = wifi::Config::new(
        config.channel,
        config.country,
        config.cap_dbm,
        Some(config.stand),
        false,
    )
    .unwrap_or_else(|error| failure(error));
    let profile = wifi::Profile::open(&setup, address, config.ssid, wifi::OpenCallbacks, Observed)
        .unwrap_or_else(|error| failure(error));
    let mut running = core::pin::pin!(runner.run(profile, SoftwareKeyUnwrap::new(SoftwareAes128)));
    let mut workload = core::pin::pin!(async {
        match control.wait_started().await {
            Ok(()) => {
                crate::network::workload(
                    config,
                    address,
                    seed,
                    control,
                    net,
                    &LINK,
                    &DISCONNECTED,
                    &FAILED,
                )
                .await
            }
            Err(_) => {
                oer_firmware_apps_w3::report::Report::new(config.channel, address, config.stand)
            }
        }
    });
    let (_, mut report) = embassy_futures::join::join(
        crate::future::Boundary(running.as_mut()),
        crate::future::Boundary(workload.as_mut()),
    )
    .await;
    SERVICE_TALLY.report(&mut report);
    report.tx_timeout = TIMEOUTS.load(Relaxed);
    report.queue_collision = COLLISIONS.load(Relaxed);
    report.irq_free =
        oer_espressif_staged_runtime::stacks::current_hart_interrupt_stack_free_bytes();
    crate::print_report(report);
}
