//! The family platform contracts on this chip (`oer-espressif-platform-port`),
//! implemented by [`Port`]: the early console writer, the fatal path
//! ([`crate::cores`]: the mask and its proof, the stop of the other hart and
//! the software reset), the source identities
//! of the software interrupt lines and the system timer's comparators from
//! the PAC's interrupt parts, the owned executor blocks, and pending/debug
//! reads selected by a checked hart identity.

use oer_esp32s31_pac::{bus_monitor::DebugModeStatus, ownership::DebugPeripherals};
use oer_espressif_platform_port as port;
use oer_memory::zeroed::ZeroedOnce;
use oer_mmio::InterruptSource;

use crate::cores;
use crate::peripherals::{
    software_interrupt::{
        FromCpu0, FromCpu0Irq, FromCpu1, FromCpu1Irq, FromCpu2, FromCpu2Irq, FromCpu3, FromCpu3Irq,
    },
    system_timer::{
        self, SystimerAlarm0, SystimerAlarm1, SystimerAlarm2, SystimerComparators, SystimerCounter,
    },
};
use crate::platform::{Hart, interrupt_controller};

/// The chip's implementation of the family platform contracts.
pub struct Port;

/// The port's hart geometry is the PAC's: both come from the same facts.
const _: () = {
    if port::BOOT_HART.get() != crate::platform::BOOT_HART.get() {
        panic!("the port's boot hart is the PAC's");
    }
};

/// The PAC's identity of a checked port hart. The selection type-checks
/// only while both geometries hold the same hart count.
#[inline(always)]
fn hart(hart: port::Hart) -> Hart {
    *hart.at(&Hart::ALL)
}

/// Per-hart debug status, published once by adoption of the reset root's
/// parts ([`publish_debug_status`]), before any executor exists.
static DEBUG: ZeroedOnce<DebugModeStatus> = ZeroedOnce::new();

/// Publish the per-hart debug status for [`Port::debugger_connected`], from
/// the root's partition.
///
/// # Panics
///
/// When it was published before: the root splits the partition once.
pub(crate) fn publish_debug_status(peripherals: DebugPeripherals) {
    assert!(DEBUG.get().is_none(), "the debug status is published once");
    let _ = DEBUG.get_or_init(|| peripherals.debug_mode_status);
}

impl port::EarlyText for Port {
    #[inline(always)]
    fn text(bytes: &[u8]) -> bool {
        crate::peripherals::usb_serial_jtag::early_text(bytes).is_whole()
    }
}

/// The port's identity of a checked PAC hart; like [`hart`], it type-checks
/// only while both geometries hold the same hart count.
#[inline(always)]
fn port_hart(hart: Hart) -> port::Hart {
    *hart.at(&port::Hart::ALL)
}

impl port::Platform for Port {
    type Fatal<'a> = cores::Fatal<'a>;
    type Stop<'a> = cores::StopTheWorld<'a>;
    type StopError = cores::StopError;

    #[inline(always)]
    fn fatal(path: impl for<'a> FnOnce(Self::Fatal<'a>)) -> ! {
        cores::fatal(path)
    }

    #[inline(always)]
    fn fatal_hart(fatal: &Self::Fatal<'_>) -> Result<port::Hart, port::InvalidHart> {
        fatal
            .hart()
            .map(port_hart)
            .map_err(|cores::Fault::InvalidHart { raw }| port::InvalidHart { raw })
    }

    #[inline(always)]
    fn stop_the_world<'a>(fatal: &Self::Fatal<'a>) -> Result<Self::Stop<'a>, Self::StopError> {
        cores::stop_the_world(fatal)
    }

    #[inline(always)]
    fn software_reset(fatal: Self::Fatal<'_>) -> port::AwaitWatchdog<Self::StopError> {
        let cores::AwaitWatchdog { reason } = cores::software_reset(fatal);
        port::AwaitWatchdog { reason }
    }
}

impl port::SecondaryStart for Port {
    type Cores = cores::Cores;
    type Permit = cores::LaunchPermit;
    type Preconditions = crate::peripherals::reset_cause::ResetCauses;
    type Released = cores::Released;
    type StartError = cores::StartError;
    type Interrupts = crate::platform::Hart1Grant;
    type Controller = crate::platform::Hart1Controller;

    #[inline(always)]
    fn release(
        cores: &mut cores::Cores,
        permit: cores::LaunchPermit,
        preconditions: Self::Preconditions,
        entry: port::SecondaryEntry,
    ) -> Result<cores::Released, cores::StartError> {
        cores.start_secondary(permit, preconditions, entry)
    }

    #[inline(always)]
    fn retire_boot_word(
        cores: &mut cores::Cores,
        released: &mut cores::Released,
    ) -> Result<(), cores::StartError> {
        cores.retire_boot_word(released)
    }

    #[inline(always)]
    fn observations(error: cores::StartError) -> [u32; 3] {
        error.observations()
    }

    fn interrupts(released: &mut cores::Released) -> Option<Self::Interrupts> {
        released.take_interrupts()
    }

    #[cfg(target_arch = "riscv32")]
    fn controller(interrupts: Self::Interrupts) -> Result<Self::Controller, port::ForeignHart> {
        interrupts.into_controller()
    }

    /// The host has no hart identity: no controller is made off the chip.
    #[cfg(not(target_arch = "riscv32"))]
    fn controller(interrupts: Self::Interrupts) -> Result<Self::Controller, port::ForeignHart> {
        interrupts.into_controller_on(usize::MAX)
    }
}

impl port::ExecutorPlatform for Port {
    /// Sources of `FROM_CPU_INTR0..3`, in line order.
    const SOFTWARE_SOURCES: [u16; 4] = [
        FromCpu0Irq::SOURCE,
        FromCpu1Irq::SOURCE,
        FromCpu2Irq::SOURCE,
        FromCpu3Irq::SOURCE,
    ];

    /// Sources of `SYSTIMER_TARGET0..2`, in comparator order.
    const ALARM_SOURCES: [u16; 3] = [
        <SystimerAlarm0 as InterruptSource>::SOURCE,
        <SystimerAlarm1 as InterruptSource>::SOURCE,
        <SystimerAlarm2 as InterruptSource>::SOURCE,
    ];

    const TICKS_PER_MICROSECOND: u64 = system_timer::TICKS_PER_MICROSECOND;

    type Line0 = FromCpu0;
    type Line1 = FromCpu1;
    type Line2 = FromCpu2;
    type Line3 = FromCpu3;
    type Counter = SystimerCounter;
    type Comparators = SystimerComparators;

    /// Whether `source` is pending at the checked hart's bank, without
    /// changing its route. The caller supplies its current hart's identity.
    #[inline(always)]
    fn is_pending(hart: port::Hart, source: u16) -> bool {
        interrupt_controller::is_pending(self::hart(hart), source)
    }

    /// Whether a debugger is attached to the checked hart: its debug module is
    /// active, so a wait for an interrupt would stall the debugger's
    /// access. The stage-two root publishes the status before any executor
    /// can wait; unpublished, no debugger is known.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/debugger.rs`
    /// (`debugger_connected`: `ASSIST_DEBUG` core 0's
    /// `DEBUG_MODULE_ACTIVE`) and ESP-IDF 4d59230d
    /// `components/soc/esp32s31/include/soc/assist_debug_reg.h:11-14`.
    #[inline(always)]
    fn debugger_connected(hart: port::Hart) -> bool {
        DEBUG
            .get()
            .is_some_and(|status| debug_module_active(status, self::hart(hart)))
    }
}

/// Whether the selected hart's debug module is active in `status`.
fn debug_module_active<P: oer_mmio::Placement>(status: &DebugModeStatus<P>, hart: Hart) -> bool {
    status.read_debug_mode(hart.index()).debug_module_active()
}

#[cfg(test)]
mod tests {
    use super::Port;
    use oer_esp32s31_pac::Interrupt;
    use oer_espressif_platform_port::ExecutorPlatform;

    #[test]
    fn the_sources_are_the_pac_s_interrupts() {
        assert_eq!(
            Port::SOFTWARE_SOURCES,
            [
                Interrupt::CPU_INTR_FROM_CPU_0.source(),
                Interrupt::CPU_INTR_FROM_CPU_1.source(),
                Interrupt::CPU_INTR_FROM_CPU_2.source(),
                Interrupt::CPU_INTR_FROM_CPU_3.source(),
            ]
        );
        assert_eq!(
            Port::ALARM_SOURCES,
            [
                Interrupt::SYSTIMER_TARGET0.source(),
                Interrupt::SYSTIMER_TARGET1.source(),
                Interrupt::SYSTIMER_TARGET2.source(),
            ]
        );
    }

    #[test]
    fn debugger_activity_is_local_to_the_selected_hart() {
        use crate::platform::Hart;
        use oer_esp32s31_pac::{
            bus_monitor::{DebugModeStatus, debug_mode},
            instances,
        };
        for active in Hart::ALL {
            let mut words = std::vec![0_u32; instances::bus_monitor::SIZE / 4];
            let index = (debug_mode::OFFSET + active.get() * debug_mode::STRIDE) / 4;
            words[index] = debug_mode::DEBUG_MODE_MASK;
            let status = DebugModeStatus::host(&mut words).expect("memory holds the block");
            assert!(
                !super::debug_module_active(&status, active),
                "debug mode alone"
            );
            words[index] = debug_mode::DEBUG_MODULE_ACTIVE_MASK;
            let status = DebugModeStatus::host(&mut words).expect("memory holds the block");
            for observed in Hart::ALL {
                assert_eq!(
                    super::debug_module_active(&status, observed),
                    observed == active
                );
            }
        }
    }

    #[test]
    fn every_port_hart_is_the_pac_hart_of_its_index() {
        for (port, pac) in oer_espressif_platform_port::Hart::ALL
            .into_iter()
            .zip(crate::platform::Hart::ALL)
        {
            assert_eq!(super::hart(port), pac);
            assert_eq!(super::port_hart(pac), port);
            assert_eq!(port.get(), pac.get());
        }
    }

    #[test]
    fn nothing_is_pending_before_the_bank_is_published() {
        // The host test binary never splits the root's interrupt matrix.
        for hart in oer_espressif_platform_port::Hart::ALL {
            assert!(!Port::is_pending(hart, 68));
            assert!(!Port::debugger_connected(hart));
        }
    }
}
