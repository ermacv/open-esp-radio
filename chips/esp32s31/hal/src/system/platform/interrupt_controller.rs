//! Each hart's interrupt controller: its interrupt-matrix bank and its own
//! CLIC, which the interrupt table's router
//! (`oer_chips_family_platform_interrupt_table::Router`) writes through the typed blocks.
//!
//! The CLIC is context-relative: every hart reaches its own controller at
//! the same address (SOURCE: `HW1_S31_CLIC_20261026` and
//! `ESP_IDF_4D59230D_S31_CLIC`), and the PAC claims its parts once per hart.
//! The root pairs each hart's CLIC parts with that hart's route bank in one
//! [`InterruptGrant`], once per hart: hart 0's in [`InterruptBlocks`], hart
//! 1's in the secondary launch permit, which the launch hands to hart 1.
//! A grant is inert: only [`InterruptGrant::into_controller`], on the
//! grant's own hart, makes its [`InterruptController`], which cannot leave
//! that hart (`!Send`). The staged runtime calls it in each hart's handoff,
//! so each hart has one controller and one router.
//!
//! Each method of [`Controller`] reads or read-modify-writes one register: a
//! route keeps its source's `PASS_LEVEL` field and the reserved bits of its
//! word; a line is four byte registers (pending, enable, attribute and
//! control: SOURCE `ESP_IDF_4D59230D_S31_CLIC` lines 113-160 and
//! `HW_S31_CLIC_LINE_BYTES_20261119`), each changed by its own byte access,
//! so a change of one never writes another, and the attribute byte keeps
//! its privilege mode. A source
//! and a line are the controller's own values ([`Source`], [`Line`]), made
//! for each of the bank's sources and the CLIC's lines in a constant, so no
//! write converts a number or refuses one.
//!
//! A fatal path of hart 0 observes hart 0's routes and lines as they are
//! ([`fatal_view`]): the root publishes the observers of the route and line
//! words once, and only a fatal path that stopped the other hart
//! ([`StopTheWorld`]) reads them, without writing. Hart 1's routes have no
//! claimed observer and its CLIC is reachable only from hart 1, so a fatal
//! path of hart 1 gets no view.

use core::marker::PhantomData;

use oer_chips_esp32s31_pac::{
    clic::{self, Clic, ClicObserver},
    instances,
    interrupt_core::{self, InterruptCore0, InterruptCore0Observer, InterruptCore1},
};
use oer_chips_family_platform_interrupt_table::{
    Controller, Error, LineExpectation, LineObservation,
};

/// A source of the interrupt matrix: its route word's index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Source(Index<{ interrupt_core::route::COUNT }>);

/// A line of the hart's CLIC: the index of its four byte registers, and its
/// number as a route's `MAP` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Line {
    ctrl: Index<{ clic::ip::COUNT }>,
    map: Bits<6>,
}

/// Every source, source `n` at `n`.
const SOURCES: [Source; interrupt_core::route::COUNT] = {
    let mut sources = [Source(Index::constant::<0>()); interrupt_core::route::COUNT];
    let mut rest: &mut [Source] = &mut sources;
    let mut number = 0;
    while let [source, tail @ ..] = rest {
        *source = match Index::new(number) {
            Ok(index) => Source(index),
            Err(_) => panic!("the source lies inside the matrix"),
        };
        rest = tail;
        number += 1;
    }
    sources
};

/// Every line, line `n` at `n`; a route's `MAP` holds each line's number.
const LINES: [Line; clic::ip::COUNT] = {
    let mut lines = [Line {
        ctrl: Index::constant::<0>(),
        map: Bits::constant::<0>(),
    }; clic::ip::COUNT];
    let mut rest: &mut [Line] = &mut lines;
    let (mut number, mut map) = (0, 0);
    while let [line, tail @ ..] = rest {
        *line = match (Index::new(number), Bits::new(map)) {
            (Ok(ctrl), Ok(map)) => Line { ctrl, map },
            _ => panic!("the line lies inside the CLIC and its number in MAP"),
        };
        rest = tail;
        number += 1;
        map += 1;
    }
    lines
};
use oer_portable_memory::{ClaimCell, zeroed::ZeroedOnce};
use oer_portable_riscv::clic::{Control, LevelBits};

use crate::system::cores::{StopTheWorld, key};
use oer_portable_mmio::{Bits, Fixed, Index, Placement};

/// Controller geometry generated from the chip facts, shared with runtime
/// vector generation. Lines below `LINES.first` are reserved core-local inputs.
pub mod facts {
    include!(concat!(env!("OUT_DIR"), "/interrupts.rs"));
}

/// The interrupt matrix's blocks, split from the root: hart 0's grant.
/// Hart 1's route bank and CLIC parts belong to the root's secondary launch
/// permit ([`LaunchPermit`](crate::system::cores::LaunchPermit)). Every hart's
/// pending sources are published for every context ([`is_pending`]).
#[must_use = "dropping the interrupt grant permanently loses the routes and lines"]
pub struct InterruptBlocks {
    /// The grant of hart 0, the hart stage two runs on.
    pub grant: InterruptGrant<InterruptCore0>,
}

/// The interrupt matrix's parts as the root splits them: hart 0's route
/// bank and route observer, hart 0's CLIC parts and every hart's pending
/// sources. Hart 1's CLIC parts go to the launch permit.
pub(crate) struct MatrixParts {
    pub(crate) clic: Clic,
    pub(crate) clic_observer: ClicObserver,
    pub(crate) interrupt_core0: InterruptCore0,
    pub(crate) interrupt_core0_observer: InterruptCore0Observer,
    pub(crate) interrupt_core0_status: oer_chips_esp32s31_pac::interrupt_core::InterruptCore0Status,
    pub(crate) interrupt_core1_status: oer_chips_esp32s31_pac::interrupt_core::InterruptCore1Status,
}

/// Every hart's read-only pending sources, published once at partition split.
static PENDING: ZeroedOnce<PendingBanks> = ZeroedOnce::new();

/// Whether `source` is pending at the checked hart's bank;
/// `false` before the blocks are split or for a
/// source the bank does not have. Never waits.
pub fn is_pending(hart: crate::system::platform::Hart, source: u16) -> bool {
    PENDING
        .get()
        .is_some_and(|pending| pending.is_pending(hart, source))
}

trait PendingStatus {
    fn word(&self, word: u16) -> Option<u32>;
}

macro_rules! pending_status {
    ($status:ident) => {
        impl<P: Placement> PendingStatus for oer_chips_esp32s31_pac::interrupt_core::$status<P> {
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
/// SOURCE: ESP-IDF 4d80c1b2 `interrupt_core0_reg.h`
/// (`INTERRUPT_CORE0_INT_STATUS_REG_0..5`: sources 0 to 168, one bit each).
fn pending_in(status: &impl PendingStatus, source: u16) -> bool {
    status
        .word(source / 32)
        .is_some_and(|word| word & (1 << (source % 32)) != 0)
}

impl InterruptBlocks {
    /// Split the root's partition on the boot hart, the only hart stage two
    /// runs on before a launch: give hart 0's CLIC [`facts::LEVEL_BITS`]
    /// level bits, keeping its other configuration fields, and publish every
    /// hart's pending sources and the observers of hart 0's routes and lines
    /// for a fatal path. Every route and line stays as it is until the
    /// router installs the controller.
    pub(crate) fn new(peripherals: MatrixParts) -> Self {
        let pending = take_pending!(peripherals);
        let MatrixParts {
            clic,
            mut clic_observer,
            interrupt_core0,
            interrupt_core0_observer,
            ..
        } = peripherals;
        // The root splits this partition once, so nothing is published yet;
        // a refused second value would name the same banks as the first.
        let _published = PENDING.publish(pending);
        set_level_bits(&mut clic_observer);
        // Published beside the pending sources, as once: the observers are
        // not installed either.
        let _installed = OBSERVER.install(Observer {
            routes: interrupt_core0_observer,
            lines: clic_observer,
        });
        Self {
            grant: InterruptGrant::new(interrupt_core0, clic, None),
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

impl<M: Placement, C: Placement> InterruptView<'_, M, C> {
    /// The line `source` is routed to; `None` when it is routed nowhere or
    /// is no source of the bank.
    pub fn routed(&self, source: u16) -> Option<u8> {
        let index = Index::new(usize::from(source)).ok()?;
        // MAP is six bits wide.
        let line = self.observer.routes.read_route(index).map().get() as u8;
        (line != 0).then_some(line)
    }

    /// The word of `line`, for the fatal path's record: its pending,
    /// enable, attribute and control bytes as the hardware has them; `None`
    /// for a line past the CLIC's.
    pub fn line(&self, line: u8) -> Option<u32> {
        let index = Index::new(usize::from(line)).ok()?;
        Some(line_record(&self.observer.lines, index).record())
    }

    /// The level bits the CLIC holds.
    pub fn level_bits(&self) -> u8 {
        level_bits(&self.observer.lines)
    }
}

/// Read hart 0's routes and lines in hart 0's fatal path (`stop`): the
/// other hart stopped and the image's router writes no more, so the words
/// hold what the hardware dispatches. The view only reads. `None` before
/// the root published the observers, when another hart's fatal path holds
/// them, or in a fatal path of another hart: hart 0's line words are
/// reachable only from hart 0, and no other hart's route observer is
/// claimed.
pub fn fatal_view<R>(
    stop: &StopTheWorld<'_>,
    observe: impl FnOnce(&InterruptView<'_>) -> R,
) -> Option<R> {
    if stop.hart() != crate::system::platform::BOOT_HART {
        return None;
    }
    OBSERVER
        .hold(key(stop.hart()), |observer| {
            observe(&InterruptView { observer })
        })
        .ok()
        .map(|held| held.value)
}

/// A hart's route bank in the interrupt matrix: `INTERRUPT_CORE0` for hart
/// 0, `INTERRUPT_CORE1` for hart 1. Sealed: only those banks are.
pub trait RouteBank: sealed::Sealed {
    /// The hart whose routes the bank holds.
    const HART: u8;
    #[doc(hidden)]
    fn read_route(
        &self,
        index: Index<{ interrupt_core::route::COUNT }>,
    ) -> interrupt_core::route::R;
    #[doc(hidden)]
    fn modify_route(
        &mut self,
        index: Index<{ interrupt_core::route::COUNT }>,
        change: impl FnOnce(&mut interrupt_core::route::W),
    );
}

mod sealed {
    pub trait Sealed {}
    impl<M: oer_portable_mmio::Placement> Sealed for super::InterruptCore0<M> {}
    impl<M: oer_portable_mmio::Placement> Sealed for super::InterruptCore1<M> {}
}

macro_rules! route_bank {
    ($bank:ident, $hart:literal) => {
        impl<M: Placement> RouteBank for $bank<M> {
            const HART: u8 = $hart;
            fn read_route(
                &self,
                index: Index<{ interrupt_core::route::COUNT }>,
            ) -> interrupt_core::route::R {
                $bank::read_route(self, index)
            }
            fn modify_route(
                &mut self,
                index: Index<{ interrupt_core::route::COUNT }>,
                change: impl FnOnce(&mut interrupt_core::route::W),
            ) {
                $bank::modify_route(self, index, change);
            }
        }
    };
}

route_bank!(InterruptCore0, 0);
route_bank!(InterruptCore1, 1);

/// One hart's interrupt grant: its route bank and its own CLIC's parts,
/// which the root pairs once per hart. Inert: it reads and writes nothing
/// until [`into_controller`](Self::into_controller) runs on its hart.
#[must_use = "dropping the grant permanently loses the hart's routes and lines"]
pub struct InterruptGrant<B: RouteBank, C: Placement = Fixed<{ instances::clic::BASE }>> {
    bank: B,
    lines: Clic<C>,
    /// The hart's level-bits writer, when its controller sets them: hart
    /// 1's. Hart 0's are set when the root splits the partition, where its
    /// observer is published for hart 0's fatal path.
    config: Option<ClicObserver<C>>,
}

pub use oer_chips_family_platform_port::ForeignHart;

impl<B: RouteBank, C: Placement> InterruptGrant<B, C> {
    /// The root's pairing of a hart's bank with that hart's CLIC parts.
    pub(crate) const fn new(bank: B, lines: Clic<C>, config: Option<ClicObserver<C>>) -> Self {
        Self {
            bank,
            lines,
            config,
        }
    }

    /// The grant's hart.
    pub const fn hart(&self) -> u8 {
        B::HART
    }

    /// The grant's bank, before its hart's controller exists: the launch
    /// disconnects the bank's routes before it releases the hart.
    pub(crate) fn bank(&mut self) -> &mut B {
        &mut self.bank
    }

    /// Make the hart's controller on the hart itself: give its CLIC
    /// [`facts::LEVEL_BITS`] level bits when the grant holds its writer, and
    /// own the bank and the CLIC's lines. Every route and line stays as it
    /// is until the router installs the controller.
    ///
    /// # Errors
    ///
    /// [`ForeignHart`] on another hart than the grant's: nothing is written
    /// and the grant is lost (each hart has one).
    #[cfg(target_arch = "riscv32")]
    pub fn into_controller(self) -> Result<InterruptController<B, C>, ForeignHart> {
        self.into_controller_on(oer_portable_riscv::hart_id())
    }

    /// [`into_controller`](Self::into_controller) on hart `current`.
    pub(crate) fn into_controller_on(
        self,
        current: usize,
    ) -> Result<InterruptController<B, C>, ForeignHart> {
        let expected = usize::from(B::HART);
        if current != expected {
            return Err(ForeignHart {
                actual: current,
                expected,
            });
        }
        let Self {
            bank,
            lines,
            mut config,
        } = self;
        if let Some(config) = config.as_mut() {
            set_level_bits(config);
        }
        Ok(InterruptController {
            matrix: bank,
            clic: lines,
            _config: config,
            _local: PhantomData,
        })
    }
}

/// One hart's interrupt-matrix bank and that hart's own CLIC, the router's
/// [`Controller`]: hart 0's by default ([`InterruptCore0`]), hart 1's over
/// [`InterruptCore1`] ([`Hart1Controller`]). It stays on its hart
/// (`!Send`).
///
/// ```compile_fail
/// fn send<T: Send>() {}
/// send::<oer_chips_esp32s31_hal::system::platform::InterruptController>();
/// ```
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptController<
    B: RouteBank = InterruptCore0,
    C: Placement = Fixed<{ instances::clic::BASE }>,
> {
    matrix: B,
    clic: Clic<C>,
    /// Hart 1's level-bits writer, retained once it set them.
    _config: Option<ClicObserver<C>>,
    /// The CLIC words reach the running hart's own controller.
    _local: PhantomData<*mut ()>,
}

/// Hart 1's controller.
pub type Hart1Controller = InterruptController<InterruptCore1>;
/// Hart 0's grant.
pub type Hart0Grant = InterruptGrant<InterruptCore0>;
/// Hart 1's grant.
pub type Hart1Grant = InterruptGrant<InterruptCore1>;

impl<B: RouteBank, C: Placement> Controller for InterruptController<B, C> {
    type Source = Source;
    type Line = Line;
    const SOURCES: &'static [Source] = &SOURCES;
    const LINES: &'static [Line] = &LINES;
    const LEVEL_BITS: LevelBits = LevelBits::constant::<{ facts::LEVEL_BITS }>();

    fn hart(&self) -> u8 {
        B::HART
    }

    fn verify_configuration(&self) -> Result<(), Error> {
        const { assert!(clic::ip::COUNT == facts::LINES.count as usize) };
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

    fn route(&mut self, source: Source, line: Line) {
        RouteBank::modify_route(&mut self.matrix, source.0, |route| {
            route.set_map(line.map);
        });
    }

    fn silence(&mut self, source: Source) {
        RouteBank::modify_route(&mut self.matrix, source.0, |route| {
            route.set_map(Bits::truncate(
                interrupt_core::route::Map::Disconnected.bits(),
            ));
        });
    }

    fn routed(&self, source: Source) -> Option<u8> {
        // MAP is six bits wide.
        let line = RouteBank::read_route(&self.matrix, source.0).map().get() as u8;
        (line != 0).then_some(line)
    }

    fn bind(&mut self, line: Line, control: Control) {
        self.write_line(line, LineWrite::Bind(control));
    }

    fn disable(&mut self, line: Line) {
        self.write_line(line, LineWrite::Disable);
    }

    fn check(&self, line: Line, expected: LineExpectation) -> Result<(), LineObservation> {
        let index = line.ctrl;
        let enabled = self.clic.read_ie(index).ie();
        let holds = match expected {
            LineExpectation::Disabled => !enabled,
            LineExpectation::Bound(control) => {
                let attributes = self.clic.read_attr(index);
                enabled
                    && attributes.shv()
                    && attributes.trig_variant() == clic::attr::Trig::PositiveLevel
                    && self.clic.read_ctl(index).ctl() == control.bits()
            }
        };
        if holds {
            Ok(())
        } else {
            Err(line_record(&self.clic, index))
        }
    }
}

/// What a line write leaves: the line disabled, or bound at a control byte.
#[derive(Clone, Copy)]
enum LineWrite {
    Disable,
    Bind(Control),
}

/// A line's four byte registers as they read, for a record
/// ([`LineObservation::of_bytes`]).
fn line_record<W: LineBytes>(clic: &W, index: Index<{ clic::ip::COUNT }>) -> LineObservation {
    let byte = |record: u32| record.to_le_bytes()[0];
    LineObservation::of_bytes([
        byte(clic.pending(index)),
        byte(clic.enable(index)),
        byte(clic.attributes(index)),
        byte(clic.control(index)),
    ])
}

/// The record words of a line's four byte registers, as the router's part
/// and the fatal path's observer both read them.
trait LineBytes {
    fn pending(&self, index: Index<{ clic::ip::COUNT }>) -> u32;
    fn enable(&self, index: Index<{ clic::ip::COUNT }>) -> u32;
    fn attributes(&self, index: Index<{ clic::ip::COUNT }>) -> u32;
    fn control(&self, index: Index<{ clic::ip::COUNT }>) -> u32;
}

macro_rules! line_bytes {
    ($part:ident) => {
        impl<P: Placement> LineBytes for $part<P> {
            fn pending(&self, index: Index<{ clic::ip::COUNT }>) -> u32 {
                self.read_ip(index).record_word()
            }
            fn enable(&self, index: Index<{ clic::ip::COUNT }>) -> u32 {
                self.read_ie(index).record_word()
            }
            fn attributes(&self, index: Index<{ clic::ip::COUNT }>) -> u32 {
                self.read_attr(index).record_word()
            }
            fn control(&self, index: Index<{ clic::ip::COUNT }>) -> u32 {
                self.read_ctl(index).record_word()
            }
        }
    };
}

line_bytes!(Clic);
line_bytes!(ClicObserver);

impl<B: RouteBank, C: Placement> InterruptController<B, C> {
    /// A line's bind and take-down through its byte registers, one byte a
    /// step. The bind: the enable off if it was on, the attributes (vectored,
    /// the trigger) and the level, the pending byte cleared, then the enable
    /// on last, so the line is never enabled with another level or trigger
    /// than its binding's. The take-down in reverse: the enable off first,
    /// the pending byte cleared, then the attributes and the level of a
    /// disabled line (not vectored, level byte 0).
    ///
    /// A bound line is hardware-vectored and level-triggered, the chip's one
    /// trigger of a bound line; for it the pending byte follows its source,
    /// and clearing it only drops a level no longer asserted.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/include/soc/clic_reg.h`
    /// `:113-160` (the four byte registers); `HW_S31_CLIC_LINE_BYTES_20261119`
    /// (a store of one byte changes it and no other); `ESP_HAL_1F646C5_CLIC_LEVEL_LINES`
    /// in the register model (the bound lines are level-triggered).
    fn write_line(&mut self, line: Line, write: LineWrite) {
        let index = line.ctrl;
        if self.clic.read_ie(index).ie() {
            self.clic.modify_ie(index, |byte| {
                byte.set_ie(false);
            });
        }
        match write {
            LineWrite::Bind(control) => {
                self.attributes_and_level(index, true, control);
                self.clear_pending(index);
                self.clic.modify_ie(index, |byte| {
                    byte.set_ie(true);
                });
            }
            LineWrite::Disable => {
                self.clear_pending(index);
                self.attributes_and_level(index, false, Control::from_bits(0));
            }
        }
    }

    /// The attribute byte (vectoring, the level trigger) and the level byte.
    fn attributes_and_level(
        &mut self,
        index: Index<{ clic::ip::COUNT }>,
        vectored: bool,
        control: Control,
    ) {
        self.clic.modify_attr(index, |byte| {
            byte.set_shv(vectored)
                .set_trig(clic::attr::Trig::PositiveLevel);
        });
        self.clic.modify_ctl(index, |byte| {
            byte.set_ctl(control.bits());
        });
    }

    /// The pending byte cleared.
    fn clear_pending(&mut self, index: Index<{ clic::ip::COUNT }>) {
        self.clic.modify_ip(index, |byte| {
            byte.set_ip(false);
        });
    }
}
