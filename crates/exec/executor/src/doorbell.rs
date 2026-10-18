//! The radio doorbell: a software interrupt at the radio interrupts' level.
//!
//! The radio executor (P2) cannot write the registers the radio interrupts
//! (P3) own without sharing them with a handler that preempts it. It records
//! a request in memory both sides see and rings the doorbell; the doorbell's
//! handler runs at P3, after or before the radio handlers but never inside
//! one, and applies the request. [`SameLevel`] proves when the image compiles
//! that the doorbell's entry has the radio interrupt's level and core.
//!
//! The doorbell consumes its software interrupt, so no executor can be woken
//! through the same line. Its handler, named in the image's interrupt table,
//! first calls [`acknowledge`], then takes the requests: a ring after that
//! raises the interrupt again and is not lost.

use core::sync::atomic::{Ordering, fence};

use esp_hal::{
    interrupt::{Priority, software::SoftwareInterrupt},
    peripherals::Interrupt,
    system::Cpu,
};
use oer_exec_interrupt_table::Entry;

use crate::executor::{SameLevel, software_interrupt, software_interrupt_source};

/// The radio doorbell on software interrupt `SWI` (`FROM_CPU_INTR<SWI>`).
pub struct Doorbell<const SWI: u8> {
    interrupt: SoftwareInterrupt<'static, SWI>,
}

impl<const SWI: u8> Doorbell<SWI> {
    /// The doorbell on `interrupt`, whose source `bell` is the token of;
    /// `level` proves that it shares the radio interrupt's level and core.
    /// Resets a stale raise, then routes the source to its table level.
    ///
    /// # Errors
    ///
    /// The interrupt table does not list the source with the token's level
    /// and core, or the caller runs on another core than the table's; the
    /// source stays silent.
    pub fn new<Radio, Bell>(
        interrupt: SoftwareInterrupt<'static, SWI>,
        bell: Bell,
        level: SameLevel<Radio, Bell>,
    ) -> Result<Self, oer_exec_esp_hal_irq::Error>
    where
        Radio: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
        Bell: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    {
        const {
            assert!(
                Bell::SOURCE as u16 == software_interrupt_source(SWI) as u16,
                "the token is not the doorbell's software interrupt"
            )
        };
        let _ = level;
        interrupt.reset();
        oer_exec_esp_hal_irq::enable(&bell)?;
        Ok(Self { interrupt })
    }

    /// Raise the doorbell. The fence orders the caller's request writes
    /// before the raise; esp-hal's raise on ESP32-S31 reads the register back,
    /// so the raise has reached the interrupt matrix when this returns.
    /// Never waits.
    pub fn ring(&self) {
        fence(Ordering::SeqCst);
        self.interrupt.raise();
    }
}

/// Reset doorbell `SWI`'s software interrupt: the first step of its handler,
/// before it takes the requests.
#[inline(always)]
pub fn acknowledge<const SWI: u8>() {
    software_interrupt(SWI, false);
}
