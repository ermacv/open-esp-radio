//! The panic-reset image: the HIL console serving one intentional panic,
//! which the platform's product panic entry records and resets on.
use oer_hil_agent::base::PanicRequest;
use oer_hil_protocol::system::InjectPanic;

use crate::transport::CONSOLE;

oer_espressif_executor::tasks! {
    /// The panic-reset image's tasks.
    PanicResetTasks: Executor<0> {
        panic_on_request() = super::panic::panic_on_request;
        run(usb: crate::transport::Usb, boot: u64) = run;
    }
}

pub(crate) fn start(
    executor: crate::Executor<0>,
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
    executor.run(wake, |spawner| {
        PanicResetTasks::panic_on_request(spawner).expect("panic-reset task");
        PanicResetTasks::run(spawner, usb, boot).expect("system console task");
    });
}

async fn run(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        0,
        |request, PanicRequest::Inject(InjectPanic)| super::panic::queue(request),
    )
    .await
}
