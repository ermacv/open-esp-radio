//! Owned adapters of the family adoption driver. Startup order is not HAL policy.
use super::*;
use crate::{cores, power::PsramMpll};
use oer_esp32s31_pac::ownership::{
    ChipClockPeripherals, DebugPeripherals, InterruptMatrixPeripherals,
};
use oer_espressif_boot::adoption::{self, InitError, Port};

/// The HAL's adoption port over the root's unique parts.
pub struct RuntimePort;

/// Uninstalled platform clock words; only the root supplies them.
pub struct ClockParts(pub(super) ChipClockPeripherals);
/// Uninstalled fatal/core-control owners.
pub struct CoreParts {
    pub(super) core: CoreControlRegisters,
    pub(super) uart0: Uart0ClockRegisters,
}
/// Unpublished per-hart debug observers.
pub struct DebugParts(pub(super) DebugPeripherals);

/// Remaining unstarted resources. Interrupt publication stays after arming.
pub struct RuntimeResources {
    pub(super) aes: crate::peripherals::aes::AesParts,
    pub(super) system_timer: SystemTimer,
    pub(super) timg0_alarm: Timg0AlarmParts,
    pub(super) timg1_alarm: Timg1AlarmParts,
    pub(super) software_interrupts: SoftwareInterrupts,
    pub(super) interrupt_matrix: InterruptMatrixPeripherals,
    pub(super) efuse: EfuseRegisters,
    pub(super) entropy: EntropyRegisters,
    pub(super) reset_cause: ResetCauseRegisters,
    pub(super) wifi_interrupts: WifiInterruptBanks,
    pub(super) radio: RadioHardware,
}

/// Adopted hardware owners, also usable by an isolated verifier session.
/// This is not the staged runtime: it contains no handoff or mapping proof.
#[must_use = "the initialized owners belong to a session or the staged runner"]
pub struct PlatformParts {
    pub hardware: Hardware,
    pub interrupts: InterruptBlocks,
}

/// Hardware consumed by tasks or a verifier session after driver adoption.
pub struct Hardware {
    /// The CPU AES block and its exclusive crypto clock/reset word.
    pub aes: crate::peripherals::aes::AesParts,
    pub watchdogs: adoption::Watchdogs<
        Mwdt0Registers,
        crate::peripherals::watchdog::Mwdt1Registers,
        RwdtRegisters,
        SwdRegisters,
    >,
    pub system_timer: SystemTimer,
    pub timg0_alarm: Timg0AlarmParts,
    pub timg1_alarm: Timg1AlarmParts,
    pub software_interrupts: SoftwareInterrupts,
    pub efuse: EfuseRegisters,
    pub entropy: EntropyRegisters,
    pub reset_cause: ResetCauseRegisters,
    pub wifi_interrupts: WifiInterruptBanks,
    pub clocks: ChipClocks,
    pub radio: RadioHardware,
}

impl Port for RuntimePort {
    type CpuClock = crate::boot::RuntimeCpuClock;
    type Console = ConsoleRegisters;
    type ClockParts = ClockParts;
    type Clocks = ChipClocks;
    type CoreControl = CoreParts;
    type Debug = DebugParts;
    type Resources = RuntimeResources;
    type Main0 = Mwdt0Registers;
    type Main1 = crate::peripherals::watchdog::Mwdt1Registers;
    type Rtc = RwdtRegisters;
    type Super = SwdRegisters;
    type Output = PlatformParts;

    const WATCHDOGS: adoption::WatchdogProfile = adoption::WatchdogProfile {
        tick: crate::peripherals::watchdog::MAIN_WATCHDOG_TICK,
        rtc_stage0_hold: crate::peripherals::watchdog::RTC_WATCHDOG_STAGE0_HOLD,
    };

    fn install_console(console: ConsoleRegisters) -> Result<(), InitError> {
        crate::peripherals::usb_serial_jtag::install(console)
            .map_err(|_| InitError::ConsoleInstalled)
    }
    fn adopt_clocks(parts: ClockParts) -> ChipClocks {
        ChipClocks::install(parts.0, PsramMpll::adopted())
    }
    fn install_core_control(parts: CoreParts) -> Result<(), InitError> {
        cores::install(parts.core, parts.uart0).map_err(|_| InitError::CoreControlInstalled)
    }
    fn publish_debug_status(debug: DebugParts) {
        crate::platform::port::publish_debug_status(debug.0);
    }
    fn finish(
        resources: RuntimeResources,
        clocks: ChipClocks,
        watchdogs: adoption::Watchdogs<Self::Main0, Self::Main1, Self::Rtc, Self::Super>,
    ) -> PlatformParts {
        let RuntimeResources {
            aes,
            system_timer,
            timg0_alarm,
            timg1_alarm,
            software_interrupts,
            interrupt_matrix,
            efuse,
            entropy,
            reset_cause,
            wifi_interrupts,
            radio,
        } = resources;
        PlatformParts {
            interrupts: InterruptBlocks::new(interrupt_matrix),
            hardware: Hardware {
                aes,
                watchdogs,
                system_timer,
                timg0_alarm,
                timg1_alarm,
                software_interrupts,
                efuse,
                entropy,
                reset_cause,
                wifi_interrupts,
                clocks,
                radio,
            },
        }
    }
}
