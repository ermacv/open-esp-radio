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
    clic::{self, Clic, ClicObserver, ClicPending},
    instances,
    interrupt_core::{self, InterruptCore0, InterruptCore0Observer, InterruptCore1},
};
use oer_chips_family_platform_interrupt_table::{
    Controller, Error, LineExpectation, LineObservation,
};
use oer_portable_riscv::OnHart;

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
use oer_portable_bounded::Index;
use oer_portable_mmio::{Bits, Fixed, Placement};

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
    pub grant: Hart0Grant,
}

/// The interrupt matrix's parts as the root splits them: hart 0's route
/// bank and route observer, hart 0's CLIC parts and every hart's pending
/// sources. Hart 1's CLIC parts go to the launch permit.
pub(crate) struct MatrixParts {
    pub(crate) clic: Clic,
    pub(crate) clic_observer: ClicObserver,
    /// Hart 0's pending bytes, in their static place.
    pub(crate) clic_pending: &'static ClicPending<0>,
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
            clic_pending,
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
            grant: InterruptGrant::new(
                Hart0Routes::new(interrupt_core0),
                clic,
                None,
                HartPending::new(clic_pending),
            ),
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

/// Hart 0's route bank in the interrupt matrix, `INTERRUPT_CORE0`: the
/// HAL's holder of the PAC's owner.
pub struct Hart0Routes<M: Placement = Fixed<{ instances::interrupt_core0::BASE }>>(
    InterruptCore0<M>,
);

/// Hart 1's route bank in the interrupt matrix, `INTERRUPT_CORE1`: the
/// HAL's holder of the PAC's owner.
pub struct Hart1Routes<M: Placement = Fixed<{ instances::interrupt_core1::BASE }>>(
    InterruptCore1<M>,
);

impl<M: Placement> Hart0Routes<M> {
    /// Hold the root's hart 0 bank.
    pub(crate) const fn new(bank: InterruptCore0<M>) -> Self {
        Self(bank)
    }
}

impl<M: Placement> Hart1Routes<M> {
    /// Hold the root's hart 1 bank.
    pub(crate) const fn new(bank: InterruptCore1<M>) -> Self {
        Self(bank)
    }
}

/// A hart's route bank in the interrupt matrix: [`Hart0Routes`] for hart
/// 0, [`Hart1Routes`] for hart 1. Sealed: only those banks are.
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
    impl<M: oer_portable_mmio::Placement> Sealed for super::Hart0Routes<M> {}
    impl<M: oer_portable_mmio::Placement> Sealed for super::Hart1Routes<M> {}
}

macro_rules! route_bank {
    ($routes:ident, $bank:ident, $hart:literal) => {
        impl<M: Placement> RouteBank for $routes<M> {
            const HART: u8 = $hart;
            fn read_route(
                &self,
                index: Index<{ interrupt_core::route::COUNT }>,
            ) -> interrupt_core::route::R {
                $bank::read_route(&self.0, index)
            }
            fn modify_route(
                &mut self,
                index: Index<{ interrupt_core::route::COUNT }>,
                change: impl FnOnce(&mut interrupt_core::route::W),
            ) {
                $bank::modify_route(&mut self.0, index, change);
            }
        }
    };
}

route_bank!(Hart0Routes, InterruptCore0, 0);
route_bank!(Hart1Routes, InterruptCore1, 1);

/// Hart `H`'s CLIC pending bytes in their static place: the HAL's holder
/// of the PAC's owner.
pub struct HartPending<const H: u8>(&'static ClicPending<H>);

impl<const H: u8> HartPending<H> {
    /// Hold the hart's pending bytes in their static place.
    pub(crate) const fn new(bytes: &'static ClicPending<H>) -> Self {
        Self(bytes)
    }
}

/// A hart's pending bytes, as its controller stores them: [`HartPending`]
/// on the chip, or a host test's owned over its memory. Sealed: only a
/// hart's pending bytes are.
pub trait PendingBytes<const H: u8>: pending::Sealed {
    #[doc(hidden)]
    fn clear(&self, index: Index<{ clic::ip::COUNT }>, hart: &OnHart<H>);
}

mod pending {
    pub trait Sealed {}
    impl<const H: u8> Sealed for super::HartPending<H> {}
    #[cfg(test)]
    impl<const H: u8, P: oer_portable_mmio::Placement> Sealed for super::ClicPending<H, P> {}
}

impl<const H: u8> PendingBytes<H> for HartPending<H> {
    #[inline(always)]
    fn clear(&self, index: Index<{ clic::ip::COUNT }>, hart: &OnHart<H>) {
        self.0.clear_line(index, hart);
    }
}

#[cfg(test)]
impl<const H: u8, P: Placement> PendingBytes<H> for ClicPending<H, P> {
    #[inline(always)]
    fn clear(&self, index: Index<{ clic::ip::COUNT }>, hart: &OnHart<H>) {
        self.clear_line(index, hart);
    }
}

/// One hart's interrupt grant: its route bank, its own CLIC's parts and
/// its pending bytes, which the root pairs once per hart. Inert: it reads
/// and writes nothing until [`into_controller`](Self::into_controller)
/// runs on its hart.
#[must_use = "dropping the grant permanently loses the hart's routes and lines"]
pub struct InterruptGrant<
    B: RouteBank,
    const H: u8,
    C: Placement = Fixed<{ instances::clic::BASE }>,
    P: PendingBytes<H> = HartPending<H>,
> {
    bank: B,
    lines: Clic<C>,
    /// The hart's level-bits writer, when its controller sets them: hart
    /// 1's. Hart 0's are set when the root splits the partition, where its
    /// observer is published for hart 0's fatal path.
    config: Option<ClicObserver<C>>,
    pending: P,
}

pub use oer_chips_family_platform_port::ForeignHart;

impl<B: RouteBank, const H: u8, C: Placement, P: PendingBytes<H>> InterruptGrant<B, H, C, P> {
    /// The root's pairing of a hart's bank with that hart's CLIC parts and
    /// pending bytes.
    pub(crate) const fn new(
        bank: B,
        lines: Clic<C>,
        config: Option<ClicObserver<C>>,
        pending: P,
    ) -> Self {
        const {
            if B::HART != H {
                panic!("a grant pairs a hart's bank with that hart's lines")
            }
        };
        Self {
            bank,
            lines,
            config,
            pending,
        }
    }

    /// The grant's hart.
    pub const fn hart(&self) -> u8 {
        H
    }

    /// The grant's bank, before its hart's controller exists: the launch
    /// disconnects the bank's routes before it releases the hart.
    pub(crate) fn bank(&mut self) -> &mut B {
        &mut self.bank
    }

    /// Make the hart's controller on the hart itself, as it starts: the
    /// proof of the hart is taken here, once ([`OnHart::check`]), and kept
    /// for the pending bytes' stores.
    ///
    /// # Errors
    ///
    /// [`ForeignHart`] on another hart than the grant's: nothing is written
    /// and the grant is lost (each hart has one).
    #[cfg(target_arch = "riscv32")]
    pub fn into_controller(self) -> Result<InterruptController<B, H, C, P>, ForeignHart> {
        let hart = OnHart::<H>::check().map_err(|other| ForeignHart {
            actual: other.raw,
            expected: usize::from(H),
        })?;
        Ok(self.into_controller_with(hart))
    }

    /// Make the hart's controller with the proof that this code runs on
    /// it: give its CLIC [`facts::LEVEL_BITS`] level bits when the grant
    /// holds its writer, and own the bank and the CLIC's lines. Every route
    /// and line stays as it is until the router installs the controller.
    pub fn into_controller_with(self, hart: OnHart<H>) -> InterruptController<B, H, C, P> {
        let Self {
            bank,
            lines,
            mut config,
            pending,
        } = self;
        if let Some(config) = config.as_mut() {
            set_level_bits(config);
        }
        InterruptController {
            matrix: bank,
            clic: lines,
            _config: config,
            pending,
            hart,
        }
    }
}

impl<B: RouteBank, const H: u8, C: Placement> InterruptGrant<B, H, C> {
    /// The hart's pend lines, over the pending bytes the grant holds for
    /// its controller.
    pub(crate) const fn pend_lines(&self) -> PendLines<'static, H> {
        PendLines::new(self.pending.0)
    }
}

/// Hart `H`'s pend lines: the one thing it does is give the pend handle of
/// a line fixed at build ([`LinePend`]). It holds the hart's pending bytes
/// and gives no clear, no index made at run time and no access to them: a
/// clear is the controller's, at a task line's bind.
///
/// ```
/// use oer_chips_esp32s31_hal::system::platform::{LinePend, PendLines};
/// fn keeper(lines: &PendLines<'static, 0>) -> LinePend<'static, 0, 34> {
///     lines.line()
/// }
/// # let _: fn(_) -> _ = keeper;
/// ```
///
/// A line the CLIC has not does not build:
///
/// ```compile_fail,E0080
/// use oer_chips_esp32s31_hal::system::platform::{LinePend, PendLines};
/// fn past(lines: &PendLines<'static, 0>) -> LinePend<'static, 0, 48> {
///     lines.line()
/// }
/// # let past: fn(_) -> _ = past;
/// # core::hint::black_box(past);
/// ```
pub struct PendLines<'b, const H: u8, P: Placement = Fixed<{ instances::clic::BASE }>> {
    bytes: &'b ClicPending<H, P>,
}

impl<'b, const H: u8, P: Placement> PendLines<'b, H, P> {
    /// The pend lines over hart `H`'s pending bytes (on the chip, in their
    /// static place).
    pub(crate) const fn new(bytes: &'b ClicPending<H, P>) -> Self {
        Self { bytes }
    }

    /// The pend handle of line `LINE` on hart `H`.
    pub const fn line<const LINE: u8>(&self) -> LinePend<'b, H, LINE, P> {
        LinePend::new(self.bytes)
    }
}

/// The handle of one line's pending byte on hart `H`, the line fixed at
/// build: a level task's raisers pend its line through it, and it reaches
/// no other line
/// ([`PendLine`](oer_chips_family_platform_interrupt_table::PendLine)).
/// The hart's [`PendLines`] give it; outside the HAL a line is pended
/// through the task's `Pend`.
pub struct LinePend<
    'b,
    const H: u8,
    const LINE: u8,
    P: Placement = Fixed<{ instances::clic::BASE }>,
> {
    bytes: &'b ClicPending<H, P>,
}

impl<'b, const H: u8, const LINE: u8, P: Placement> LinePend<'b, H, LINE, P> {
    /// Line `LINE`'s element of the pending bytes, made at build.
    const INDEX: Index<{ clic::ip::COUNT }> = match Index::new(LINE as usize) {
        Ok(index) => index,
        Err(_) => panic!("a line's pending byte lies in the CLIC"),
    };

    /// The handle of line `LINE` in hart `H`'s pending bytes.
    pub(crate) const fn new(bytes: &'b ClicPending<H, P>) -> Self {
        let _index = Self::INDEX;
        Self { bytes }
    }

    /// Store line `LINE`'s pending byte as pended, on hart `H`'s copy.
    #[inline(always)]
    pub(crate) fn pend(&self, hart: &OnHart<H>) {
        self.bytes.pend_line(Self::INDEX, hart);
    }
}

impl<const H: u8, const LINE: u8, P: Placement>
    oer_chips_family_platform_interrupt_table::PendLine<H> for LinePend<'_, H, LINE, P>
where
    ClicPending<H, P>: Sync,
{
    const LINE: u8 = LINE;

    #[inline(always)]
    fn pend(&self, hart: &OnHart<H>) {
        Self::pend(self, hart);
    }
}

/// One hart's interrupt-matrix bank and that hart's own CLIC, the router's
/// [`Controller`]: hart 0's by default ([`Hart0Routes`]), hart 1's over
/// [`Hart1Routes`] ([`Hart1Controller`]). It holds the proof of its hart
/// and stays on it (`!Send`).
///
/// ```compile_fail
/// fn send<T: Send>() {}
/// send::<oer_chips_esp32s31_hal::system::platform::InterruptController>();
/// ```
#[must_use = "dropping the interrupt controller permanently loses the routes and lines"]
pub struct InterruptController<
    B: RouteBank = Hart0Routes,
    const H: u8 = 0,
    C: Placement = Fixed<{ instances::clic::BASE }>,
    P: PendingBytes<H> = HartPending<H>,
> {
    matrix: B,
    clic: Clic<C>,
    /// Hart 1's level-bits writer, retained once it set them.
    _config: Option<ClicObserver<C>>,
    /// The hart's pending bytes, which the controller clears at a level
    /// task's bind.
    pending: P,
    /// The proof that the controller runs on hart `H`: the CLIC words
    /// reach the running hart's own controller.
    hart: OnHart<H>,
}

/// Hart 1's controller.
pub type Hart1Controller = InterruptController<Hart1Routes, 1>;
/// Hart 0's grant.
pub type Hart0Grant = InterruptGrant<Hart0Routes, 0>;
/// Hart 1's grant, over its bank's, its CLIC's and its pending bytes'
/// placements.
pub type Hart1Grant<
    M = Fixed<{ instances::interrupt_core1::BASE }>,
    K = Fixed<{ instances::clic::BASE }>,
    G = HartPending<1>,
> = InterruptGrant<Hart1Routes<M>, 1, K, G>;

impl<B: RouteBank, const H: u8, C: Placement, P: PendingBytes<H>> Controller
    for InterruptController<B, H, C, P>
{
    type Source = Source;
    type Line = Line;
    const SOURCES: &'static [Source] = &SOURCES;
    const LINES: &'static [Line] = &LINES;
    const LEVEL_BITS: LevelBits = LevelBits::constant::<{ facts::LEVEL_BITS }>();

    fn hart(&self) -> u8 {
        H
    }

    fn verify_configuration(&self) -> Result<(), Error> {
        const {
            if !(clic::ip::COUNT == facts::LINES.count as usize) {
                panic!("build-time check: clic::ip::COUNT == facts::LINES.count as usize")
            }
        };
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
            route.set_map(interrupt_core::route::Map::Disconnected.into());
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

    fn bind_task(&mut self, line: Line, control: Control) {
        self.write_line(line, LineWrite::Task(control));
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
                self.holds(index, ROUTED_TRIGGER, control) && enabled
            }
            LineExpectation::Task(control) => self.holds(index, TASK_TRIGGER, control) && enabled,
        };
        if holds {
            Ok(())
        } else {
            Err(line_record(&self.clic, index))
        }
    }
}

/// What a line write leaves: the line disabled, bound for its routed source
/// at a control byte, or a level task's at a control byte.
#[derive(Clone, Copy, Debug)]
enum LineWrite {
    Disable,
    Bind(Control),
    Task(Control),
}

/// The trigger of a bound line whose source is routed to it: level, its
/// pending bit following the source.
///
/// SOURCE: `ESP_HAL_1F646C5_CLIC_LEVEL_LINES` in the register model.
const ROUTED_TRIGGER: clic::attr::Trig = clic::attr::Trig::PositiveLevel;

/// The trigger of a level task's line: positive edge, the one trigger whose
/// pending bit is software's, so that its raisers pend it and the vectored
/// entry clears it.
///
/// SOURCE: `HW1_S31_CLIC_20261026` (a software-set positive-edge pending bit
/// vectored once and read clear after the entry);
/// `HW_S31_CLIC_LEVELS_SOFTWARE_PEND_20261118` and
/// `HW_S31_CLIC_LINE_BYTES_20261119` (line 34 bound positive-edge, its
/// pending bit cleared before its enable, entered once a pend).
const TASK_TRIGGER: clic::attr::Trig = clic::attr::Trig::PositiveEdge;

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

/// One write of a line's bind or take-down, each to one byte register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineStep {
    /// The enable byte off.
    EnableOff,
    /// The attribute byte (vectored or not, the trigger) and the level
    /// byte.
    AttributesAndLevel {
        vectored: bool,
        trigger: clic::attr::Trig,
        control: Control,
    },
    /// The pending byte cleared: only of a level task's line, whose trigger
    /// is already an edge.
    PendingClear,
    /// The enable byte on.
    EnableOn,
}

/// A line's bind and take-down, one byte a step, in order. The bind: the
/// enable off if it was on, the attributes (vectored, the trigger) and the
/// level, then the enable on last, so the line is never enabled with
/// another level or trigger than its binding's. The take-down: the enable
/// off first, then the attributes and the level of a disabled line (not
/// vectored, level byte 0).
///
/// A bound line is hardware-vectored. A routed line is level-triggered
/// ([`ROUTED_TRIGGER`]): its pending byte follows its source and is not the
/// router's, so neither its bind nor the take-down reads or writes it. A
/// level task's line is edge-triggered ([`TASK_TRIGGER`]): its pending byte
/// is software's, and its bind clears it after the trigger is an edge and
/// before the enable, so the line enters for its raisers' pends alone.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/include/soc/clic_reg.h`
/// `:113-160` (the four byte registers); `HW_S31_CLIC_LINE_BYTES_20261119`
/// (a store of one byte changes it and no other).
fn line_steps(write: LineWrite, enabled: bool) -> [Option<LineStep>; 4] {
    let off = enabled.then_some(LineStep::EnableOff);
    match write {
        LineWrite::Bind(control) => [
            off,
            Some(LineStep::AttributesAndLevel {
                vectored: true,
                trigger: ROUTED_TRIGGER,
                control,
            }),
            Some(LineStep::EnableOn),
            None,
        ],
        LineWrite::Task(control) => [
            off,
            Some(LineStep::AttributesAndLevel {
                vectored: true,
                trigger: TASK_TRIGGER,
                control,
            }),
            Some(LineStep::PendingClear),
            Some(LineStep::EnableOn),
        ],
        LineWrite::Disable => [
            off,
            Some(LineStep::AttributesAndLevel {
                vectored: false,
                trigger: ROUTED_TRIGGER,
                control: Control::from_bits(0),
            }),
            None,
            None,
        ],
    }
}

impl<B: RouteBank, const H: u8, C: Placement, P: PendingBytes<H>> InterruptController<B, H, C, P> {
    /// Whether the line's attributes are vectored with `trigger` and its
    /// level byte is `control`'s.
    fn holds(
        &self,
        index: Index<{ clic::ip::COUNT }>,
        trigger: clic::attr::Trig,
        control: Control,
    ) -> bool {
        let attributes = self.clic.read_attr(index);
        attributes.shv()
            && attributes.trig() == trigger
            && self.clic.read_ctl(index).ctl() == control.bits()
    }

    /// The line's bind or take-down ([`line_steps`]), each step one byte.
    fn write_line(&mut self, line: Line, write: LineWrite) {
        let index = line.ctrl;
        let enabled = self.clic.read_ie(index).ie();
        for step in line_steps(write, enabled).into_iter().flatten() {
            match step {
                LineStep::EnableOff => self.clic.modify_ie(index, |byte| {
                    byte.set_ie(false);
                }),
                LineStep::AttributesAndLevel {
                    vectored,
                    trigger,
                    control,
                } => {
                    self.clic.modify_attr(index, |byte| {
                        byte.set_shv(vectored).set_trig(trigger);
                    });
                    self.clic.modify_ctl(index, |byte| {
                        byte.set_ctl(control.bits());
                    });
                }
                LineStep::PendingClear => self.pending.clear(index, &self.hart),
                LineStep::EnableOn => self.clic.modify_ie(index, |byte| {
                    byte.set_ie(true);
                }),
            }
        }
    }
}
