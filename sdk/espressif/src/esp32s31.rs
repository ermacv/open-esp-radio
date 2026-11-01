//! The ESP32-S31 implementation: the default interrupt layout, executors,
//! service tasks and boot sequence behind [`main!`](crate::main).
//!
//! Layout, fixed for every image (lines and levels from the chip's
//! measured arbitration):
//!
//! | Context | Level | Entries |
//! |---|---|---|
//! | application | 1 | executor wake (line 40), its alarm (comparator 1, line 41), console (USB Serial/JTAG, line 42) |
//! | radio executor | 2 | SAR/PWDET pilot (line 16), executor (line 17) |
//! | radio | 3 | radio alarm (comparator 0, line 19), Wi-Fi MAC (line 21) |
//! | profile (`observe`) | 4 | PC sampler (TIMG1 T0, line 22) |
//!
//! The application executor runs in thread mode on hart 0; the library's
//! service tasks ([`SdkTasks`]) and the image's tasks share it. The radio
//! executor runs only when a Wi-Fi station starts. An entry no driver binds
//! routes no source.

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    peripherals::{
        aes::AesParts,
        entropy::EntropyRegisters,
        software_interrupt::{FromCpu0Irq, SoftwareInterrupts},
        system_timer::{SystemTimer, SystimerAlarm1},
        watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    },
    platform::{InterruptController, Port},
    root::Root,
};
use oer_espressif_executor::{Executor, Spawner};
use oer_espressif_staged_runtime::{Adopted, Handoff};

pub use oer_esp32s31_compose::Esp32s31;

/// Time on the executors' clock: timers, timeouts, instants.
pub use embassy_time as time;

/// `$future`, pinned in place for the statement that awaits it and
/// observed as the library's part `$part` (`observe`).
#[cfg(feature = "observe")]
macro_rules! observed {
    ($part:ident, $future:expr) => {
        $crate::esp32s31::observed::Polled::library(
            $crate::observe::parts::LibraryPart::$part,
            core::pin::pin!($future),
        )
    };
}
/// `$future` itself.
#[cfg(not(feature = "observe"))]
macro_rules! observed {
    ($part:ident, $future:expr) => {
        $future
    };
}

/// Shadow the pinned root `$root` with itself observed as the library's
/// part `$part` (`observe`).
#[cfg(feature = "observe")]
macro_rules! observed_root {
    ($root:ident, $part:ident) => {
        let mut $root = core::pin::pin!($crate::esp32s31::observed::Polled::library(
            $crate::observe::parts::LibraryPart::$part,
            $root.as_mut(),
        ));
    };
}
/// Nothing.
#[cfg(not(feature = "observe"))]
macro_rules! observed_root {
    ($root:ident, $part:ident) => {};
}

#[cfg(feature = "observe")]
pub(crate) mod observed;
#[cfg(feature = "observe")]
use observed::executor_observer;
/// The observer of executor `SLOT`: none without `observe`.
#[cfg(not(feature = "observe"))]
#[cfg_attr(
    not(oer_trace),
    allow(
        dead_code,
        reason = "only a trace build asks a task set for its observer"
    )
)]
const fn executor_observer<const SLOT: usize>() -> oer_trace::Noop {
    oer_trace::Noop
}

mod aes;
pub use aes::{Aes, AesClient, AesSeats, CAPACITY as AES_CAPACITY, CLIENTS as AES_CLIENTS};

/// The Wi-Fi station and its IP service.
#[cfg(feature = "wifi")]
pub mod wifi;

#[cfg(feature = "console")]
mod console;
#[cfg(feature = "console")]
pub use console::{Console, ConsolePeripheral, UsbConsole};

/// What the interrupt controller showed after the previous boot's
/// exception trapped.
pub use oer_esp32s31_compose::exception::{LineView, Observed, Terminal, TrapIdentity};
/// Why a hart left reset.
pub use oer_esp32s31_hal::peripherals::reset_cause::ResetCause;
/// The record of the previous boot's panic.
pub use oer_espressif_staged_runtime::panic::PanicRecord;
/// The previous boot's exception as the trap left it.
pub use oer_espressif_staged_runtime::trap::PreviousException;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; hart1_app: 1; profile: 4; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    /// The application executor's alarm (system timer comparator 1).
    alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<SdkTasks, 1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    #[cfg(feature = "wifi")]
    /// The radio executor's alarm (system timer comparator 0).
    radio_alarm: RadioAlarmToken = oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<wifi::RadioTasks, 0, oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    #[cfg(feature = "wifi")]
    /// The SAR/PWDET pilot, between radio executor rounds.
    phy: PhyToken = oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => wifi::on_pilot, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    #[cfg(feature = "wifi")]
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = oer_esp32s31_hal::peripherals::software_interrupt::FromCpu2Irq => oer_espressif_executor::Round<wifi::RadioTasks>, line 17, context radio_executor, hart 0, Executor;
    #[cfg(feature = "wifi")]
    /// The radio handler (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = wifi::WifiPart => wifi::on_wifi, line 21, context radio, hart 0, Hardware;
    #[cfg(feature = "console")]
    /// The USB Serial/JTAG console: it raises the console endpoint's signal.
    console_usb: ConsoleToken = oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt => console::on_usb, line 42, context app_wake, hart 0, Hardware;
    #[cfg(feature = "observe")]
    /// The PC sampler (TIMG1 T0), above every other level of hart 0 so that
    /// it sees what it interrupts; it ticks only while a session runs.
    sampler: SamplerToken = oer_esp32s31_hal::peripherals::timg1_alarm::Timg1AlarmInterrupt => observed::on_sampler, line 22, context profile, hart 0, Hardware;
    #[cfg(feature = "hart1")]
    /// Hart 1's executor's wake (thread mode).
    hart1_wake: Hart1WakeToken = oer_esp32s31_hal::peripherals::software_interrupt::FromCpu1Irq => oer_espressif_executor::wake_handler::<Esp32s31, 1, oer_esp32s31_hal::peripherals::software_interrupt::FromCpu1Irq>, line 40, context hart1_app, hart 1, Hardware;
    #[cfg(feature = "hart1")]
    /// Hart 1's executor's alarm (system timer comparator 2).
    hart1_alarm: Hart1AlarmToken = oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm2 => oer_espressif_executor::alarm_interrupt::<hart1::Hart1Tasks, 2, oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm2>, line 41, context hart1_app, hart 1, Hardware;
}

#[cfg(feature = "hart1")]
mod hart1;
#[cfg(feature = "hart1")]
#[doc(hidden)]
pub use hart1::{Hart1Parts, hart1_main};

oer_espressif_executor::tasks! {
    /// The library's service tasks on the application executor; an image's
    /// set extends it.
    pub SdkTasks: Executor<Esp32s31, 0> {
        @observer: executor_observer::<0>();
        /// Feeds the RTC watchdog while the executor polls.
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
        /// The shared AES service.
        aes_service(launch: aes::AesStart) = aes::serve;
        #[cfg(feature = "wifi")]
        /// The Wi-Fi station's runner.
        station(launch: wifi::StationStart) = wifi::serve_station;
    }
}

oer_espressif_executor::time_driver!(Esp32s31);
oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);

/// The peripherals an application may take, each once.
pub struct Peripherals {
    /// The AES accelerator: [`Aes::start`] makes it the shared service.
    pub aes: AesPeripheral,
    /// The radio: [`wifi::Station::start`] takes it.
    #[cfg(feature = "wifi")]
    pub wifi: wifi::WifiPeripheral,
    /// The USB Serial/JTAG console: [`Console::open`] takes it.
    #[cfg(feature = "console")]
    pub console: ConsolePeripheral,
}

/// The AES accelerator's owner.
pub struct AesPeripheral(pub(crate) AesParts);

/// What the application's boot builds drivers and services with: the
/// interrupt router before interrupts are enabled, and the library's
/// service tasks to start once the executor runs. It exists only while the
/// boot function runs, before any executor.
pub struct Boot {
    adopted: Adopted<InterruptController>,
    pub(crate) pending: Pending,
    /// The system timer, until a driver installs it or the boot ends.
    timer: Option<SystemTimer>,
    app_alarm: Option<AppAlarmToken>,
    #[cfg(feature = "wifi")]
    radio: Option<wifi::RadioBoot>,
    /// The doorbell's urgent source, once a station started.
    #[cfg(feature = "wifi")]
    urgent: Option<wifi::Urgent>,
    /// The entropy source, until a station takes it or the boot ends.
    pub(crate) entropy: Option<EntropyRegisters>,
    /// The boot's identity, once a station drew it.
    pub(crate) boot_id: Option<u64>,
    evidence: Evidence,
    /// The station's network device, for the second boot.
    #[cfg(feature = "wifi")]
    pub(crate) device: Option<wifi::Device>,
    /// The time driver's authority and the comparator for hart 1's
    /// executor, once a station installed the time driver.
    #[cfg(feature = "hart1")]
    pub(crate) timers: Option<(
        oer_espressif_executor::TimeDriverReady<Esp32s31>,
        oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm2,
    )>,
    /// What only the boot hart observes, until the boot takes it.
    #[cfg(feature = "observe")]
    observe: Option<observed::Hart0>,
}

/// No IP service: the second boot started none.
pub struct NoNet;

impl Boot {
    pub(crate) fn router(
        &mut self,
    ) -> &mut oer_espressif_interrupt_table::Router<InterruptController, 0> {
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
pub(crate) fn draw_boot_id(rng: &mut oer_esp32s31_compose::platform::Entropy) -> u64 {
    let mut bytes = [0; 8];
    rng.fill(&mut bytes);
    crate::boot::identity(bytes)
}

/// Service tasks a boot step prepared, started with the executor.
#[derive(Default)]
pub(crate) struct Pending {
    pub(crate) aes: Option<aes::AesStart>,
    #[cfg(feature = "wifi")]
    pub(crate) station: Option<wifi::StationStart>,
}

impl Boot {
    /// End the boot: run `app` on hart 0 beside the library's service
    /// tasks once the executor starts.
    pub fn run<F: core::future::Future<Output = AppResult>>(self, app: F) -> Run<F> {
        Run { boot: self, app }
    }
}

/// A finished boot and the application it runs: what an image's boot
/// returns, as `impl Started`.
pub trait Started: started::Sealed {
    /// The application's future.
    type App: core::future::Future<Output = AppResult>;
    #[doc(hidden)]
    fn into_parts(self) -> (Boot, Self::App);
}

mod started {
    pub trait Sealed {}
}

/// The finished boot [`Boot::run`] returns.
pub struct Run<F> {
    boot: Boot,
    app: F,
}

impl<F> started::Sealed for Run<F> {}

impl<F: core::future::Future<Output = AppResult>> Started for Run<F> {
    type App = F;
    fn into_parts(self) -> (Boot, F) {
        (self.boot, self.app)
    }
}

/// What the boot hands the second boot: the station's network device when
/// the boot started a station.
pub struct Hart1Peripherals {
    /// The station's network device, which [`wifi::Net::start`] takes.
    #[cfg(feature = "wifi")]
    pub device: Option<wifi::Device>,
}

/// The second boot's context: `entry!(boot, hart1 = boot1)` calls `boot1`
/// with it once the boot ended, before any executor, and runs the future it
/// ends with beside the IP service it started. Both run where the second
/// boot runs: on hart 0, the application's executor, or with the `hart1`
/// feature on hart 1's own executor, which hart 1 runs alone.
///
/// `N` is the IP service the second boot started, whose pool sizes are the
/// image's: [`NoNet`] until `wifi::Net::start` consumes the context and
/// returns it with the service.
pub struct Hart1Boot<N = NoNet> {
    net: N,
}

impl Hart1Boot {
    /// The same context with `net` as its IP service.
    #[cfg(feature = "wifi")]
    pub(crate) fn with_net<M>(self, net: M) -> Hart1Boot<M> {
        let Self { net: NoNet } = self;
        Hart1Boot { net }
    }
}

impl<N: NetTask> Hart1Boot<N> {
    /// End the second boot: run `future` beside its IP service.
    pub fn run<F: core::future::Future<Output = AppResult>>(self, future: F) -> Hart1Run<F, N> {
        Hart1Run {
            net: self.net,
            future,
        }
    }
}

/// A finished second boot: what `boot1` returns, as `impl Hart1Started`.
pub trait Hart1Started: started::Sealed {
    /// The second boot's future.
    type Future: core::future::Future<Output = AppResult>;
    /// The IP service it started, or [`NoNet`].
    type Net: NetTask;
    #[doc(hidden)]
    fn into_parts(self) -> (Self::Net, Self::Future);
}

/// The finished second boot [`Hart1Boot::run`] returns.
pub struct Hart1Run<F, N> {
    net: N,
    future: F,
}

impl<F, N> started::Sealed for Hart1Run<F, N> {}

impl<F: core::future::Future<Output = AppResult>, N: NetTask> Hart1Started for Hart1Run<F, N> {
    type Future = F;
    type Net = N;
    fn into_parts(self) -> (N, F) {
        (self.net, self.future)
    }
}

pub use crate::channel::ChannelError;

/// A channel between the application's future and the second boot's
/// ([`crate::channel`]), its ends waking through [`AppNotify`] and
/// [`SecondNotify`].
pub type Channel<A, B, const N: usize> = crate::channel::Channel<A, B, N, AppNotify, SecondNotify>;
/// The application's end of a [`Channel`]: it sends `A` and receives `B`.
pub type ApplicationEnd<A, B, const N: usize> =
    crate::channel::End<A, B, N, AppNotify, SecondNotify>;
/// The second boot's end of a [`Channel`]: it sends `B` and receives `A`.
pub type SecondEnd<A, B, const N: usize> = crate::channel::End<B, A, N, SecondNotify, AppNotify>;

/// How a wait of the application's future is told: on one hart its waker,
/// with the second boot on hart 1 the application's executor signal, which
/// hart 1 raises.
#[cfg(not(feature = "hart1"))]
pub type AppNotify = atomic_waker::AtomicWaker;
/// How a wait of the second boot's future is told, likewise.
#[cfg(not(feature = "hart1"))]
pub type SecondNotify = atomic_waker::AtomicWaker;
#[cfg(feature = "hart1")]
pub type AppNotify =
    &'static oer_espressif_interrupt_table::signal::PeerNotify<Executor<Esp32s31, 0>>;
#[cfg(feature = "hart1")]
pub type SecondNotify =
    &'static oer_espressif_interrupt_table::signal::PeerNotify<Executor<Esp32s31, 1>>;

/// The application's notify, which the IP status publication tells.
#[cfg(all(feature = "wifi", not(feature = "hart1")))]
pub(crate) static APPLICATION: AppNotify = atomic_waker::AtomicWaker::new();
#[cfg(all(feature = "wifi", feature = "hart1"))]
pub(crate) static APPLICATION: AppNotify = &hart1::APPLICATION;

#[cfg(feature = "hart1")]
impl<A, B, const N: usize> crate::channel::Channel<A, B, N, AppNotify, SecondNotify> {
    /// A channel between hart 0's application and hart 1's second boot.
    pub const fn new() -> Self {
        Self::across_harts(&hart1::APPLICATION, &hart1::SECOND)
    }
}

impl<A: Send, B: Send, const N: usize> Channel<A, B, N> {
    /// The application's end, taken once in the boot.
    ///
    /// # Errors
    /// [`Error::Channel`]: the end was taken already.
    pub fn application(&'static self, boot: &mut Boot) -> Result<ApplicationEnd<A, B, N>, Error> {
        let _ = boot;
        self.take_application().map_err(Error::Channel)
    }

    /// The second boot's end, taken once in the second boot after the
    /// boot took the application's.
    ///
    /// # Errors
    /// [`Error::Channel`]: the end was taken already, or the boot took no
    /// application end.
    pub fn second<M>(&'static self, boot: &mut Hart1Boot<M>) -> Result<SecondEnd<A, B, N>, Error> {
        let _ = boot;
        self.take_second().map_err(Error::Channel)
    }
}

/// The second boot of an image that declares none: it drops what the boot
/// handed it (a station's device tells the station that no network takes
/// its frames) and runs nothing.
#[doc(hidden)]
pub fn no_hart1(_handed: Hart1Peripherals, boot: Hart1Boot) -> Result<impl Hart1Started, Error> {
    Ok(boot.run(core::future::pending()))
}

/// An IP service the executor runs beside the application, with the
/// image's pool sizes; not an API.
#[doc(hidden)]
pub trait NetTask: 'static {
    /// The service, for good.
    fn serve(self) -> impl core::future::Future<Output = ()>;
}

impl NetTask for NoNet {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        core::future::ready(())
    }
}

/// Run the IP service, or end at once without one.
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
    /// The AES service has no free client seat.
    AesClients,
    /// A static storage was taken already.
    Storage,
    /// The entropy source failed its health check.
    Entropy,
    /// A channel's end was refused.
    Channel(ChannelError),
    /// The console could not take the USB Serial/JTAG block.
    #[cfg(feature = "console")]
    Console,
    /// The IP service refused its configuration.
    #[cfg(feature = "wifi")]
    Net(oer_embassy_net::ip::Error),
    /// The station's configuration was refused.
    #[cfg(feature = "wifi")]
    WifiConfig(wifi::ConfigError),
    /// A network profile was refused.
    #[cfg(feature = "wifi")]
    WifiProfile(wifi::ProfileError),
    /// A wait on the IP service ended without its condition.
    #[cfg(feature = "wifi")]
    NetWait(crate::net::NetError),
    /// The second boot asked for the station's device, and the boot started
    /// no station.
    #[cfg(feature = "wifi")]
    NoDevice,
    /// The image named every one of its observed parts.
    #[cfg(feature = "observe")]
    ImageParts,
    /// The boot took what the boot hart observes before.
    #[cfg(feature = "observe")]
    ObserveTaken,
}

#[cfg(feature = "wifi")]
impl From<wifi::ConfigError> for Error {
    fn from(error: wifi::ConfigError) -> Self {
        Self::WifiConfig(error)
    }
}

#[cfg(feature = "wifi")]
impl From<wifi::ProfileError> for Error {
    fn from(error: wifi::ProfileError) -> Self {
        Self::WifiProfile(error)
    }
}

#[cfg(feature = "wifi")]
impl From<crate::net::NetError> for Error {
    fn from(error: crate::net::NetError) -> Self {
        Self::NetWait(error)
    }
}

impl Error {
    /// The error's name in the retained record.
    pub const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::AlreadyStarted => oer_diagnostics::name!("sdk-already-started"; 32),
            Self::InterruptTaken => oer_diagnostics::name!("sdk-interrupt-taken"; 32),
            Self::InterruptRouted => oer_diagnostics::name!("sdk-interrupt-routed"; 32),
            Self::AesClients => oer_diagnostics::name!("sdk-aes-clients"; 32),
            Self::Storage => oer_diagnostics::name!("sdk-storage"; 32),
            Self::Entropy => oer_diagnostics::name!("sdk-entropy"; 32),
            Self::Channel(_) => oer_diagnostics::name!("sdk-channel"; 32),
            #[cfg(feature = "console")]
            Self::Console => oer_diagnostics::name!("sdk-console"; 32),
            #[cfg(feature = "wifi")]
            Self::Net(_) => oer_diagnostics::name!("sdk-net"; 32),
            #[cfg(feature = "wifi")]
            Self::WifiConfig(_) => oer_diagnostics::name!("sdk-wifi-config"; 32),
            #[cfg(feature = "wifi")]
            Self::WifiProfile(_) => oer_diagnostics::name!("sdk-wifi-profile"; 32),
            #[cfg(feature = "wifi")]
            Self::NetWait(_) => oer_diagnostics::name!("sdk-net-wait"; 32),
            #[cfg(feature = "wifi")]
            Self::NoDevice => oer_diagnostics::name!("sdk-no-device"; 32),
            #[cfg(feature = "observe")]
            Self::ImageParts => oer_diagnostics::name!("sdk-image-parts"; 32),
            #[cfg(feature = "observe")]
            Self::ObserveTaken => oer_diagnostics::name!("sdk-observe-taken"; 32),
        }
    }
}

/// What an application's future ends with: never, or why it stopped.
pub type AppResult = Result<core::convert::Infallible, Error>;

/// Run the image: adopt the runtime, report the previous boot's faults,
/// call `boot` with the peripherals, then `hart1` with what the boot handed
/// it, and start the executor with the library's service tasks, the second
/// boot's IP service and future, and the application's future. Any error
/// records and resets.
#[doc(hidden)]
pub fn start<S: Started, H: Hart1Started>(
    handoff: Handoff<Port>,
    boot: fn(Peripherals, Boot) -> Result<S, Error>,
    hart1: fn(Hart1Peripherals, Hart1Boot) -> Result<H, Error>,
    #[cfg_attr(not(feature = "hart1"), allow(unused_variables))] mailbox: Hart1Mailbox,
) -> ! {
    let previous = oer_espressif_staged_runtime::panic::peek_previous();
    // Read before adoption prints and clears them.
    let previous_exception =
        oer_espressif_staged_runtime::trap::peek_previous_exception().map(|trap| {
            ExceptionEvidence {
                trap,
                // Only the observations of this trap: an earlier trap's
                // record may outlive it.
                observed: oer_esp32s31_compose::exception::peek_previous()
                    .filter(|observed| observed.follows(&trap)),
            }
        });
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let (hardware, _psram, adopted) =
        match oer_espressif_staged_runtime::adoption::run::<Esp32s31, _, _>(
            handoff,
            root.into_runtime(),
            INTERRUPT_TABLE,
            oer_esp32s31_board::BootstrapPsram,
            oer_esp32s31_compose::phy_fault::report_previous,
            |hardware| {
                let causes = oer_esp32s31_compose::reset_causes::read(hardware.reset_cause);
                oer_esp32s31_compose::wifi_fault::report_previous();
                oer_esp32s31_compose::radio_fault::report_previous();
                oer_esp32s31_compose::exception::report_previous();
                if let Some(record) = previous {
                    let mut console = oer_espressif_staged_runtime::diagnostics::Console::shared(
                        crate::print::whole,
                    );
                    let _ = oer_espressif_staged_runtime::diagnostics::previous_panic(
                        &mut console,
                        &record,
                    );
                }
                Owned {
                    reset_cause: causes.of(oer_esp32s31_hal::platform::BOOT_HART),
                    entropy: hardware.entropy,
                    aes: hardware.aes,
                    system_timer: hardware.system_timer,
                    software: hardware.software_interrupts,
                    watchdog: hardware.watchdogs.rtc,
                    #[cfg(feature = "hart1")]
                    hart1: hardware.hart1,
                    #[cfg(feature = "hart1")]
                    causes,
                    #[cfg(feature = "observe")]
                    timg1_alarm: hardware.timg1_alarm,
                    #[cfg(feature = "observe")]
                    cache_counters: hardware.cache_counters,
                    #[cfg(feature = "wifi")]
                    wifi: wifi::WifiPeripheral {
                        radio: hardware.radio,
                        clocks: hardware.clocks,
                        timg0_alarm: hardware.timg0_alarm,
                        interrupts: hardware.wifi_interrupts,
                        efuse: hardware.efuse,
                    },
                }
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    oer_esp32s31_compose::exception::report_stack_guard();
    let Some(interrupts) = Interrupts::take() else {
        fail(Error::InterruptTaken.name());
    };
    let software: SoftwareInterrupts = hardware.software;
    #[cfg_attr(not(feature = "observe"), allow(unused_mut))]
    let mut setup = Boot {
        adopted,
        pending: Pending::default(),
        timer: Some(hardware.system_timer),
        app_alarm: Some(interrupts.alarm),
        #[cfg(feature = "wifi")]
        radio: Some(wifi::RadioBoot {
            line2: software.line2,
            irq2: software.irq2,
            line3: software.line3,
            irq3: software.irq3,
            radio_alarm: interrupts.radio_alarm,
            phy: interrupts.phy,
            radio: interrupts.radio,
            wifi: interrupts.wifi,
        }),
        #[cfg(feature = "wifi")]
        urgent: None,
        entropy: Some(hardware.entropy),
        boot_id: None,
        evidence: Evidence {
            reset_cause: hardware.reset_cause,
            previous_panic: previous,
            previous_exception,
        },
        #[cfg(feature = "wifi")]
        device: None,
        #[cfg(feature = "hart1")]
        timers: None,
        #[cfg(feature = "observe")]
        observe: None,
    };
    #[cfg(feature = "observe")]
    {
        let hart0 = observed::hart0(
            hardware.timg1_alarm,
            hardware.cache_counters,
            &mut setup,
            interrupts.sampler,
        );
        setup.observe = Some(hart0);
    }
    let peripherals = Peripherals {
        aes: AesPeripheral(hardware.aes),
        #[cfg(feature = "wifi")]
        wifi: hardware.wifi,
        #[cfg(feature = "console")]
        console: ConsolePeripheral(interrupts.console_usb),
    };
    let (mut setup, app) = boot(peripherals, setup)
        .unwrap_or_else(|error| fail(error.name()))
        .into_parts();
    let handed = Hart1Peripherals {
        #[cfg(feature = "wifi")]
        device: setup.device.take(),
    };
    // On hart 0 the second boot runs here; with `hart1` on hart 1, below.
    #[cfg(not(feature = "hart1"))]
    let (net, second) = hart1(handed, Hart1Boot { net: NoNet })
        .unwrap_or_else(|error| fail(error.name()))
        .into_parts();
    #[cfg(feature = "hart1")]
    let _ = hart1;
    // Without a radio executor the application executor writes the timer.
    if let (Some(timer), Some(alarm)) = (setup.timer.take(), setup.app_alarm.take()) {
        let ready = oer_espressif_executor::init::<Esp32s31, 0, _, _, _>(
            oer_espressif_executor::systimer::SystemTimer {
                comparators: timer.comparators,
                counter: timer.counter,
            },
            setup.adopted.router(),
            alarm,
            timer.alarm1,
        )
        .unwrap_or_else(|failure| failure.reset());
        #[cfg(feature = "hart1")]
        {
            setup.timers = Some((ready, timer.alarm2));
        }
        #[cfg(not(feature = "hart1"))]
        let _ = ready;
    }
    // A station drew the identity with the entropy source it took; otherwise
    // the timer is installed now, and the read gap has a clock.
    let id = match (setup.boot_id, setup.entropy.take()) {
        (Some(id), _) => id,
        (None, Some(registers)) => {
            let mut rng = oer_esp32s31_compose::platform::Entropy::new(registers, EmbassyClock);
            if !rng.is_healthy() {
                fail(Error::Entropy.name());
            }
            draw_boot_id(&mut rng)
        }
        (None, None) => fail(Error::Entropy.name()),
    };
    BOOT_ID.publish(id);
    // Hart 1 runs the second boot; this hart waits until it ended.
    #[cfg(feature = "hart1")]
    let _hart1 = {
        let Some(timers) = setup.timers.take() else {
            fail(oer_diagnostics::name!("hart1-timer"; 32));
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
            handed,
        )
    };
    #[cfg(feature = "wifi")]
    let urgent = setup.urgent.take();
    let Boot {
        adopted, pending, ..
    } = setup;
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(enabled) => enabled,
        Err(error) => fail(error.name()),
    };
    let starts = Starts {
        watchdog: hardware.watchdog,
        pending,
    };
    // The image's futures stay in this frame, which never returns: the
    // executor polls them as its tasks after the library's.
    #[cfg(not(feature = "hart1"))]
    let mut net = core::pin::pin!(run_net(net));
    #[cfg(not(feature = "hart1"))]
    observed_root!(net, Ip);
    #[cfg(not(feature = "hart1"))]
    let mut second = core::pin::pin!(run_app(second));
    #[cfg(not(feature = "hart1"))]
    observed_root!(second, Second);
    let mut app = core::pin::pin!(run_app(app));
    observed_root!(app, Application);
    #[cfg(not(feature = "hart1"))]
    let roots = (
        (
            oer_espressif_executor::Root::new(net.as_mut()),
            oer_espressif_executor::Root::new(second.as_mut()),
        ),
        oer_espressif_executor::Root::new(app.as_mut()),
    );
    #[cfg(feature = "hart1")]
    let roots = oer_espressif_executor::Root::new(app.as_mut());
    #[cfg(feature = "wifi")]
    if let Some(urgent) = urgent {
        match executor
            .run(
                &hart,
                &mut router,
                interrupts.wake,
                software.irq0,
                urgent,
                roots,
                |spawner| {
                    starts.spawn(spawner);
                },
            )
            .unwrap_or_else(|failure| failure.reset()) {}
    }
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
    pending: Pending,
}

impl Starts {
    fn spawn(self, library: &Spawner<SdkTasks>) {
        let Self { watchdog, pending } = self;
        let started = SdkTasks::watchdog(library, watchdog).is_ok()
            && pending
                .aes
                .is_none_or(|engine| SdkTasks::aes_service(library, engine).is_ok());
        #[cfg(feature = "wifi")]
        let started = started
            && pending
                .station
                .is_none_or(|start| SdkTasks::station(library, start).is_ok());
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
    #[cfg(feature = "hart1")]
    hart1: oer_esp32s31_hal::cores::LaunchPermit,
    #[cfg(feature = "hart1")]
    causes: oer_esp32s31_hal::peripherals::reset_cause::ResetCauses,
    #[cfg(feature = "observe")]
    timg1_alarm: oer_esp32s31_hal::peripherals::timg1_alarm::Timg1AlarmParts,
    #[cfg(feature = "observe")]
    cache_counters: oer_esp32s31_hal::peripherals::cache_counters::CacheCounters,
    #[cfg(feature = "wifi")]
    wifi: wifi::WifiPeripheral,
}

/// Where hart 1 takes its parts: the mailbox `entry!` declares with the
/// `hart1` feature, nothing without it. Not an API.
#[doc(hidden)]
#[cfg(feature = "hart1")]
pub type Hart1Mailbox = &'static oer_espressif_staged_runtime::Mailbox<
    oer_esp32s31_hal::platform::Hart1Grant,
    Hart1Parts,
>;
#[doc(hidden)]
#[cfg(not(feature = "hart1"))]
pub type Hart1Mailbox = ();

/// Run the application's future; an error ends the image.
async fn run_app(app: impl core::future::Future<Output = AppResult>) {
    match app.await {
        Ok(never) => match never {},
        Err(error) => fail(error.name()),
    }
}

/// Feed the RTC watchdog for good: the chip resets when the executor stops
/// polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match observed!(Watchdog, watchdog.feed_every(&EmbassyClock, period)).await {}
}

/// Retain `reason`, report it and reset: the record-and-reset policy the
/// library applies on the image's behalf.
pub(crate) fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = oer_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_diagnostics::Note::runtime_fail(reason),
        );
    })
}

/// What [`main!`](crate::main) expands to needs; not an API.
#[doc(hidden)]
pub mod __private {
    pub use super::{no_hart1, start};
    pub use oer_esp32s31_compose::exception::capture as exception;
    pub use oer_esp32s31_hal::platform::Port;
    pub use oer_espressif_staged_runtime::{Handoff, entry};
    pub use oer_espressif_staged_runtime::{SecondaryHandoff, secondary_entry};
}
