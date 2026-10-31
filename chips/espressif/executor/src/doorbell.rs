//! The radio doorbell: a software interrupt at the radio interrupts' level.
//!
//! The radio executor (P2) cannot write the registers the radio interrupts
//! (P3) own without sharing them with a handler that preempts it. It records
//! a request in memory both sides see and rings the doorbell; the doorbell's
//! line is one source of the radio interrupt's part, so the handler that
//! holds the radio registers runs at P3 and applies the request.
//!
//! [`DoorbellLine::split`] takes the line from the reset root and hands out the
//! lowering half ([`DoorbellAcknowledger`]) for the handler's part and the
//! unbound line ([`DoorbellLine`]). Only the binding of that part makes the
//! ringing half: [`Doorbell::new`] takes the router's [`Bound`] proof of a
//! part whose sources hold the line's, checked when the image compiles. A
//! part of the line alone binds the acknowledger itself, which names the
//! line's source.
//!
//! The doorbell consumes its software interrupt, so no executor can be woken
//! through the same line. Its handler first calls
//! [`DoorbellAcknowledger::acknowledge`], then takes the requests: a ring
//! after that raises the interrupt again and is not lost.

use crate::SoftwareInterrupt;
use core::marker::PhantomData;

use oer_espressif_interrupt_table::{
    Bound, Class, Context, Entry, InterruptSource, InterruptSources, request_direction,
};

use crate::chip::{Binding, Line, Line0, Line1, Line2, Line3, PortOf, software_interrupt_source};
use crate::software_line::SoftwareLine;
use crate::urgent_sources::UrgentSource;

/// The radio doorbell of chip binding `B` on software interrupt `SWI`
/// (`FROM_CPU_INTR<SWI>`): the ringing half.
pub struct Doorbell<B: Binding, const SWI: u8, E: Entry, X: Entry> {
    line: Line<PortOf<B>>,
    entries: PhantomData<fn() -> (E, X)>,
}

/// Software interrupt `SWI`'s line before the binding of the part it is a
/// source of: it rings nothing until [`Doorbell::new`] takes the binding's
/// proof.
#[must_use = "the line rings only through the doorbell its binding makes"]
pub struct DoorbellLine<B: Binding, const SWI: u8> {
    line: Line<PortOf<B>>,
}

/// Whether `source` is one of `sources`, at compile time.
const fn holds(sources: &[u16], source: u16) -> bool {
    let mut rest = sources;
    while let [first, tail @ ..] = rest {
        if *first == source {
            return true;
        }
        rest = tail;
    }
    false
}

macro_rules! impl_doorbell {
    ($number:literal, $block:ident, $take:ident) => {
        impl<B: Binding> DoorbellLine<B, $number> {
            /// Move the line's block from the reset root and lower a stale
            /// raise: the lowering half for the handler's part, and the line
            /// that rings once the part is bound.
            pub fn split(
                interrupt: $block<B>,
            ) -> (DoorbellLine<B, $number>, DoorbellAcknowledger<B, $number>) {
                let line = crate::chip::$take::<B>(interrupt);
                line.lower();
                (
                    DoorbellLine { line },
                    DoorbellAcknowledger {
                        line: LineAcknowledger { line },
                    },
                )
            }
        }
    };
}

impl_doorbell!(0, Line0, own_line_0);
impl_doorbell!(1, Line1, own_line_1);
impl_doorbell!(2, Line2, own_line_2);
impl_doorbell!(3, Line3, own_line_3);

impl<B: Binding, const SWI: u8, E: Entry, X: Entry> Doorbell<B, SWI, E, X> {
    /// The handler and requester selected by this doorbell's constructor.
    pub const HANDLER: Context = E::CONTEXT;
    pub const EXECUTOR: Context = X::CONTEXT;

    /// The ringing half of `line`, once `bound` proves that the image's
    /// interrupt table binds entry `E`, whose part holds `Member`'s source, the
    /// line's own: its handler then serves every ring. A part of the line
    /// alone is its own member (each [`InterruptSource`] is
    /// [`InterruptSources`]).
    ///
    /// A member that is not the line, or that the bound part does not hold,
    /// fails to compile (the constant's check, `E0080`). `X` is the
    /// requesting executor's entry; `E` must be above it on the same hart.
    /// The accompanying urgent source retains that same `Member` type for
    /// executor admission; it cannot name another software line.
    pub fn new<Member>(line: DoorbellLine<B, SWI>, bound: &Bound<E>) -> (Self, UrgentSource<Member>)
    where
        Member: InterruptSource,
    {
        const {
            if !matches!(X::CLASS, Class::Executor) {
                panic!("the doorbell requester must be an executor entry");
            }
            if !request_direction(E::CONTEXT, X::CONTEXT) {
                panic!("a request handler must be above its executor on the same hart");
            }
            if Member::SOURCE
                != software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<SWI>())
            {
                panic!("the member is not the doorbell's software interrupt");
            }
            if !holds(<E::Part as InterruptSources>::SOURCES, Member::SOURCE) {
                panic!("the bound part does not hold the doorbell's software interrupt");
            }
        };
        let _bound = bound;
        (
            Self {
                line: line.line,
                entries: PhantomData,
            },
            UrgentSource::new(),
        )
    }

    /// Raise the doorbell. Its `fence w, o` orders the caller's request
    /// writes before the raise (a memory fence alone orders no device
    /// write); on ESP32-S31 the raise reads the line back, so it has reached
    /// the interrupt matrix when this returns. Never waits.
    pub fn ring(&self) {
        crate::software_line::raise(&self.line);
    }
}

/// The one lowering half of software interrupt line `SWI`, for its handler.
pub struct LineAcknowledger<B: Binding, const SWI: u8> {
    line: Line<PortOf<B>>,
}

impl<B: Binding, const SWI: u8> LineAcknowledger<B, SWI> {
    /// Lower the line, then `fence o, rw`, before taking the requests its
    /// raises announced: a ring after this raises the line again and is not
    /// lost, and the requests a ring announced are visible.
    #[inline(always)]
    pub fn acknowledge(&mut self) {
        crate::software_line::serve(&self.line, || ());
    }
}

/// The doorbell's lowering half, which its handler's part holds: inside a
/// part of several sources, or as the part of the line alone, whose source
/// it names.
pub struct DoorbellAcknowledger<B: Binding, const SWI: u8> {
    line: LineAcknowledger<B, SWI>,
}

impl<B: Binding, const SWI: u8> InterruptSource for DoorbellAcknowledger<B, SWI> {
    const SOURCE: u16 =
        software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<SWI>());
}

impl<B: Binding, const SWI: u8> DoorbellAcknowledger<B, SWI> {
    /// Lower the line before taking requests. Only the handler owns this half.
    #[inline(always)]
    pub fn acknowledge(&mut self) {
        self.line.acknowledge();
    }
}
