//! The executor's platform contract on this chip (`oer-espressif-executor`'s
//! `ExecutorPlatform`): the source identities of the software interrupt
//! lines and the system timer's comparators, from the PAC's interrupt
//! parts, the pending read of hart 0's interrupt-matrix bank and whether a
//! debugger is attached to hart 0.

use oer_esp32s31_pac::{bus_monitor::DebugModeStatus, ownership::DebugPeripherals};
use oer_memory::zeroed::ZeroedOnce;
use oer_mmio::InterruptSource;

use crate::platform::{
    FromCpu0Irq, FromCpu1Irq, FromCpu2Irq, FromCpu3Irq, SystimerAlarm0, SystimerAlarm1,
    SystimerAlarm2, interrupt_controller,
};

/// The chip's executor platform.
pub struct Platform;

/// Hart 0's debug status, published once by the stage-two split of the
/// reset root ([`publish_debug_status`]), before any executor exists.
static DEBUG: ZeroedOnce<DebugModeStatus> = ZeroedOnce::new();

/// Publish hart 0's debug status for [`Platform::debugger_connected`], from
/// the root's partition.
///
/// # Panics
///
/// When it was published before: the root splits the partition once.
pub(crate) fn publish_debug_status(peripherals: DebugPeripherals) {
    assert!(DEBUG.get().is_none(), "the debug status is published once");
    let _ = DEBUG.get_or_init(|| peripherals.debug_mode_status);
}

impl Platform {
    /// Sources of `FROM_CPU_INTR0..3`, in line order.
    pub const SOFTWARE_SOURCES: [u16; 4] = [
        FromCpu0Irq::SOURCE,
        FromCpu1Irq::SOURCE,
        FromCpu2Irq::SOURCE,
        FromCpu3Irq::SOURCE,
    ];

    /// Sources of `SYSTIMER_TARGET0..2`, in comparator order.
    pub const ALARM_SOURCES: [u16; 3] = [
        <SystimerAlarm0 as InterruptSource>::SOURCE,
        <SystimerAlarm1 as InterruptSource>::SOURCE,
        <SystimerAlarm2 as InterruptSource>::SOURCE,
    ];

    /// Whether `source` is pending at hart 0's bank, the hart stage two
    /// runs on, without changing its route.
    #[inline(always)]
    pub fn is_pending(source: u16) -> bool {
        interrupt_controller::is_pending(source)
    }

    /// Whether a debugger is attached to hart 0: its debug module is
    /// active, so a wait for an interrupt would stall the debugger's
    /// access. The stage-two root publishes the status before any executor
    /// can wait; unpublished, no debugger is known.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/debugger.rs`
    /// (`debugger_connected`: `ASSIST_DEBUG` core 0's
    /// `DEBUG_MODULE_ACTIVE`) and ESP-IDF 4d59230d
    /// `components/soc/esp32s31/include/soc/assist_debug_reg.h:11-14`.
    #[inline(always)]
    pub fn debugger_connected() -> bool {
        DEBUG.get().is_some_and(debug_module_active)
    }
}

/// Whether hart 0's debug module is active in `status`.
fn debug_module_active<P: oer_mmio::Placement>(status: &DebugModeStatus<P>) -> bool {
    status.read_core_0_debug_mode().core_0_debug_module_active()
}

#[cfg(test)]
mod tests {
    use super::Platform;
    use oer_esp32s31_pac::Interrupt;

    #[test]
    fn the_sources_are_the_pac_s_interrupts() {
        assert_eq!(
            Platform::SOFTWARE_SOURCES,
            [
                Interrupt::CPU_INTR_FROM_CPU_0.source(),
                Interrupt::CPU_INTR_FROM_CPU_1.source(),
                Interrupt::CPU_INTR_FROM_CPU_2.source(),
                Interrupt::CPU_INTR_FROM_CPU_3.source(),
            ]
        );
        assert_eq!(
            Platform::ALARM_SOURCES,
            [
                Interrupt::SYSTIMER_TARGET0.source(),
                Interrupt::SYSTIMER_TARGET1.source(),
                Interrupt::SYSTIMER_TARGET2.source(),
            ]
        );
    }

    #[test]
    fn a_debugger_is_attached_while_hart_0_s_debug_module_is_active() {
        use oer_esp32s31_pac::{bus_monitor::DebugModeStatus, instances};
        // SOURCE: ESP-IDF 4d59230d `bus_monitor_reg.h:562-580`:
        // `CORE_0_DEBUG_MODE` at 0x64, `DEBUG_MODULE_ACTIVE` bit 1.
        let mut words = std::vec![0_u32; instances::bus_monitor::SIZE / 4];
        words[0x64 / 4] = 0b01;
        let status = DebugModeStatus::host(&mut words).expect("memory holds the block");
        assert!(!super::debug_module_active(&status), "debug mode alone");
        words[0x64 / 4] = 0b10;
        let status = DebugModeStatus::host(&mut words).expect("memory holds the block");
        assert!(super::debug_module_active(&status));
    }

    #[test]
    fn nothing_is_pending_before_the_bank_is_published() {
        // The host test binary never splits the root's interrupt matrix.
        assert!(!Platform::is_pending(68));
    }
}
