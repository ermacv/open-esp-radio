//! Adoption of an already initialized platform, without cold boot or a
//! staged-runtime dependency. Isolated verifier sessions use this same API.

use oer_espressif_watchdog::{
    ArmedRtcWatchdog, MainWatchdog, MainWatchdogEnable, MainWatchdogRegisters, MicrosecondTick,
    RtcWatchdog, RtcWatchdogArm, StoppedMainWatchdog, SuperWatchdog, SuperWatchdogRegisters,
};

/// Accept only the mapping the staged entry proves is already in use.
/// The board implementation performs no reset, remap or clock programming.
/// This contract belongs to staged-layout, not to the staged runtime.
pub fn adopt_mapping<B: oer_espressif_staged_layout::handoff::AdoptsBootstrapMapping>(
    board: B,
    proof: &oer_espressif_staged_layout::handoff::StageTwoHandoff,
) -> B::Mapping {
    board.adopt(proof)
}

/// Read-only observation of the clock already driving the CPU.
pub struct CpuRate {
    pub fractional: bool,
    pub integer_hz: Option<u32>,
}

pub trait CpuClock {
    fn sample(&self) -> CpuRate;
    /// Publish the adopted rate for short settles; no clock register is written.
    fn publish(self, rate_hz: Option<u32>);
}

/// Fractional or unsupported clocks continue to refuse short settles.
pub fn adopt_cpu_clock<C: CpuClock>(clock: C) {
    let rate = clock.sample();
    clock.publish(if rate.fractional {
        None
    } else {
        rate.integer_hz
    });
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
    fn publish_debug_status(debug: Self::Debug);
    /// Publish the interrupt observers/configuration and assemble the remaining
    /// owners after watchdog arming. No cold-start register owner is returned.
    fn finish(
        resources: Self::Resources,
        clocks: Self::Clocks,
        watchdogs: Watchdogs<Self::Main0, Self::Main1, Self::Rtc, Self::Super>,
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
}

impl InitError {
    /// Stable diagnostic spelling; images retain their existing report format.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::ConsoleInstalled => "console-installed",
            Self::CoreControlInstalled => "core-control-installed",
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
    adopt_cpu_clock(cpu_clock);
    P::install_console(console)?;
    let clocks = P::adopt_clocks(clocks);
    P::install_core_control(core_control)?;
    P::publish_debug_status(debug);
    let watchdogs = arm_watchdogs(watchdogs, P::WATCHDOGS);
    Ok(P::finish(resources, clocks, watchdogs))
}

#[cfg(test)]
mod tests;
