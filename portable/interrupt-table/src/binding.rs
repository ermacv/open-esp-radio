//! Binding failures keep ownership and source observations distinct.
use core::mem::ManuallyDrop;

use crate::{Entry, Error, LineState};

/// The operation whose invariant failed after admission of a binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindFaultKind {
    StorageClaimed,
    HandlerOccupied,
    LineRefused(Error),
    RouteRefused(Error),
}

/// An admitted binding failed without releasing its token or register owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindFault {
    pub source: u16,
    pub line: u8,
    pub hart: u8,
    pub kind: BindFaultKind,
}

impl BindFault {
    /// Four lossless diagnostic words: source/line/hart, error discriminant,
    /// and both error payload words. The operation is named by the reason.
    pub fn diagnostic(self) -> (&'static str, [u32; 4]) {
        let (name, words) = match self.kind {
            BindFaultKind::StorageClaimed => ("BindingStorageClaimed", [0; 3]),
            BindFaultKind::HandlerOccupied => ("BindingHandlerOccupied", [0; 3]),
            BindFaultKind::LineRefused(error) => ("BindingLineRefused", error_words(error)),
            BindFaultKind::RouteRefused(error) => ("BindingRouteRefused", error_words(error)),
        };
        let [tag, first, second] = words;
        (
            name,
            [
                context(self.source, self.line, self.hart),
                tag,
                first,
                second,
            ],
        )
    }
}

/// A pre-admission input refusal returns the unchanged token and part.
/// An invariant fault retains its owners permanently and requires the image's
/// retained reset path; it never grants `Bound` or a retry authority.
pub enum BindError<E: Entry> {
    Refused {
        error: Error,
        token: E,
        part: E::Part,
    },
    Fault(Quarantined<E>),
}

impl<E: Entry> BindError<E> {
    /// The entry identity and complete source observation for retained output.
    pub fn diagnostic(&self) -> (&'static str, [u32; 4]) {
        match self {
            Self::Refused { error, .. } => {
                let [tag, first, second] = error_words(*error);
                // A refused source is in the Error payload. Low context bits
                // are reserved here; no source observation is substituted.
                (
                    "BindingRefused",
                    [context(0, E::LINE, E::HART), tag, first, second],
                )
            }
            Self::Fault(held) => held.fault.diagnostic(),
        }
    }
}

impl<E: Entry> core::fmt::Debug for BindError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Refused { error, .. } => f.debug_tuple("Refused").field(error).finish(),
            Self::Fault(held) => f.debug_tuple("Fault").field(&held.fault).finish(),
        }
    }
}

/// No API releases these owners, even if the failure itself is dropped.
/// A placed part remains in its permanently claimed allocation; an installed
/// part stays with its handler. The image resets without dropping its router
/// or interrupted task owners.
pub struct Quarantined<E: Entry> {
    pub fault: BindFault,
    _token: ManuallyDrop<E>,
    _part: ManuallyDrop<Part<E::Part>>,
}

enum Part<P: 'static> {
    Owned(P),
    Placed { _owner: &'static mut P },
    Installed,
}

impl<E: Entry> Quarantined<E> {
    pub(crate) fn owned(fault: BindFault, token: E, part: E::Part) -> Self {
        Self {
            fault,
            _token: ManuallyDrop::new(token),
            _part: ManuallyDrop::new(Part::Owned(part)),
        }
    }
    pub(crate) fn placed(fault: BindFault, token: E, part: &'static mut E::Part) -> Self {
        Self {
            fault,
            _token: ManuallyDrop::new(token),
            _part: ManuallyDrop::new(Part::Placed { _owner: part }),
        }
    }
    pub(crate) fn installed(fault: BindFault, token: E) -> Self {
        Self {
            fault,
            _token: ManuallyDrop::new(token),
            _part: ManuallyDrop::new(Part::Installed),
        }
    }
}

fn context(source: u16, line: u8, hart: u8) -> u32 {
    u32::from(source) | (u32::from(line) << 16) | (u32::from(hart) << 24)
}

fn line(state: LineState) -> u32 {
    u32::from(state.enabled)
        | (u32::from(state.vectored) << 1)
        | ((state.trigger as u32) << 2)
        | (u32::from(state.control) << 8)
}

// Software diagnostic encoding, independent of any register layout. Each
// LineState fits sixteen bits: flags/trigger in bits 0..3, control in 8..15.
fn error_words(error: Error) -> [u32; 3] {
    match error {
        Error::WrongControlBits { actual, expected } => [1, actual.into(), expected.into()],
        Error::WrongInterruptCount { actual, expected } => [2, actual.into(), expected.into()],
        Error::WrongLevelBits { actual, expected } => [3, actual.into(), expected.into()],
        Error::WrongHart {
            source,
            hart,
            current,
        } => [
            4,
            source.into(),
            u32::from(hart) | (u32::from(current) << 8),
        ],
        Error::NotInTable { source } => [5, source.into(), 0],
        Error::ForeignRoute { source, line } => [6, source.into(), line.into()],
        Error::WrongLine {
            line: index,
            state,
            expected,
        } => [7, index.into(), line(state) | (line(expected) << 16)],
        Error::NoSuchSource { source } => [8, source.into(), 0],
        Error::NoSuchLine { line } => [9, line.into(), 0],
        Error::NoSuchLevel { level } => [10, level.into(), 0],
        Error::Bindable { source } => [11, source.into(), 0],
    }
}
