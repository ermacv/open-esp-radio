//! The panic-reset image: the HIL console serving one intentional panic,
//! which the platform's product panic entry records and resets on.
use oer_hil_agent::base::PanicRequest;
use oer_hil_protocol::system::InjectPanic;

use crate::transport::CONSOLE;

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
        spawner.spawn(super::panic::panic_on_request().expect("panic-reset task"));
        spawner.spawn(run(usb, boot).expect("system console task"));
    });
}

#[embassy_executor::task]
async fn run(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        0,
        |request, PanicRequest::Inject(InjectPanic)| super::panic::queue(request),
    )
    .await
}
