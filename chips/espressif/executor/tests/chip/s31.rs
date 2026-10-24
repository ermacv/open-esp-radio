#![no_std]
use esp_hal::{
    interrupt::{InterruptRoutes, software::SoftwareInterrupt},
    peripherals::{PSRAM, RNG, SYSTIMER},
    usb::usb_serial_jtag::UsbPadControl,
};
use oer_espressif_esp_hal_soc::{
    entropy::Entropy,
    watchdog::{DeadlineBudget, DeadlineWatchdog},
};
use oer_espressif_executor::{DoorbellAcknowledger, Executor};
use oer_espressif_executor::systimer::{Comparator, ComparatorWriter};

pub fn executor(line: SoftwareInterrupt<'static, 2>) {
    let _first = Executor::<2>::new(line);
    #[cfg(ui_reject)]
    let _second = Executor::<2>::new(line); // ui-error: E0382
}
pub fn doorbell(mut owner: DoorbellAcknowledger<3>) {
    #[cfg(ui_reject)]
    let _moved = owner;
    owner.acknowledge(); // ui-error: E0382
}
pub fn no_free_acknowledgement() {
    #[cfg(ui_reject)]
    oer_espressif_executor::acknowledge::<3>(); // ui-error: E0425
}
pub fn timer(timer: SYSTIMER<'static>) {
    let (mut bank, mut zero, mut one, mut two) = ComparatorWriter::adopt(timer);
    bank.bind(&mut zero);
    bank.bind(&mut one);
    bank.bind(&mut two);
    bank.program(Comparator::Zero, Some(100));
    zero.acknowledge();
    #[cfg(ui_reject)]
    let _second = ComparatorWriter::adopt(timer); // ui-error: E0382
}
pub fn entropy(rng: RNG<'static>) {
    let mut owner = Entropy::new(rng);
    let _health = owner.source_status();
    let _bytes = owner.random_bytes::<8>();
    #[cfg(ui_reject)]
    let _second = Entropy::new(rng); // ui-error: E0382
}
pub fn usb(pads: UsbPadControl<'static>) {
    pads.disconnect();
    #[cfg(ui_reject)]
    pads.disconnect(); // ui-error: E0382
}
pub fn psram(mut psram: PSRAM<'static>) {
    let _supply = esp_hal::psram::existing_supply_configuration(&mut psram);
    #[cfg(ui_reject)]
    let _unwitnessed = esp_hal::psram::existing_supply_configuration(); // ui-error: E0061
}
pub fn matrix(routes: InterruptRoutes) {
    let _banks = routes.split();
    #[cfg(ui_reject)]
    let _second = routes.split(); // ui-error: E0382
}
pub fn no_free_matrix() {
    #[cfg(ui_reject)]
    let _matrix = oer_espressif_esp_hal_irq::EspHalMatrix; // ui-error: E0423
}
pub fn watchdog(owner: DeadlineWatchdog, budget: DeadlineBudget) {
    let _lease = owner.arm(budget);
    #[cfg(ui_reject)]
    let _second = owner.arm(budget); // ui-error: E0382
}

/// Declared tasks are bound to their executor by type.
pub mod tasks {
    use core::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    };
    use oer_espressif_executor::task::{
        AlreadyStarted, Interrupt, PollToken, Spawner, Task, Tasks, Thread,
    };
    pub struct Idle;
    impl Future for Idle {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }
    pub struct Radio;
    impl Tasks for Radio {
        type Mode = Interrupt;
        const EXECUTOR: u8 = 2;
        const COUNT: u8 = 1;
        fn poll(_: u8, token: &PollToken<Self>, cx: &mut Context<'_>) {
            RADIO_TASK.poll(token, cx);
        }
    }
    pub struct App;
    impl Tasks for App {
        type Mode = Thread;
        const EXECUTOR: u8 = 0;
        const COUNT: u8 = 1;
        fn poll(_: u8, token: &PollToken<Self>, cx: &mut Context<'_>) {
            APP_TASK.poll(token, cx);
        }
    }
    static RADIO_TASK: Task<Idle, Radio> = Task::new(0);
    static APP_TASK: Task<Idle, App> = Task::new(0);
    fn send<S: Send>(_: &S) {}

    pub fn own(radio: &Spawner<Radio>, app: &Spawner<App>) -> Result<(), AlreadyStarted> {
        send(radio);
        let local: *const u8 = core::ptr::null();
        app.start(&APP_TASK, move || {
            let _held = local;
            Idle
        })?;
        radio.start(&RADIO_TASK, || Idle)
    }
    pub fn another_executors_task(radio: &Spawner<Radio>) {
        #[cfg(ui_reject)]
        let _ = radio.start(&APP_TASK, || Idle); // ui-error: E0308
    }
    pub fn radio_arguments_are_send(radio: &Spawner<Radio>) {
        let local: *const u8 = core::ptr::null();
        #[cfg(ui_reject)]
        let _ = radio.start(&RADIO_TASK, move || { let _held = local; Idle }); // ui-error: E0277
    }
    pub fn the_application_spawner_stays_in_its_context(app: &Spawner<App>) {
        #[cfg(ui_reject)]
        send(app); // ui-error: E0277
    }
}
