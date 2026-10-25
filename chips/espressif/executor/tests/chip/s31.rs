#![no_std]
use esp_hal::{
    interrupt::{InterruptRoutes, software::SoftwareInterrupt},
    peripherals::{LP_AON_CLK_RST, LP_PERI, LP_WDT, PSRAM, RNG, SYSTIMER, TIMG0, TIMG1, USB_DEVICE},
};
use oer_espressif_watchdog::{
    MainWatchdog, MainWatchdogRegisters,
    deadline::{DeadlineBudget, DeadlineWatchdog},
};
use oer_espressif_esp_hal_soc::handoff::{
    take_entropy, take_reset_causes, take_usb_serial_jtag, take_watchdogs,
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
pub fn entropy(rng: RNG<'static>, lp_peri: LP_PERI<'static>, clock: impl oer_time::Clock) {
    let owner = take_entropy(rng, lp_peri);
    let mut entropy = oer_esp32s31_hal::platform::Entropy::new(owner, clock);
    let _health = entropy.status();
    let _word = entropy.random();
    #[cfg(ui_reject)]
    let _second = take_entropy(
        rng,     // ui-error: E0382
        lp_peri, // ui-error: E0382
    );
}
pub fn reset_causes(lp_aon: LP_AON_CLK_RST<'static>) {
    let _owner = take_reset_causes(lp_aon);
    #[cfg(ui_reject)]
    let _second = take_reset_causes(lp_aon); // ui-error: E0382
}
pub fn usb(usb: USB_DEVICE<'static>) {
    let blocks = take_usb_serial_jtag(usb).expect("no early record holds the block");
    let (_endpoint, _interrupt, pads) = oer_esp32s31_hal::usb_serial_jtag::console_parts(blocks);
    pads.disconnect();
    #[cfg(ui_reject)]
    pads.disconnect(); // ui-error: E0382
    #[cfg(ui_reject)]
    let _second = take_usb_serial_jtag(usb); // ui-error: E0382
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
pub fn watchdog<R: MainWatchdogRegisters, C: oer_time::Clock>(
    owner: DeadlineWatchdog<R, C>,
    budget: DeadlineBudget,
) {
    let _lease = owner.arm(budget);
    #[cfg(ui_reject)]
    let _second = owner.arm(budget); // ui-error: E0382
}
pub fn watchdogs(
    timg0: TIMG0<'static>,
    timg1: TIMG1<'static>,
    lp_wdt: LP_WDT<'static>,
) {
    let blocks = take_watchdogs(timg0, timg1, lp_wdt);
    let mut watchdogs = oer_esp32s31_hal::watchdog::Watchdogs::new(blocks);
    watchdogs.main0.disable();
    #[cfg(ui_reject)]
    let _second = take_watchdogs(
        timg0,  // ui-error: E0382
        timg1,  // ui-error: E0382
        lp_wdt, // ui-error: E0382
    );
}
pub fn locked_watchdog<R: MainWatchdogRegisters>(mut main: MainWatchdog<R>) {
    main.with_unlocked(|session| session.feed());
    #[cfg(ui_reject)]
    main.feed(); // ui-error: E0599
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
    /// A start that names its future's maker: it holds nothing.
    type Make = fn() -> Idle;
    /// A start holding a value that stays in its context: a reference to a
    /// maker that is not `Sync`.
    type Held = &'static dyn Fn() -> Idle;
    static RADIO_TASK: Task<Make, Radio> = Task::new(0);
    static APP_TASK: Task<Make, App> = Task::new(0);
    static RADIO_HELD: Task<Held, Radio> = Task::new(0);
    static APP_HELD: Task<Held, App> = Task::new(0);
    fn send<S: Send>(_: &S) {}

    pub fn own(radio: &Spawner<Radio>, app: &Spawner<App>) -> Result<(), AlreadyStarted> {
        send(radio);
        app.start(&APP_TASK, || Idle)?;
        radio.start(&RADIO_TASK, || Idle)
    }
    pub fn another_executors_task(radio: &Spawner<Radio>) {
        #[cfg(ui_reject)]
        let _ = radio.start(&APP_TASK, || Idle); // ui-error: E0308
    }
    /// The executor makes the future from the start in its own context, so
    /// only the start crosses to an interrupt-mode executor: it is `Send`.
    pub fn radio_starts_are_send(radio: &Spawner<Radio>, app: &Spawner<App>, held: Held) {
        let _ = app.start(&APP_HELD, held);
        #[cfg(ui_reject)]
        let _ = radio.start(&RADIO_HELD, held); // ui-error: E0277
    }
    pub fn the_application_spawner_stays_in_its_context(app: &Spawner<App>) {
        #[cfg(ui_reject)]
        send(app); // ui-error: E0277
    }
}
