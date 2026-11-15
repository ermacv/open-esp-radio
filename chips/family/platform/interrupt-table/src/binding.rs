//! Binding failures keep ownership and source observations distinct.
use core::mem::ManuallyDrop;

use crate::{Entry, Error, LineExpectation};

/// Numeric operation identity, safe to capture without reading cached text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum BindFaultCode {
    StorageClaimed = 1,
    HandlerOccupied = 2,
    LineRefused = 3,
    RouteRefused = 4,
}

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
    pub fn diagnostic(self) -> (BindFaultCode, [u32; 4]) {
        let (name, words) = match self.kind {
            BindFaultKind::StorageClaimed => (BindFaultCode::StorageClaimed, [0; 3]),
            BindFaultKind::HandlerOccupied => (BindFaultCode::HandlerOccupied, [0; 3]),
            BindFaultKind::LineRefused(error) => (BindFaultCode::LineRefused, error_words(error)),
            BindFaultKind::RouteRefused(error) => (BindFaultCode::RouteRefused, error_words(error)),
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

impl<E: Entry> core::fmt::Debug for Quarantined<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Quarantined").field(&self.fault).finish()
    }
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

/// A line and what the table expected of it, both the family's own values,
/// in one word of a record: the line in bits 0..8, the expectation's record
/// (`LineExpectation::record`) from bit 8.
const fn line_and_expectation(line: u8, expected: LineExpectation) -> u32 {
    line as u32 | (expected.record() << 8)
}

// The diagnostic encoding of an error: its tag and two payload words. A wrong
// line's are the line's register word as the hardware has it, then the line
// with its expectation.
fn error_words(error: Error) -> [u32; 3] {
    match error {
        Error::WrongControlBits { actual, expected } => [1, actual.into(), expected.into()],
        Error::WrongInterruptCount { actual, expected } => [2, actual.into(), expected.into()],
        Error::WrongLevelBits { actual, expected } => [3, actual.into(), expected.into()],
        Error::WrongHart { hart, current } => [4, hart.into(), current.into()],
        Error::ForeignRoute { source, line } => [6, source.into(), line.into()],
        Error::WrongLine {
            line,
            observed,
            expected,
        } => [7, observed.record(), line_and_expectation(line, expected)],
        Error::NoSuchSource { source } => [8, source.into(), 0],
        Error::NoSuchLine { line } => [9, line.into(), 0],
        Error::NoSuchLevel { level } => [10, level.into(), 0],
    }
}
