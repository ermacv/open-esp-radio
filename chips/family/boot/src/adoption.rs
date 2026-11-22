//! Adoption of an already initialized platform, without cold boot or a
//! platform-runtime dependency. Isolated verifier sessions use this same API.

use oer_chips_family_drivers_watchdog::{
    ArmedRtcWatchdog, MainWatchdog, MainWatchdogEnable, MainWatchdogRegisters, MicrosecondTick,
    RtcWatchdog, RtcWatchdogArm, StoppedMainWatchdog, SuperWatchdog, SuperWatchdogRegisters,
};

/// Accept only the mapping the staged entry proves is already in use.
/// The board implementation performs no reset, remap or clock programming.
/// This contract belongs to platform-layout, not to the staged runtime.
pub fn adopt_mapping<B: oer_chips_family_platform_layout::handoff::AdoptsBootstrapMapping>(
    board: B,
    proof: &oer_chips_family_platform_layout::handoff::StageTwoHandoff,
) -> B::Mapping {
    board.adopt(proof)
}

/// The rate of the clock driving the CPU, in hertz: nonzero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuHz(core::num::NonZeroU32);

impl CpuHz {
    /// A rate, or `None` for zero.
    pub const fn new(hz: u32) -> Option<Self> {
        match core::num::NonZeroU32::new(hz) {
            Some(hz) => Some(Self(hz)),
            None => None,
        }
    }

    /// The rate in hertz.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Read-only observation of the clock already driving the CPU: an integer
/// rate, or what makes it one no settle can be counted on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuRate {
    /// An integer divider of a root of known rate.
    Integer(CpuHz),
    /// A root of no known rate (`root`, its selector's code), or a
    /// fractional divider.
    Unsupported { root: u32, fractional: bool },
}

pub trait CpuClock {
    /// Read the CPU clock once; no clock register is written.
    fn sample(self) -> CpuRate;
}

/// The adopted rate, the value the settle counter is made from, or the
/// typed boot failure of a clock it cannot count on.
///
/// # Errors
/// [`ClockFailure::UnsupportedCpuClock`] for a root of no known rate or a
/// fractional divider.
pub fn adopt_cpu_clock<C: CpuClock>(clock: C) -> Result<CpuHz, crate::BootFailure> {
    match clock.sample() {
        CpuRate::Integer(hz) => Ok(hz),
        CpuRate::Unsupported { root, fractional } => Err(crate::BootFailure::Clock(
            crate::ClockFailure::UnsupportedCpuClock { root, fractional },
        )),
    }
}

/// The clock of TIMG0 T0, independent of its watchdog clock and group reset.
pub trait Timer0Clock: MainWatchdogEnable {
    fn start_timer0_clock(&mut self);
}

/// The owned clock/reset words of the runtime's system-deadline watchdog.
pub trait Main1Clock: MainWatchdogRegisters {
    fn start_clock(&mut self);
}

/// Chip settings, unchanged from the runtime's watchdog configuration.
#[derive(Clone, Copy)]
pub struct WatchdogProfile {
    pub tick: MicrosecondTick,
    pub rtc_stage0_hold: u32,
}

/// Unstarted watchdog owners; constructing this performs no writes.
pub struct WatchdogParts<M0, M1, R, S> {
    pub main0: M0,
    pub main1: M1,
    pub rtc: R,
    pub super_watchdog: S,
}

/// Runtime watchdog owners, with the RTC reset backstop armed.
pub struct Watchdogs<
    M0: MainWatchdogEnable,
    M1: MainWatchdogRegisters,
    R: RtcWatchdogArm,
    S: SuperWatchdogRegisters,
> {
    pub main0: StoppedMainWatchdog<M0>,
    pub main1: MainWatchdog<M1>,
    pub rtc: ArmedRtcWatchdog<R>,
    pub super_watchdog: SuperWatchdog<S>,
}

/// Start the existing timer clocks, stop both main watchdogs and arm the RTC.
/// The super watchdog remains as the bootstrap left it, fed by hardware.
pub fn arm_watchdogs<M0, M1, R, S>(
    parts: WatchdogParts<M0, M1, R, S>,
    profile: WatchdogProfile,
) -> Watchdogs<M0, M1, R, S>
where
    M0: Timer0Clock,
    M1: Main1Clock,
    R: RtcWatchdogArm,
    S: SuperWatchdogRegisters,
{
    let WatchdogParts {
        mut main0,
        mut main1,
        rtc,
        super_watchdog,
    } = parts;
    main0.start_timer0_clock();
    main1.start_clock();
    Watchdogs {
        main0: StoppedMainWatchdog::new(main0),
        main1: MainWatchdog::new(main1, profile.tick),
        rtc: RtcWatchdog::new(rtc).arm_rtc_reset(profile.rtc_stage0_hold),
        super_watchdog: SuperWatchdog::new(super_watchdog),
    }
}

/// Hardware adapters supplied by the chip. Each method transfers its exact
/// affine owners; this driver owns the order, including the watchdog window.
pub trait Port: Sized {
    type CpuClock: CpuClock;
    type Console;
    type ClockParts;
    type Clocks;
    type CoreControl;
    type Debug;
    type Resources;
    type Main0: Timer0Clock;
    type Main1: Main1Clock;
    type Rtc: RtcWatchdogArm;
    type Super: SuperWatchdogRegisters;
    type Output;

    const WATCHDOGS: WatchdogProfile;

    fn install_console(console: Self::Console) -> Result<(), InitError>;
    fn adopt_clocks(parts: Self::ClockParts) -> Self::Clocks;
    fn install_core_control(core: Self::CoreControl) -> Result<(), InitError>;
    fn publish_debug_status(debug: Self::Debug) -> Result<(), InitError>;
    /// Publish the interrupt observers/configuration and assemble the remaining
    /// owners after watchdog arming. No cold-start register owner is returned.
    fn finish(
        resources: Self::Resources,
        clocks: Self::Clocks,
        watchdogs: Watchdogs<Self::Main0, Self::Main1, Self::Rtc, Self::Super>,
        cpu: CpuHz,
    ) -> Self::Output;
}

/// The reset claim's unstarted owners. No ready runtime is contained here.
#[must_use = "the adoption driver consumes the unstarted owners"]
pub struct Owners<P: Port> {
    pub cpu_clock: P::CpuClock,
    pub console: P::Console,
    pub clocks: P::ClockParts,
    pub core_control: P::CoreControl,
    pub debug: P::Debug,
    pub watchdogs: WatchdogParts<P::Main0, P::Main1, P::Rtc, P::Super>,
    pub resources: P::Resources,
}

/// Why adoption could not install the image's platform owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitError {
    ConsoleInstalled,
    CoreControlInstalled,
    DebugStatusPublished,
    /// The CPU clock is one no settle can be counted on.
    Boot(crate::BootFailure),
}

impl InitError {
    /// Stable diagnostic spelling; images retain their existing report format.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::ConsoleInstalled => "console-installed",
            Self::CoreControlInstalled => "core-control-installed",
            Self::DebugStatusPublished => "debug-status-published",
            Self::Boot(failure) => failure.reason(),
        }
    }
}

/// Adopt existing clocks and install runtime owners in the original order.
/// This neither resets/remaps PSRAM nor starts a second cold boot. The output
/// is hardware for a session or a staged runner, not a staged `Runtime`.
pub fn initialize<P: Port>(owners: Owners<P>) -> Result<P::Output, InitError> {
    let Owners {
        cpu_clock,
        console,
        clocks,
        core_control,
        debug,
        watchdogs,
        resources,
    } = owners;
    let cpu = adopt_cpu_clock(cpu_clock).map_err(InitError::Boot)?;
    P::install_console(console)?;
    let clocks = P::adopt_clocks(clocks);
    P::install_core_control(core_control)?;
    P::publish_debug_status(debug)?;
    let watchdogs = arm_watchdogs(watchdogs, P::WATCHDOGS);
    Ok(P::finish(resources, clocks, watchdogs, cpu))
}

#[cfg(test)]
mod tests;
