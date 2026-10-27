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
//!
//! A fatal path observes hart 0's routes and lines as they are
//! ([`fatal_view`]): the root publishes the observers of the route and line
//! words once, and only a fatal path that stopped the other hart
//! ([`StopTheWorld`]) reads them, without writing.

use oer_esp32s31_pac::{
    clic::{self, Clic, ClicObserver},
    instances,
    interrupt_core::{self, InterruptCore0, InterruptCore0Observer, InterruptCore1},
    ownership::InterruptMatrixPeripherals,
};
use oer_interrupt_table::{Controller, Error, LineState, Trigger};
use oer_memory::{ClaimCell, zeroed::ZeroedOnce};

use crate::cores::{StopTheWorld, key};
use oer_mmio::{Bits, Fixed, Index, Placement};

/// Controller geometry generated from the chip facts, shared with runtime
/// vector generation. Lines below `LINES.first` are reserved core-local inputs.
pub mod facts {
    include!(concat!(env!("OUT_DIR"), "/interrupts.rs"));
}

/// The interrupt matrix's blocks, split from the root: hart 0's controller
/// and hart 1's bank. Every hart's pending sources are published for every
/// context ([`is_pending`]).
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptBlocks {
    /// The controller of hart 0, the hart stage two runs on.
    pub controller: InterruptController,
    /// Hart 1's bank, which nothing writes while hart 1 runs no code.
    pub hart1: Hart1Interrupts,
}

/// Every hart's read-only pending sources, published once at partition split.
static PENDING: ZeroedOnce<PendingBanks> = ZeroedOnce::new();

/// Whether `source` is pending at the checked hart's bank;
/// `false` before the blocks are split or for a
/// source the bank does not have. Never waits.
pub fn is_pending(hart: crate::platform::Hart, source: u16) -> bool {
    PENDING
        .get()
        .is_some_and(|pending| pending.is_pending(hart, source))
}

trait PendingStatus {
    fn word(&self, word: u16) -> Option<u32>;
}

macro_rules! pending_status {
    ($status:ident) => {
        impl<P: Placement> PendingStatus for oer_esp32s31_pac::interrupt_core::$status<P> {
            fn word(&self, word: u16) -> Option<u32> {
                Some(match word {
                    0 => self.read_int_status_reg_0().int_status_0(),
                    1 => self.read_int_status_reg_1().int_status_1(),
                    2 => self.read_int_status_reg_2().int_status_2(),
                    3 => self.read_int_status_reg_3().int_status_3(),
                    4 => self.read_int_status_reg_4().int_status_4(),
                    5 => self.read_int_status_reg_5().int_status_5().get(),
                    _ => return None,
                })
            }
        }
    };
}

include!(concat!(env!("OUT_DIR"), "/pending.rs"));

#[cfg(test)]
mod tests;

/// Whether `source` is pending in `status`: source `32 * n + k` is bit `k`
/// of the bank's status word `n`.
///
/// SOURCE: ESP-IDF 4d59230d `interrupt_core0_reg.h`
/// (`INTERRUPT_CORE0_INT_STATUS_REG_0..5`: sources 0 to 168, one bit each).
fn pending_in(status: &impl PendingStatus, source: u16) -> bool {
    status
        .word(source / 32)
        .is_some_and(|word| word & (1 << (source % 32)) != 0)
}

impl InterruptBlocks {
    /// Split the root's partition: give the CLIC [`facts::LEVEL_BITS`] level bits,
    /// keeping its other configuration fields, and publish every hart's pending
    /// sources and the observers of its routes and lines for a fatal path.
    /// Every route and line stays as it is until the router installs the
    /// controller.
    ///
    /// # Panics
    ///
    /// When the pending sources were published before: the root splits the
    /// partition once.
    pub fn new(peripherals: InterruptMatrixPeripherals) -> Self {
        let pending = take_pending!(peripherals);
        let InterruptMatrixPeripherals {
            clic,
            mut clic_observer,
            interrupt_core0,
            interrupt_core0_observer,
            interrupt_core1,
            ..
        } = peripherals;
        assert!(
            PENDING.get().is_none(),
            "the root publishes the pending sources once"
        );
        let _ = PENDING.get_or_init(|| pending);
        set_level_bits(&mut clic_observer);
        // Published beside the pending sources, which the assertion above
        // finds unpublished: the observers are not installed either.
        let _installed = OBSERVER.install(Observer {
            routes: interrupt_core0_observer,
            lines: clic_observer,
        });
        Self {
            controller: InterruptController::new(interrupt_core0, clic),
            hart1: Hart1Interrupts {
                _routes: interrupt_core1,
            },
        }
    }
}

/// Give the CLIC [`facts::LEVEL_BITS`] level bits, keeping its other configuration
/// fields.
fn set_level_bits<C: Placement>(config: &mut ClicObserver<C>) {
    config.modify_int_config(|config| {
        config.set_mnlbits(Bits::constant::<{ facts::LEVEL_BITS as u32 }>());
    });
}

/// The level bits the CLIC holds.
fn level_bits<C: Placement>(config: &ClicObserver<C>) -> u8 {
    // MNLBITS is four bits wide.
    config.read_int_config().mnlbits().get() as u8
}

/// Hart 0's route and line observers, published once by
/// [`InterruptBlocks::new`] and held for good by the first fatal path that
/// reads them.
static OBSERVER: ClaimCell<Observer> = ClaimCell::new();

/// The observers of hart 0's route words and of the CLIC's line words.
struct Observer<
    M: Placement = Fixed<{ instances::interrupt_core0::BASE }>,
    C: Placement = Fixed<{ instances::clic::BASE }>,
> {
    routes: InterruptCore0Observer<M>,
    lines: ClicObserver<C>,
}

/// Hart 0's routes and lines as a fatal path reads them: what the hardware
/// holds, not what the image's table declares.
pub struct InterruptView<
    'a,
    M: Placement = Fixed<{ instances::interrupt_core0::BASE }>,
    C: Placement = Fixed<{ instances::clic::BASE }>,
> {
    observer: &'a Observer<M, C>,
}

/// One CLIC line as a fatal path reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineObservation {
    /// The line's pending bit (`clicintip`).
    pub pending: bool,
    /// Its enable, vectoring, trigger and control byte.
    pub state: LineState,
}

impl<M: Placement, C: Placement> InterruptView<'_, M, C> {
    /// The line `source` is routed to; `None` when it is routed nowhere or
    /// is no source of the bank.
    pub fn routed(&self, source: u16) -> Option<u8> {
        let index = Index::new(usize::from(source)).ok()?;
        // MAP is six bits wide.
        let line = self.observer.routes.read_route(index).map().get() as u8;
        (line != 0).then_some(line)
    }

    /// The state of `line`; `None` for a line past the CLIC's.
    pub fn line(&self, line: u8) -> Option<LineObservation> {
        let index = Index::new(usize::from(line)).ok()?;
        let word = self.observer.lines.read_ctrl(index);
        Some(LineObservation {
            pending: word.ip(),
            state: LineState {
                enabled: word.ie(),
                vectored: word.attr_shv(),
                trigger: trigger_of(word.attr_trig()),
                control: word.ctl(),
            },
        })
    }

    /// The level bits the CLIC holds.
    pub fn level_bits(&self) -> u8 {
        level_bits(&self.observer.lines)
    }
}

/// Read hart 0's routes and lines in `stop`'s fatal path: the other hart
/// stopped and the image's router writes no more, so the words hold what
/// the hardware dispatches. The view only reads. `None` before the root
/// published the observers, or when another hart's fatal path holds them.
pub fn fatal_view<R>(
    stop: &StopTheWorld<'_>,
    observe: impl FnOnce(&InterruptView<'_>) -> R,
) -> Option<R> {
    OBSERVER
        .hold(key(stop.hart()), |observer| {
            observe(&InterruptView { observer })
        })
        .ok()
        .map(|held| held.value)
}

/// Hart 1's interrupt-matrix route bank. Its read-only pending sources are
/// published with every hart's status. Hart 1 runs no
/// code (the staged runtime refuses `cpu1-runs`): nothing writes the bank,
/// and holding it keeps every other owner from it.
#[must_use = "dropping hart 1's bank permanently loses its routes"]
pub struct Hart1Interrupts {
    _routes: InterruptCore1,
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
    /// Own the bank and the CLIC's lines. Every route and line stays as it
    /// is until the router installs the controller.
    pub const fn new(matrix: InterruptCore0<M>, clic: Clic<C>) -> Self {
        Self { matrix, clic }
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
    const LEVEL_BITS: u8 = facts::LEVEL_BITS;

    fn hart(&self) -> u8 {
        crate::platform::BOOT_HART.get() as u8
    }

    fn verify_configuration(&self) -> Result<(), Error> {
        const { assert!(Self::LINES == facts::LINES.count) };
        let info = self.clic.read_int_info();
        // These fields fit their destination types (four, thirteen and four bits).
        let control_bits = info.ctlbits().get() as u8;
        let lines = info.num_int().get() as u16;
        let level_bits = self.clic.read_int_config().mnlbits().get() as u8;
        if control_bits != facts::CONTROL_BITS {
            return Err(Error::WrongControlBits {
                actual: control_bits,
                expected: facts::CONTROL_BITS,
            });
        }
        if lines != u16::from(facts::LINES.count) {
            return Err(Error::WrongInterruptCount {
                actual: lines,
                expected: u16::from(facts::LINES.count),
            });
        }
        if level_bits != facts::LEVEL_BITS {
            return Err(Error::WrongLevelBits {
                actual: level_bits,
                expected: facts::LEVEL_BITS,
            });
        }
        Ok(())
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
