#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::let_underscore_must_use,
        reason = "the pump, the entry and the images"
    )
)]
#![cfg(target_arch = "riscv32")]
#![no_std]
#![feature(type_alias_impl_trait)]
#![forbid(unsafe_code)]
//! The ESP32-S31 implementation: the default interrupt layout, executors,
//! service tasks and boot sequence behind the application entry's `entry!`
//! (`oer`), which re-exports this crate for an image built for the ESP32-S31.
//!
//! The layout, fixed for every image, comes from `[interrupt-layout]` in the
//! chip facts: each entry's line, its context's level and its hart.
//!
//! The application executor runs in thread mode on hart 0; the library's
//! service tasks ([`SdkTasks`]) and the image's tasks share it. An entry no
//! driver binds routes no source.

use oer_chips_esp32s31_hal::{
    peripherals::{
        aes::AesParts,
        entropy::EntropyRegisters,
        software_interrupt::{FromCpu0Irq, SoftwareInterrupts},
        system_timer::{SystemTimer, SystimerAlarm1},
        watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    },
    system::platform::{InterruptController, Port},
    system::root::Root,
};
use oer_chips_family_platform_executor::{Executor, Spawner};
use oer_chips_family_platform_runtime::{Adopted, Handoff};
use oer_portable_embassy_time::EmbassyClock;

// The family's shared parts keep their paths in this crate.
use oer_chips_family_app::{boot, channel};

pub use oer_chips_esp32s31_platform_runtime::Esp32s31;

/// Time on the executors' clock: timers, timeouts, instants.
pub use embassy_time as time;

/// `$future` itself.
macro_rules! observed {
    ($part:ident, $future:expr) => {
        $future
    };
}

/// Nothing.
macro_rules! observed_root {
    ($root:ident, $part:ident) => {};
}

pub mod cpu;
/// The observer of executor `SLOT`: none without `observe`.
#[cfg_attr(
    not(oer_trace),
    allow(
        dead_code,
        reason = "only a trace build asks a task set for its observer"
    )
)]
const fn executor_observer<const SLOT: usize>() -> oer_portable_trace::Noop {
    oer_portable_trace::Noop
}

mod aes;
pub use aes::{AesEngine, AesStop};

#[cfg(feature = "console")]
mod console;
#[cfg(feature = "console")]
pub use console::{Console, ConsolePeripheral, UsbConsole};

/// Why a hart left reset.
pub use oer_chips_esp32s31_hal::peripherals::reset_cause::ResetCause;
/// What the interrupt controller showed after the previous boot's
/// exception trapped.
pub use oer_chips_esp32s31_platform_runtime::exception::{
    LineView, Observed, Terminal, TrapIdentity,
};
/// The record of the previous boot's panic.
pub use oer_chips_family_platform_runtime::panic::PanicRecord;
/// The previous boot's exception as the trap left it.
pub use oer_chips_family_platform_runtime::trap::PreviousException;

include!(concat!(env!("OUT_DIR"), "/interrupt_table.rs"));

#[cfg(feature = "group")]
mod hart1;
#[cfg(feature = "group")]
#[doc(hidden)]
pub use hart1::{Hart1Parts, hart1_main};

oer_chips_family_platform_executor::tasks! {
    /// The library's service tasks on the application executor; an image's
    /// set extends it.
    pub SdkTasks: Executor<Esp32s31, 0> {
        @observer: executor_observer::<0>();
        /// Feeds the RTC watchdog while the executor polls.
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

oer_chips_family_platform_executor::time_driver!(Esp32s31);

/// The peripherals an application may take, each once.
pub struct Peripherals {
    /// The AES accelerator: [`AesPeripheral::engine`] makes it the image's
    /// synchronous AES engine.
    pub aes: AesPeripheral,
    /// The USB Serial/JTAG console: [`Console::open`] takes it.
    #[cfg(feature = "console")]
    pub console: ConsolePeripheral,
    /// The boot hart's cycle, instruction and cache counters ([`cpu::Cpu0`]).
    pub cpu: cpu::Cpu0,
}

/// The AES accelerator's owner.
pub struct AesPeripheral(pub(crate) AesParts);

/// What the application's boot builds drivers and services with: the
/// interrupt router before interrupts are enabled, and the library's
/// service tasks to start once the executor runs. It exists only while the
/// boot function runs, before any executor.
pub struct Boot {
    adopted: Adopted<InterruptController, InterruptTable>,
    /// The system timer, until a driver installs it or the boot ends.
    timer: Option<SystemTimer>,
    app_alarm: Option<AppAlarmToken>,
    /// The entropy source, until the boot ends.
    pub(crate) entropy: Option<EntropyRegisters>,
    /// The boot's identity, once drawn.
    pub(crate) boot_id: Option<u64>,
    /// The settle counter at the adopted CPU rate, for each driver.
    pub(crate) cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
    evidence: Evidence,
    /// The time driver's authority and the comparator for hart 1's
    /// executor, once the time driver is installed.
    #[cfg(feature = "group")]
    pub(crate) timers: Option<(
        oer_chips_family_platform_executor::TimeDriverReady<Esp32s31>,
        oer_chips_esp32s31_hal::peripherals::system_timer::SystimerAlarm2,
    )>,
}

/// No network: the group started none.
pub struct NoNet;

impl Boot {
    pub(crate) fn router(
        &mut self,
    ) -> &mut oer_chips_family_platform_interrupt_table::Router<
        InterruptController,
        0,
        InterruptTable,
    > {
        self.adopted.router()
    }

    /// Why the chip left reset and the previous boot's panic, as the
    /// library read and reported them before the boot.
    pub fn evidence(&self) -> Evidence {
        self.evidence
    }
}

/// What the library read before the boot.
#[derive(Clone, Copy, Debug)]
pub struct Evidence {
    /// Why hart 0 left reset.
    pub reset_cause: ResetCause,
    /// The previous boot's panic, already printed and cleared.
    pub previous_panic: Option<PanicRecord>,
    /// The previous boot's CPU exception, already printed and cleared.
    pub previous_exception: Option<ExceptionEvidence>,
}

/// The previous boot's CPU exception: the registers the runtime retained at
/// the trap, before it stopped the other hart, and what the library's
/// exception hook read after that stop. The second is no snapshot of the
/// trap's instant: a source may have become pending in between. It is
/// `None` when no record of the hook follows this trap: the hook did not
/// commit one, the boot printed and cleared it, or the record left is of an
/// earlier terminal, even one with the same registers (`Observed::follows`).
#[derive(Clone, Copy, Debug)]
pub struct ExceptionEvidence {
    pub trap: PreviousException,
    pub observed: Option<Observed>,
}

static BOOT_ID: crate::boot::BootId = crate::boot::BootId::new();

/// This boot's identity: a nonzero random value the library draws from the
/// entropy source once, before the executor starts, so every task reads
/// the same value.
pub fn boot_id() -> u64 {
    BOOT_ID.get()
}

/// A nonzero boot identity from `rng`.
pub(crate) fn draw_boot_id(
    rng: &mut oer_chips_esp32s31_platform_runtime::platform::Entropy,
) -> Result<u64, oer_chips_esp32s31_hal::peripherals::entropy::EntropyError> {
    let mut bytes = [0; 8];
    rng.fill(&mut bytes)?;
    Ok(crate::boot::identity(bytes))
}

impl Boot {
    /// End the boot: run the future `app` builds on hart 0 beside the
    /// library's service tasks once the executor starts. The library calls
    /// `app` where the future stays, so the future is never moved.
    pub fn run<C, F>(self, app: C) -> Run<C>
    where
        C: FnOnce() -> F,
        F: core::future::Future<Output = AppResult>,
    {
        Run { boot: self, app }
    }
}

/// A finished boot and the application it runs: what an image's boot
/// returns, as `impl Started`.
pub trait Started: started::Sealed {
    /// The application's future.
    type App: core::future::Future<Output = AppResult>;
    /// What builds it, where it stays.
    type Build: FnOnce() -> Self::App;
    #[doc(hidden)]
    fn into_parts(self) -> (Boot, Self::Build);
}

mod started {
    pub trait Sealed {}
}

/// The finished boot [`Boot::run`] returns.
pub struct Run<C> {
    boot: Boot,
    app: C,
}

impl<C> started::Sealed for Run<C> {}

impl<C, F> Started for Run<C>
where
    C: FnOnce() -> F,
    F: core::future::Future<Output = AppResult>,
{
    type App = F;
    type Build = C;
    fn into_parts(self) -> (Boot, C) {
        (self.boot, self.app)
    }
}

/// What the group owns: with `group`, hart 1's counters.
pub struct GroupPeripherals {
    /// Hart 1's cycle and instruction counters ([`cpu::Cpu1`]).
    #[cfg(feature = "group")]
    pub cpu: cpu::Cpu1,
}

/// The group's context: `entry!(boot, group = boot1)` calls `boot1`
/// with it once the boot ended, before any executor, and runs the future it
/// ends with. Both run where the second
/// boot runs: on hart 0, the application's executor, or with the `group`
/// feature on hart 1's own executor, which hart 1 runs alone.
///
/// `N` is the networks the group started: [`NoNet`], since no network
/// role is built.
pub struct GroupBoot<N = NoNet> {
    net: N,
}

impl GroupBoot {
    /// The group's context before its networks.
    pub(crate) const fn new() -> Self {
        Self { net: NoNet }
    }
}

impl<N: NetTask> GroupBoot<N> {
    /// End the group: run the future `future` builds beside its IP
    /// service. The library calls `future` where the future stays, so the
    /// future is never moved.
    pub fn run<C, F>(self, future: C) -> GroupRun<C, N>
    where
        C: FnOnce() -> F,
        F: core::future::Future<Output = AppResult>,
    {
        GroupRun {
            net: self.net,
            future,
        }
    }
}

/// A finished group: what `boot1` returns, as `impl GroupStarted`.
pub trait GroupStarted: started::Sealed {
    /// The group's future.
    type Future: core::future::Future<Output = AppResult>;
    /// What builds it, where it stays.
    type Build: FnOnce() -> Self::Future;
    /// The networks it started, or [`NoNet`].
    type Net: NetTask;
    #[doc(hidden)]
    fn into_parts(self) -> (Self::Net, Self::Build);
}

/// The finished group [`GroupBoot::run`] returns.
pub struct GroupRun<C, N> {
    net: N,
    future: C,
}

impl<C, N> started::Sealed for GroupRun<C, N> {}

impl<C, F, N> GroupStarted for GroupRun<C, N>
where
    C: FnOnce() -> F,
    F: core::future::Future<Output = AppResult>,
    N: NetTask,
{
    type Future = F;
    type Build = C;
    type Net = N;
    fn into_parts(self) -> (N, C) {
        (self.net, self.future)
    }
}

pub use crate::channel::ChannelError;

/// A channel between the application's future and the group's
/// (`oer_chips_family_app::channel`), its ends waking through [`AppNotify`] and
/// [`GroupNotify`].
pub struct Channel<A: 'static, B: 'static, const N: usize>(
    crate::channel::Channel<A, B, N, AppNotify, GroupNotify>,
);
/// The application's end of a [`Channel`]: it sends `A` and receives `B`.
pub type ApplicationEnd<A, B, const N: usize> =
    crate::channel::End<A, B, N, AppNotify, GroupNotify>;
/// The group's end of a [`Channel`]: it sends `B` and receives `A`.
pub type GroupEnd<A, B, const N: usize> = crate::channel::End<B, A, N, GroupNotify, AppNotify>;

/// How a wait of the application's future is told: on one hart its waker,
/// with the group on hart 1 the application's executor signal, which
/// hart 1 raises.
#[cfg(not(feature = "group"))]
pub type AppNotify = oer_portable_waker::AtomicWaker;
/// How a wait of the group's future is told, likewise.
#[cfg(not(feature = "group"))]
pub type GroupNotify = oer_portable_waker::AtomicWaker;
#[cfg(feature = "group")]
pub type AppNotify =
    &'static oer_chips_family_platform_interrupt_table::signal::PeerNotify<Executor<Esp32s31, 0>>;
#[cfg(feature = "group")]
pub type GroupNotify =
    &'static oer_chips_family_platform_interrupt_table::signal::PeerNotify<Executor<Esp32s31, 1>>;

impl<A, B, const N: usize> Channel<A, B, N> {
    /// A channel between the application's future and the group's.
    #[cfg(not(feature = "group"))]
    pub const fn new() -> Self {
        Self(crate::channel::Channel::new())
    }

    /// A channel between hart 0's application and hart 1's group.
    #[cfg(feature = "group")]
    pub const fn new() -> Self {
        Self(crate::channel::Channel::across_harts(
            &hart1::APPLICATION,
            &hart1::SECOND,
        ))
    }
}

impl<A, B, const N: usize> Default for Channel<A, B, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Send, B: Send, const N: usize> Channel<A, B, N> {
    /// The application's end, taken once in the boot.
    ///
    /// # Errors
    /// [`Error::Channel`]: the end was taken already.
    pub fn application(&'static self, boot: &mut Boot) -> Result<ApplicationEnd<A, B, N>, Error> {
        let _ = boot;
        self.0.take_application().map_err(Error::Channel)
    }

    /// The group's end, taken once in the group after the
    /// boot took the application's.
    ///
    /// # Errors
    /// [`Error::Channel`]: the end was taken already, or the boot took no
    /// application end.
    pub fn group<M>(&'static self, boot: &mut GroupBoot<M>) -> Result<GroupEnd<A, B, N>, Error> {
        let _ = boot;
        self.0.take_second().map_err(Error::Channel)
    }
}

/// The group of an image that declares none: it drops what the boot
/// handed it and runs nothing.
#[doc(hidden)]
pub fn no_group(_handed: GroupPeripherals, boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    Ok(boot.run(core::future::pending::<AppResult>))
}

/// A network the executor runs beside the application, with the
/// image's pool sizes; not an API.
#[doc(hidden)]
pub trait NetTask: 'static {
    /// The service, for good.
    fn serve(self) -> impl core::future::Future<Output = ()>;
}

impl NetTask for NoNet {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        // Ready at every poll: `core::future::ready` would panic at a
        // second one.
        core::future::poll_fn(|_| core::task::Poll::Ready(()))
    }
}

/// Two networks of the group, served together in one task: the group's
/// network signal wakes it for either.
impl<A: NetTask, B: NetTask> NetTask for (A, B) {
    async fn serve(self) {
        let first = core::pin::pin!(self.0.serve());
        let second = core::pin::pin!(self.1.serve());
        both(first, second).await;
    }
}

/// Poll both futures until both ended, each never again after its end:
/// no output is held, so none is taken twice.
async fn both(
    mut first: core::pin::Pin<&mut impl core::future::Future<Output = ()>>,
    mut second: core::pin::Pin<&mut impl core::future::Future<Output = ()>>,
) {
    let (mut first_ended, mut second_ended) = (false, false);
    core::future::poll_fn(|cx| {
        if !first_ended {
            first_ended = first.as_mut().poll(cx).is_ready();
        }
        if !second_ended {
            second_ended = second.as_mut().poll(cx).is_ready();
        }
        if first_ended && second_ended {
            core::task::Poll::Ready(())
        } else {
            core::task::Poll::Pending
        }
    })
    .await;
}

/// Run the networks, or end at once without one.
async fn run_net(net: impl NetTask) {
    net.serve().await;
}

/// Why the boot or the application stopped; the library records it and
/// resets the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A peripheral's service was started twice.
    AlreadyStarted,
    /// The interrupt table's entry was taken.
    InterruptTaken,
    /// A peripheral's interrupt could not be held silent.
    InterruptRouted,
    /// A static storage was taken already.
    Storage,
    /// The entropy source failed its health check.
    Entropy,
    /// A channel's end was refused.
    Channel(ChannelError),
    /// The console could not take the USB Serial/JTAG block.
    #[cfg(feature = "console")]
    Console,
}

impl Error {
    /// The error's name in the retained record.
    pub const fn name(self) -> oer_portable_diagnostics::Name<32> {
        match self {
            Self::AlreadyStarted => oer_portable_diagnostics::name!("sdk-already-started"; 32),
            Self::InterruptTaken => oer_portable_diagnostics::name!("sdk-interrupt-taken"; 32),
            Self::InterruptRouted => oer_portable_diagnostics::name!("sdk-interrupt-routed"; 32),
            Self::Storage => oer_portable_diagnostics::name!("sdk-storage"; 32),
            Self::Entropy => oer_portable_diagnostics::name!("sdk-entropy"; 32),
            Self::Channel(_) => oer_portable_diagnostics::name!("sdk-channel"; 32),
            #[cfg(feature = "console")]
            Self::Console => oer_portable_diagnostics::name!("sdk-console"; 32),
        }
    }
}

/// What an application's future ends with: never, or why it stopped.
pub type AppResult = Result<core::convert::Infallible, Error>;

/// Run the image: adopt the runtime, report the previous boot's faults,
/// call `boot` with the peripherals, then the group's setup with what the boot handed
/// it, and start the executor with the library's service tasks, the second
/// boot's networks and future, and the application's future. Any error
/// records and resets.
#[doc(hidden)]
pub fn start<S: Started, H: GroupStarted>(
    handoff: Handoff<Port>,
    boot: fn(Peripherals, Boot) -> Result<S, Error>,
    hart1: fn(GroupPeripherals, GroupBoot) -> Result<H, Error>,
    #[cfg_attr(not(feature = "group"), allow(unused_variables))] mailbox: GroupMailbox,
) -> ! {
    let previous = oer_chips_family_platform_runtime::panic::peek_previous();
    // Read before adoption prints and clears them.
    let previous_exception = oer_chips_family_platform_runtime::trap::peek_previous_exception()
        .map(|trap| {
            ExceptionEvidence {
                trap,
                // Only the observations of this trap: an earlier trap's
                // record may outlive it.
                observed: oer_chips_esp32s31_platform_runtime::exception::peek_previous()
                    .filter(|observed| observed.follows(&trap)),
            }
        });
    let Some(root) = Root::take() else {
        fail(oer_portable_diagnostics::name!("root-taken"; 32));
    };
    let (hardware, _psram, adopted) =
        match oer_chips_family_platform_runtime::adoption::run::<Esp32s31, _, _, _>(
            handoff,
            root.into_runtime(),
            InterruptTable,
            oer_chips_esp32s31_platform_board::BootstrapPsram,
            || {},
            |hardware| {
                let causes =
                    oer_chips_esp32s31_platform_runtime::reset_causes::read(hardware.reset_cause);
                oer_chips_esp32s31_platform_runtime::wifi_fault::report_previous();
                oer_chips_esp32s31_platform_runtime::radio_fault::report_previous();
                oer_chips_esp32s31_platform_runtime::exception::report_previous();
                if let Some(record) = previous {
                    let mut console =
                        oer_chips_family_platform_runtime::diagnostics::Console::shared(whole);
                    let _ = oer_chips_family_platform_runtime::diagnostics::previous_panic(
                        &mut console,
                        &record,
                    );
                }
                Owned {
                    reset_cause: causes.of(oer_chips_esp32s31_hal::system::platform::BOOT_HART),
                    entropy: hardware.entropy,
                    aes: hardware.aes,
                    system_timer: hardware.system_timer,
                    software: hardware.software_interrupts,
                    watchdog: hardware.watchdogs.rtc,
                    #[cfg(feature = "group")]
                    hart1: hardware.hart1,
                    #[cfg(feature = "group")]
                    causes,
                    cache_counters: hardware.cache_counters,
                    cycles: hardware.cycles,
                }
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    oer_chips_esp32s31_platform_runtime::exception::report_stack_guard();
    let Some(interrupts) = Interrupts::take() else {
        fail(Error::InterruptTaken.name());
    };
    let software: SoftwareInterrupts = hardware.software;
    let setup = Boot {
        adopted,
        timer: Some(hardware.system_timer),
        app_alarm: Some(interrupts.alarm),
        entropy: Some(hardware.entropy),
        boot_id: None,
        cycles: hardware.cycles,
        evidence: Evidence {
            reset_cause: hardware.reset_cause,
            previous_panic: previous,
            previous_exception,
        },
        #[cfg(feature = "group")]
        timers: None,
    };
    let peripherals = Peripherals {
        aes: AesPeripheral(hardware.aes),
        #[cfg(feature = "console")]
        console: ConsolePeripheral(interrupts.console_usb),
        cpu: cpu::Cpu0::new(hardware.cache_counters),
    };
    let (mut setup, app) = boot(peripherals, setup)
        .unwrap_or_else(|error| fail(error.name()))
        .into_parts();
    // On hart 0 the group runs here; with `group` on hart 1, below.
    #[cfg(not(feature = "group"))]
    let (net, second) = hart1(GroupPeripherals {}, GroupBoot::new())
        .unwrap_or_else(|error| fail(error.name()))
        .into_parts();
    #[cfg(feature = "group")]
    let _ = hart1;
    // Without a radio executor the application executor writes the timer.
    if let (Some(timer), Some(alarm)) = (setup.timer.take(), setup.app_alarm.take()) {
        let ready = oer_chips_family_platform_executor::init::<Esp32s31, 0, _, _, _, _>(
            oer_chips_family_platform_executor::systimer::SystemTimer {
                comparators: timer.comparators,
                counter: timer.counter,
            },
            setup.adopted.router(),
            alarm,
            timer.alarm1,
        )
        .unwrap_or_else(|failure| failure.reset());
        #[cfg(feature = "group")]
        {
            setup.timers = Some((ready, timer.alarm2));
        }
        #[cfg(not(feature = "group"))]
        let _ = ready;
    }
    // The timer is installed now.
    let id = match (setup.boot_id, setup.entropy.take()) {
        (Some(id), _) => id,
        (None, Some(registers)) => {
            let mut rng = oer_chips_esp32s31_platform_runtime::platform::Entropy::new(
                registers,
                setup.cycles,
            );
            if !rng.is_healthy() {
                fail(Error::Entropy.name());
            }
            match draw_boot_id(&mut rng) {
                Ok(id) => id,
                Err(_) => fail(Error::Entropy.name()),
            }
        }
        (None, None) => fail(Error::Entropy.name()),
    };
    BOOT_ID.publish(id);
    // Hart 1 runs the group; this hart waits until it ended.
    #[cfg(feature = "group")]
    let _hart1 = {
        let Some(timers) = setup.timers.take() else {
            fail(oer_portable_diagnostics::name!("hart1-timer"; 32));
        };
        hart1::launch(
            mailbox,
            timers,
            hart1::Launch {
                permit: hardware.hart1,
                causes: hardware.causes,
                line: software.line1,
                irq: software.irq1,
                wake: interrupts.hart1_wake,
                alarm: interrupts.hart1_alarm,
            },
        )
    };
    let Boot { adopted, .. } = setup;
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(enabled) => enabled,
        Err(error) => fail(error.name()),
    };
    let starts = Starts {
        watchdog: hardware.watchdog,
    };
    // The image's futures stay in this frame, which never returns: the
    // executor polls them as its tasks after the library's.
    #[cfg(not(feature = "group"))]
    let mut net = core::pin::pin!(run_net(net));
    #[cfg(not(feature = "group"))]
    observed_root!(net, Ip);
    #[cfg(not(feature = "group"))]
    let mut second = core::pin::pin!(second());
    #[cfg(not(feature = "group"))]
    let mut second = core::pin::pin!(run_app(second.as_mut()));
    #[cfg(not(feature = "group"))]
    observed_root!(second, Group);
    let mut app = core::pin::pin!(app());
    let mut app = core::pin::pin!(run_app(app.as_mut()));
    observed_root!(app, Application);
    #[cfg(not(feature = "group"))]
    let roots = (
        (
            oer_chips_family_platform_executor::Root::new(net.as_mut()),
            oer_chips_family_platform_executor::Root::new(second.as_mut()),
        ),
        oer_chips_family_platform_executor::Root::new(app.as_mut()),
    );
    #[cfg(feature = "group")]
    let roots = oer_chips_family_platform_executor::Root::new(app.as_mut());
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            roots,
            |spawner| {
                starts.spawn(spawner);
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// The library's tasks the executor starts.
struct Starts {
    watchdog: ArmedRtcWatchdog<RwdtRegisters>,
}

impl Starts {
    fn spawn(self, library: &Spawner<SdkTasks>) {
        let Self { watchdog } = self;
        let started = SdkTasks::watchdog(library, watchdog).is_ok();
        if !started {
            fail(Error::AlreadyStarted.name());
        }
    }
}

/// The parts the boot keeps from the runtime's hardware.
struct Owned {
    reset_cause: ResetCause,
    entropy: EntropyRegisters,
    aes: AesParts,
    system_timer: SystemTimer,
    software: SoftwareInterrupts,
    watchdog: ArmedRtcWatchdog<RwdtRegisters>,
    #[cfg(feature = "group")]
    hart1: oer_chips_esp32s31_hal::system::cores::LaunchPermit,
    #[cfg(feature = "group")]
    causes: oer_chips_esp32s31_hal::peripherals::reset_cause::ResetCauses,
    cache_counters: oer_chips_esp32s31_hal::peripherals::cache_counters::CacheCounters,
    cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
}

/// Where hart 1 takes its parts: the mailbox `entry!` declares with the
/// `group` feature, nothing without it. Not an API.
#[doc(hidden)]
#[cfg(feature = "group")]
pub type GroupMailbox = &'static oer_chips_family_platform_runtime::Mailbox<
    oer_chips_esp32s31_hal::system::platform::Hart1Grant,
    Hart1Parts,
>;
#[doc(hidden)]
#[cfg(not(feature = "group"))]
pub type GroupMailbox = ();

/// Run the application's future, pinned where its root lives: an async
/// function that took it by value would keep a second copy beside the one
/// it awaits. An error ends the image.
async fn run_app<F: core::future::Future<Output = AppResult>>(app: core::pin::Pin<&mut F>) {
    match app.await {
        Ok(never) => match never {},
        Err(error) => fail(error.name()),
    }
}

/// Feed the RTC watchdog for good: the chip resets when the executor stops
/// polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_portable_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match observed!(Watchdog, watchdog.feed_every(&EmbassyClock, period)).await {}
}

/// Retain `reason`, report it and reset: the record-and-reset policy the
/// library applies on the image's behalf.
pub(crate) fn fail<const B: usize>(reason: oer_portable_diagnostics::Name<B>) -> ! {
    oer_chips_family_platform_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_portable_diagnostics::Note::runtime_fail(reason),
        );
    })
}

/// What [`main!`](crate::main) expands to needs; not an API.
#[doc(hidden)]
pub mod __private {
    pub use super::{no_group, start};
    pub use oer_chips_esp32s31_hal::system::platform::Port;
    pub use oer_chips_esp32s31_platform_runtime::exception::capture as exception;
    pub use oer_chips_family_platform_runtime::{Handoff, entry};
    pub use oer_chips_family_platform_runtime::{SecondaryHandoff, secondary_entry};
}

/// Write `bytes` whole to the console, or fail.
pub(crate) fn whole(bytes: &[u8]) -> Result<(), ()> {
    if oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole() {
        Ok(())
    } else {
        Err(())
    }
}

/// Write one application line (`oer::println!`) whole under one hold of the
/// console; whether all of it reached the console.
#[doc(hidden)]
pub fn console_line(line: oer_portable_diagnostics::Line) -> bool {
    oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::early_note(&line.note()).is_whole()
}

/// The ESP32-S31 places a task group on hart 1: its mailbox and root, which
/// call the group's setup there. What `oer::entry!` expands to with the
/// `group` feature. Not an API.
#[cfg(feature = "group")]
#[doc(hidden)]
#[macro_export]
macro_rules! __group_entry {
    ($group:path) => {
        $crate::__private::secondary_entry!(
            1,
            __OER_HART1: $crate::__private::Port,
            $crate::Hart1Parts => __oer_hart1_main
        );

        fn __oer_hart1_main(
            handoff: $crate::__private::SecondaryHandoff<$crate::__private::Port, 1>,
            parts: $crate::Hart1Parts,
        ) -> ! {
            $crate::hart1_main(handoff, parts, $group)
        }
    };
}

/// The mailbox `oer::entry!` passes the library. Not an API.
#[cfg(feature = "group")]
#[doc(hidden)]
#[macro_export]
macro_rules! __group_mailbox {
    () => {
        &__OER_HART1
    };
}
