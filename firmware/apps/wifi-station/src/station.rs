//! The station facade's binding, its callbacks and the image's exit policy.
use crate::profile::{PROFILE, Security};
use oer_esp32s31_wifi as wifi;

/// The image keeps no trace observations.
#[derive(Clone, Copy, Default)]
pub struct Unobserved;
impl oer_trace::Observer for Unobserved {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, _: E) {}
}

/// The boot-seeded nonce stream the RSN handshake draws from.
pub struct Callbacks(pub &'static mut wifi::NonceSource);
impl wifi::Callbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0
            .next_nonce()
            .unwrap_or_else(|wifi::NonceExhausted| crate::fail(oer_diagnostics::name!("nonce-exhausted"; 32)))
    }
}

/// Product events have no consumer in this image: Control's operations and
/// the exit carry every result it acts on.
pub struct ProductEvents;
impl wifi::Events for ProductEvents {
    fn event(&mut self, _: wifi::Event) {}
}

/// The image's terminal policy for a handled construction error.
pub fn failure(error: wifi::InitializationError) -> ! {
    match error {
        wifi::InitializationError::Request(request) => {
            oer_esp32s31_compose::platform::reset_request(request)
        }
        wifi::InitializationError::Phy(fault) => fault.reset(),
        wifi::InitializationError::MembershipUnavailable => {
            crate::fail(oer_diagnostics::name!("station-membership"; 32))
        }
        _ => crate::fail(oer_diagnostics::name!("station-initialization"; 32)),
    }
}

wifi::bind_station! {pub(crate) mod binding{wifi:crate::WifiToken,radio:crate::RadioToken,callbacks:super::Callbacks,observer:super::Unobserved,driver_observer:super::Unobserved,sink:(),engine:oer_esp32s31_compose::crypto::KeyUnwrapEngine,events:super::ProductEvents,fail:super::failure}}

pub async fn drive(driver: wifi::Driver<binding::Binding>) {
    driver.run().await;
}

/// Prepare the profile's credentials, run the station and apply the exit
/// policy: an ordinary stop ends the task; every other exit records its
/// cause and resets.
pub async fn serve(
    runner: wifi::StationRunner<binding::Binding>,
    _control: wifi::Control<'static>,
    config: wifi::Config,
    address: [u8; 6],
    nonces: &'static mut wifi::NonceSource,
) {
    let settings = match PROFILE.security {
        Security::Open => wifi::StationSettings::open(PROFILE.ssid),
        Security::Wpa2Personal { passphrase } => {
            wifi::StationSettings::wpa2_passphrase(PROFILE.ssid, passphrase)
        }
    }
    .unwrap_or_else(|_| failure(wifi::InitializationError::Ssid));
    let credentials = settings
        .prepare()
        .await
        .unwrap_or_else(|_| failure(wifi::InitializationError::Ssid));
    let profile = wifi::Profile::new(
        &config,
        address,
        credentials,
        Callbacks(nonces),
        Unobserved,
        (),
    )
    .unwrap_or_else(|error| failure(error));
    match runner.run(profile).await {
        wifi::StationExit::Stopped => {}
        wifi::StationExit::Unavailable(wifi::UnavailableCause::Initialization(error)) => {
            failure(error)
        }
        wifi::StationExit::Unavailable(wifi::UnavailableCause::Abandoned) => {
            crate::fail(oer_diagnostics::name!("station-abandoned"; 32))
        }
        wifi::StationExit::Failed(_) => crate::fail(oer_diagnostics::name!("station-failed"; 32)),
        wifi::StationExit::Poisoned(_) => {
            crate::fail(oer_diagnostics::name!("station-poisoned"; 32))
        }
        wifi::StationExit::Quarantined(_) => {
            crate::fail(oer_diagnostics::name!("station-quarantined"; 32))
        }
        wifi::StationExit::RetirementFailed(_) => {
            crate::fail(oer_diagnostics::name!("station-retirement"; 32))
        }
        wifi::StationExit::ReleaseFailed(_) => {
            crate::fail(oer_diagnostics::name!("station-release"; 32))
        }
    }
}
