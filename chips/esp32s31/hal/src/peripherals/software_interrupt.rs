//! The four software interrupt lines (`FROM_CPU_INTR0..3`): each line's
//! owner raises and lowers it from any context, and its interrupt part
//! names the line's source for the interrupt table's entry.
//!
//! A line is a shared part: its one word is stored whole and read without
//! a side effect, so every context reaches it through `&self`. The
//! interrupt part holds no register: the handler lowers the line through
//! the line's owner, which the executor or the doorbell publishes.

use oer_esp32s31_pac::{
    hp_system, instances,
    ownership::{
        SoftwareInterrupt0Peripherals, SoftwareInterrupt1Peripherals,
        SoftwareInterrupt2Peripherals, SoftwareInterrupt3Peripherals,
    },
};
use oer_mmio::{Fixed, InterruptSource, Placement};

#[cfg(test)]
mod tests;

macro_rules! software_interrupt {
    (
        $(#[$doc:meta])*
        $line:ident, $irq:ident, $part:ident, $partition:ident, $member:ident, $read:ident, $store:ident, $field:ident
    ) => {
        $(#[$doc])*
        #[must_use = "dropping a register owner permanently loses its register authority"]
        pub struct $line<P: Placement = Fixed<{ instances::hp_sys::BASE }>>(hp_system::$part<P>);

        /// The line's interrupt part: it names the line's source and holds
        /// no register. The table's entry moves it into the handler.
        #[must_use = "the interrupt table binds the line through its part"]
        pub struct $irq(());

        impl InterruptSource for $irq {
            const SOURCE: u16 = <hp_system::$part as InterruptSource>::SOURCE;
        }

        impl $line {
            /// Split the root's partition into the line and its interrupt
            /// part.
            pub fn new(peripherals: $partition) -> (Self, $irq) {
                (Self(peripherals.$member), $irq(()))
            }
        }

        impl<P: Placement> $line<P> {
            /// Raise the line, then read it back, so the raise has reached
            /// the interrupt matrix when this returns.
            ///
            /// SOURCE: esp-hal 1f646c5 `interrupt/software.rs` (`raise`: the
            /// store and its read back).
            #[inline(always)]
            pub fn raise(&self) {
                self.0.$store(true);
                let _ = self.0.$read().$field();
            }

            /// Lower the line.
            #[inline(always)]
            pub fn lower(&self) {
                self.0.$store(false);
            }

            /// Whether the line is raised.
            #[inline(always)]
            pub fn raised(&self) -> bool {
                self.0.$read().$field()
            }
        }

        impl<P: Placement> oer_espressif_platform_port::SoftwareLine for $line<P> {
            #[inline(always)]
            fn raise(&self) {
                Self::raise(self);
            }

            #[inline(always)]
            fn lower(&self) {
                Self::lower(self);
            }
        }

        #[cfg(any(test, feature = "test-support"))]
        impl<'a> $line<oer_mmio::Host<'a>> {
            /// The line over host memory from the instance's base.
            pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
                hp_system::$part::host(words).map(Self)
            }
        }
    };
}

software_interrupt!(
    /// Software interrupt line 0.
    FromCpu0, FromCpu0Irq, FromCpu0, SoftwareInterrupt0Peripherals, from_cpu0,
    read_hp_cpu_int_from_cpu_0, store_hp_cpu_int_from_cpu_0, hp_cpu_int_from_cpu_0
);
software_interrupt!(
    /// Software interrupt line 1.
    FromCpu1, FromCpu1Irq, FromCpu1, SoftwareInterrupt1Peripherals, from_cpu1,
    read_hp_cpu_int_from_cpu_1, store_hp_cpu_int_from_cpu_1, hp_cpu_int_from_cpu_1
);
software_interrupt!(
    /// Software interrupt line 2.
    FromCpu2, FromCpu2Irq, FromCpu2, SoftwareInterrupt2Peripherals, from_cpu2,
    read_hp_cpu_int_from_cpu_2, store_hp_cpu_int_from_cpu_2, hp_cpu_int_from_cpu_2
);
software_interrupt!(
    /// Software interrupt line 3.
    FromCpu3, FromCpu3Irq, FromCpu3, SoftwareInterrupt3Peripherals, from_cpu3,
    read_hp_cpu_int_from_cpu_3, store_hp_cpu_int_from_cpu_3, hp_cpu_int_from_cpu_3
);

/// The four lines and their interrupt parts, split from the root.
pub struct SoftwareInterrupts {
    pub line0: FromCpu0,
    pub irq0: FromCpu0Irq,
    pub line1: FromCpu1,
    pub irq1: FromCpu1Irq,
    pub line2: FromCpu2,
    pub irq2: FromCpu2Irq,
    pub line3: FromCpu3,
    pub irq3: FromCpu3Irq,
}

impl SoftwareInterrupts {
    /// Split the root's four software interrupt partitions.
    pub fn new(
        zero: SoftwareInterrupt0Peripherals,
        one: SoftwareInterrupt1Peripherals,
        two: SoftwareInterrupt2Peripherals,
        three: SoftwareInterrupt3Peripherals,
    ) -> Self {
        let (line0, irq0) = FromCpu0::new(zero);
        let (line1, irq1) = FromCpu1::new(one);
        let (line2, irq2) = FromCpu2::new(two);
        let (line3, irq3) = FromCpu3::new(three);
        Self {
            line0,
            irq0,
            line1,
            irq1,
            line2,
            irq2,
            line3,
            irq3,
        }
    }
}
