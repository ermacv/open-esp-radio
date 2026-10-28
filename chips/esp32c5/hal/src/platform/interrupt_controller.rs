//! The hart's interrupt controller: its interrupt-matrix bank and its CLIC,
//! which the interrupt table's router (`oer_espressif_interrupt_table::Router`)
//! writes through the typed blocks.
//!
//! Each method of [`Controller`] reads or read-modify-writes one word: a
//! route keeps its source's `PASS_IN_SEC` flag and the reserved bits
//! of its word, a line keeps its pending bit and its privilege mode. A source
//! outside the bank or a line outside the CLIC is refused
//! ([`Error::NoSuchSource`], [`Error::NoSuchLine`]) and nothing is written;
//! it reads as silent or disabled.

use oer_esp32c5_pac::platform::{
    Clic, InterruptCore0, InterruptCore0Status, InterruptMatrixPeripherals, clic, interrupt_core0,
};
use oer_espressif_interrupt_table::{Controller, Error};
use oer_mmio::{Bits, Fixed, Index, Placement};
use oer_riscv::clic::{Control, LevelBits, LineState, Trigger};

#[cfg(test)]
mod tests;

/// The level bits of a line's control byte (`CLIC_INT_CONFIG.MNLBITS`): eight
/// levels.
///
/// SOURCE: ESP-IDF 4d59230d `components/soc/esp32c5/include/soc/clic_reg.h:13`
/// (`NLBITS` 3); esp-hal 1f646c5 `esp-hal/src/interrupt/riscv/clic.rs:8-10`
/// (`init` writes `mnlbits` 3).
pub const LEVEL_BITS: u8 = 3;

/// The interrupt matrix's blocks, taken once from their handoff: hart 0's
/// controller and the pending sources every context reads.
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptBlocks {
    /// The controller of hart 0, the chip's one hart.
    pub controller: InterruptController,
    /// Which sources are pending at hart 0's bank.
    pub pending: InterruptCore0Status,
}

impl InterruptBlocks {
    /// Split the claimed partition; [`InterruptController::new`] sets the
    /// CLIC's level bits.
    pub fn new(peripherals: InterruptMatrixPeripherals) -> Self {
        let InterruptMatrixPeripherals {
            clic,
            interrupt_core0,
            interrupt_core0_status,
        } = peripherals;
        Self {
            controller: InterruptController::new(interrupt_core0, clic),
            pending: interrupt_core0_status,
        }
    }
}

/// Hart 0's interrupt-matrix bank and CLIC, the router's
/// [`Controller`]. A table entry outside its sources or lines fails to
/// compile where the router binds it:
///
/// ```compile_fail,E0080
/// use oer_esp32c5_hal::platform::InterruptController;
/// use oer_espressif_interrupt_table::Router;
/// pub struct FarIrq;
/// impl oer_mmio::InterruptSource for FarIrq {
///     const SOURCE: u16 = 84;
/// }
/// pub fn on_far(_: &mut FarIrq) {}
/// // `Lines` wider than the chip's: lines up to 63.
/// const WIDE: oer_espressif_interrupt_table::Lines =
///     oer_espressif_interrupt_table::Lines { first: 16, count: 64, levels: 8, harts: 1 };
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: WIDE,
///     handler_attributes: [];
///     far: FarToken = FarIrq => on_far, line 60, level 3, hart 0, Hardware;
/// }
/// pub fn bind(router: &mut Router<InterruptController>, token: FarToken) {
///     let _ = router.bind(token, FarIrq);
/// }
/// # fn main() {
/// #     let _: fn(&mut Router<InterruptController>, FarToken) = bind;
/// # }
/// ```
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptController<
    M: Placement = Fixed<{ interrupt_core0::BASE }>,
    C: Placement = Fixed<{ clic::BASE }>,
> {
    matrix: InterruptCore0<M>,
    clic: Clic<C>,
}

impl<M: Placement, C: Placement> InterruptController<M, C> {
    /// Own the bank and the CLIC and give the CLIC [`LEVEL_BITS`] level bits,
    /// keeping its other configuration fields. Every route and line stays as
    /// it is until the router installs the controller.
    pub fn new(matrix: InterruptCore0<M>, mut clic: Clic<C>) -> Self {
        clic.modify_int_config(|config| {
            config.set_mnlbits(Bits::constant::<{ LEVEL_BITS as u32 }>());
        });
        Self { matrix, clic }
    }

    /// The level bits the CLIC holds.
    pub fn level_bits(&self) -> u8 {
        let bits = self.clic.read_int_config().mnlbits().get();
        // MNLBITS is four bits wide.
        bits as u8
    }
}

/// The trigger field (`ATTR_TRIG`) of `trigger`: bit 0 set for an edge, bit
/// 1 set for the negative polarity.
///
/// SOURCE: RISC-V CLIC specification (`clicintattr.trig`); ESP-IDF 4d59230d
/// `components/soc/esp32c5/include/soc/clic_reg.h` (`CLIC_INT_ATTR_TRIG`).
const fn trigger_bits(trigger: Trigger) -> Bits<2> {
    match trigger {
        Trigger::PositiveLevel => Bits::constant::<0b00>(),
        Trigger::PositiveEdge => Bits::constant::<0b01>(),
        Trigger::NegativeLevel => Bits::constant::<0b10>(),
        Trigger::NegativeEdge => Bits::constant::<0b11>(),
    }
}

/// The trigger a line's `ATTR_TRIG` field holds.
const fn trigger_of(bits: Bits<2>) -> Trigger {
    match bits.get() {
        0b00 => Trigger::PositiveLevel,
        0b01 => Trigger::PositiveEdge,
        0b10 => Trigger::NegativeLevel,
        _ => Trigger::NegativeEdge,
    }
}

impl<M: Placement, C: Placement> Controller for InterruptController<M, C> {
    const SOURCES: u16 = interrupt_core0::route::COUNT as u16;
    const LINES: u8 = clic::ctrl::COUNT as u8;
    const LEVEL_BITS: LevelBits = LevelBits::constant::<LEVEL_BITS>();

    fn hart(&self) -> u8 {
        0
    }

    fn route(&mut self, source: u16, line: u8) -> Result<(), Error> {
        let index = Index::new(usize::from(source)).map_err(|_| Error::NoSuchSource { source })?;
        // MAP is wider than the CLIC's lines: a line past them is refused,
        // not written.
        let map = (line < Self::LINES)
            .then(|| Bits::new(u32::from(line)).ok())
            .flatten()
            .ok_or(Error::NoSuchLine { line })?;
        self.matrix.modify_route(index, |route| {
            route.set_map(map);
        });
        Ok(())
    }

    fn silence(&mut self, source: u16) -> Result<(), Error> {
        let index = Index::new(usize::from(source)).map_err(|_| Error::NoSuchSource { source })?;
        self.matrix.modify_route(index, |route| {
            route.set_map(Bits::constant::<0>());
        });
        Ok(())
    }

    fn routed(&self, source: u16) -> Option<u8> {
        let index = Index::new(usize::from(source)).ok()?;
        // MAP is six bits wide.
        let line = self.matrix.read_route(index).map().get() as u8;
        (line != 0).then_some(line)
    }

    /// Three writes of the line's word, each changing one kind of byte: the
    /// enable off, then the attributes and control, then the enable on. The
    /// word holds `clicintie` (bit 8), `clicintattr` (bits 16 to 23) and
    /// `clicintctl` (bits 24 to 31), and a word write takes effect in no
    /// specified byte order, so one write could enable the line before its
    /// new level and trigger hold.
    ///
    /// SOURCE: RISC-V CLIC specification 3f8ee744 `clic.adoc:377` (a word
    /// write's byte updates take effect in no specified order); ESP-IDF
    /// 4d59230d `components/soc/esp32c5/include/soc/clic_reg.h`
    /// `:82,:104,:110` (`CLIC_INT_CTL_S` 24, `CLIC_INT_ATTR_SHV_S` 16,
    /// `CLIC_INT_IE_S` 8).
    fn set_line(&mut self, line: u8, state: LineState) -> Result<(), Error> {
        let index = Index::new(usize::from(line)).map_err(|_| Error::NoSuchLine { line })?;
        if self.clic.read_ctrl(index).ie() {
            self.clic.modify_ctrl(index, |word| {
                word.set_ie(false);
            });
        }
        self.clic.modify_ctrl(index, |word| {
            word.set_attr_shv(state.vectored)
                .set_attr_trig(trigger_bits(state.trigger))
                .set_ctl(state.control.bits());
        });
        if state.enabled {
            self.clic.modify_ctrl(index, |word| {
                word.set_ie(true);
            });
        }
        Ok(())
    }

    fn line(&self, line: u8) -> LineState {
        let Ok(index) = Index::new(usize::from(line)) else {
            return LineState::DISABLED;
        };
        let word = self.clic.read_ctrl(index);
        LineState {
            enabled: word.ie(),
            vectored: word.attr_shv(),
            trigger: trigger_of(word.attr_trig()),
            control: Control::from_bits(word.ctl()),
        }
    }
}
