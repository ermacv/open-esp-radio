//! The panic-reset image: the HIL console serving one intentional panic,
//! which the platform's product panic entry records and resets on.
use oer_hil_agent::base::PanicRequest;
use oer_hil_protocol::system::InjectPanic;

use crate::transport::CONSOLE;

pub(crate) fn start(
    executor: &'static mut crate::Executor<0>,
    wake: crate::ExecutorWake,
    (usb, _pads): (
        crate::transport::Usb,
        esp_hal::usb::usb_serial_jtag::UsbPadControl<'static>,
    ),
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let mut entropy = oer_espressif_esp_hal_soc::entropy::Entropy::new(rng);
    let boot = u64::from_le_bytes(entropy.random_bytes().expect("HIL boot entropy")).max(1);
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
