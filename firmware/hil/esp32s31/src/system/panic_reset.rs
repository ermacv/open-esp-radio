//! The panic-reset image: the HIL console serving one intentional panic,
//! which the platform's product panic entry records and resets on.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_hil_agent::base::PanicRequest;
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_hil_protocol::system::{InjectPanic, PanicInjected};

use crate::transport::CONSOLE;

/// The panic request the console handed over.
static PANICS: Channel<CriticalSectionRawMutex, RequestIdentity, 1> = Channel::new();

pub(crate) fn start(
    executor: &'static mut crate::Executor<0>,
    wake: crate::ExecutorWake,
    usb: crate::transport::Usb,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let entropy = esp_hal::rng::TrngSource::new(rng);
    let rng = esp_hal::rng::Trng::try_new().expect("HIL boot entropy");
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    drop(rng);
    drop(entropy);
    CONSOLE.start(boot);
    crate::transport::init_logger();
    executor.run(wake, |spawner| {
        spawner.spawn(panic_on_request().expect("panic-reset task"));
        spawner.spawn(run(usb, boot).expect("system console task"));
    });
}

#[embassy_executor::task]
async fn run(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        0,
        |request, PanicRequest::Inject(InjectPanic)| {
            if PANICS.try_send(request).is_err() {
                CONSOLE.publish(
                    request.session_id,
                    request.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        },
    )
    .await
}

/// Acknowledges the request, then panics in thread context: the location
/// is this file, which the host checks in the next boot's evidence.
#[embassy_executor::task]
async fn panic_on_request() {
    let request = PANICS.receive().await;
    let sequence = CONSOLE
        .publish_reliably(request.session_id, request.request_id, &PanicInjected)
        .await;
    // The host must see the acknowledgement before the reset.
    CONSOLE.written(sequence).await;
    panic!("intentional HIL panic");
}
