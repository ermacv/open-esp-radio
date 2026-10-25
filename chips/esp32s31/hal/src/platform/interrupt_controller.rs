//! The hart's interrupt controller: its interrupt-matrix bank and its CLIC,
//! which the interrupt table's router (`oer_interrupt_table::Router`)
//! writes through the typed blocks.
//!
//! Each method of [`Controller`] reads or read-modify-writes one word: a
//! route keeps its source's `PASS_LEVEL` field and the reserved bits
//! of its word, a line keeps its pending bit and its privilege mode. A source
//! outside the bank or a line outside the CLIC is refused
//! ([`Error::NoSuchSource`], [`Error::NoSuchLine`]) and nothing is written;
//! it reads as silent or disabled.

use oer_esp32s31_pac::{
    clic::{self, Clic},
    instances,
    interrupt_core::{
        self, InterruptCore0, InterruptCore0Status, InterruptCore1, InterruptCore1Status,
    },
    ownership::InterruptMatrixPeripherals,
};
use oer_interrupt_table::{Controller, Error, LineState, Trigger};
use oer_memory::zeroed::ZeroedOnce;
use oer_mmio::{Bits, Fixed, Index, Placement};

#[cfg(test)]
mod tests;

/// The level bits of a line's control byte (`CLIC_INT_CONFIG.MNLBITS`): eight
/// levels.
///
/// SOURCE: ESP-IDF 4d59230d `components/soc/esp32s31/include/soc/clic_reg.h:13`
/// (`NLBITS` 3); esp-hal 1f646c5 `esp-hal/src/interrupt/riscv/clic.rs:8-10`
/// (`init` writes `mnlbits` 3).
pub const LEVEL_BITS: u8 = 3;

/// The interrupt matrix's blocks, split from the root: hart 0's controller
/// and hart 1's bank. Hart 0's pending sources are published for every
/// context ([`is_pending`]).
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptBlocks {
    /// The controller of hart 0, the hart stage two runs on.
    pub controller: InterruptController,
    /// Hart 1's bank, which nothing writes while hart 1 runs no code.
    pub hart1: Hart1Interrupts,
}

/// Hart 0's pending sources, published once by [`InterruptBlocks::new`].
static PENDING: ZeroedOnce<InterruptCore0Status> = ZeroedOnce::new();

/// Whether `source` is pending at hart 0's bank, the hart stage two runs
/// on (hart 1 runs no code); `false` before the blocks are split or for a
/// source the bank does not have. Never waits.
pub fn is_pending(source: u16) -> bool {
    PENDING
        .get()
        .is_some_and(|pending| pending_in(pending, source))
}

/// Whether `source` is pending in `status`: source `32 * n + k` is bit `k`
/// of the bank's status word `n`.
///
/// SOURCE: ESP-IDF 4d59230d `interrupt_core0_reg.h`
/// (`INTERRUPT_CORE0_INT_STATUS_REG_0..5`: sources 0 to 168, one bit each).
fn pending_in<P: Placement>(status: &InterruptCore0Status<P>, source: u16) -> bool {
    let word = match source / 32 {
        0 => status.read_int_status_reg_0().int_status_0(),
        1 => status.read_int_status_reg_1().int_status_1(),
        2 => status.read_int_status_reg_2().int_status_2(),
        3 => status.read_int_status_reg_3().int_status_3(),
        4 => status.read_int_status_reg_4().int_status_4(),
        5 => status.read_int_status_reg_5().int_status_5().get(),
        _ => return false,
    };
    word & (1 << (source % 32)) != 0
}

impl InterruptBlocks {
    /// Split the root's partition and publish hart 0's pending sources;
    /// [`InterruptController::new`] sets the CLIC's level bits.
    ///
    /// # Panics
    ///
    /// When the pending sources were published before: the root splits the
    /// partition once.
    pub fn new(peripherals: InterruptMatrixPeripherals) -> Self {
        let InterruptMatrixPeripherals {
            clic,
            interrupt_core0,
            interrupt_core0_status,
            interrupt_core1,
            interrupt_core1_status,
        } = peripherals;
        assert!(
            PENDING.get().is_none(),
            "the root publishes hart 0's pending sources once"
        );
        let _ = PENDING.get_or_init(|| interrupt_core0_status);
        Self {
            controller: InterruptController::new(interrupt_core0, clic),
            hart1: Hart1Interrupts {
                _routes: interrupt_core1,
                _pending: interrupt_core1_status,
            },
        }
    }
}

/// Hart 1's interrupt-matrix bank and its pending sources. Hart 1 runs no
/// code (the staged runtime refuses `cpu1-runs`): nothing writes the bank,
/// and holding it keeps every other owner from it.
#[must_use = "dropping hart 1's bank permanently loses its routes"]
pub struct Hart1Interrupts {
    _routes: InterruptCore1,
    _pending: InterruptCore1Status,
}

/// Hart 0's interrupt-matrix bank and the CLIC hart 0 reaches, the router's
/// [`Controller`].
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptController<
    M: Placement = Fixed<{ instances::interrupt_core0::BASE }>,
    C: Placement = Fixed<{ instances::clic::BASE }>,
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
/// `components/soc/esp32s31/include/soc/clic_reg.h` (`CLIC_INT_ATTR_TRIG`).
const fn trigger_bits(trigger: Trigger) -> clic::ctrl::AttrTrig {
    match trigger {
        Trigger::PositiveLevel => clic::ctrl::AttrTrig::PositiveLevel,
        Trigger::PositiveEdge => clic::ctrl::AttrTrig::PositiveEdge,
        Trigger::NegativeLevel => clic::ctrl::AttrTrig::NegativeLevel,
        Trigger::NegativeEdge => clic::ctrl::AttrTrig::NegativeEdge,
    }
}

const fn trigger_of(bits: Bits<2>) -> Trigger {
    match clic::ctrl::AttrTrig::from_field(bits.get()) {
        clic::ctrl::AttrTrig::PositiveLevel => Trigger::PositiveLevel,
        clic::ctrl::AttrTrig::PositiveEdge => Trigger::PositiveEdge,
        clic::ctrl::AttrTrig::NegativeLevel => Trigger::NegativeLevel,
        clic::ctrl::AttrTrig::NegativeEdge => Trigger::NegativeEdge,
    }
}

impl<M: Placement, C: Placement> Controller for InterruptController<M, C> {
    const SOURCES: u16 = interrupt_core::route::COUNT as u16;
    const LINES: u8 = clic::ctrl::COUNT as u8;
    const LEVEL_BITS: u8 = LEVEL_BITS;

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
            route.set_map(Bits::truncate(
                interrupt_core::route::Map::Disconnected.bits(),
            ));
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
    /// 4d59230d `components/soc/esp32s31/include/soc/clic_reg.h`
    /// `:77,:98,:104` (`CLIC_INT_CTL_S` 24, `CLIC_INT_ATTR_SHV_S` 16,
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
                .set_ctl(state.control);
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
            control: word.ctl(),
        }
    }
}
