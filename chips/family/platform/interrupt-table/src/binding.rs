//! Binding failures keep ownership and source observations distinct.
use core::mem::ManuallyDrop;

use crate::Entry;

/// Numeric operation identity, safe to capture without reading cached text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum BindFaultCode {
    StorageClaimed = 1,
    HandlerOccupied = 2,
}

/// The operation whose invariant failed after admission of a binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindFaultKind {
    StorageClaimed,
    HandlerOccupied,
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
    /// The record's four words: source/line/hart, then three zero words.
    /// The operation is named by the reason.
    pub fn diagnostic(self) -> (BindFaultCode, [u32; 4]) {
        let name = match self.kind {
            BindFaultKind::StorageClaimed => BindFaultCode::StorageClaimed,
            BindFaultKind::HandlerOccupied => BindFaultCode::HandlerOccupied,
        };
        (name, [context(self.source, self.line, self.hart), 0, 0, 0])
    }
}

/// No API releases these owners, even if the failure itself is dropped.
/// A placed part remains in its permanently claimed allocation. The image resets without dropping its router
/// or interrupted task owners.
pub struct Quarantined<E: Entry> {
    pub fault: BindFault,
    _token: ManuallyDrop<E>,
    _part: ManuallyDrop<Part<E::Part>>,
}

enum Part<P: 'static> {
    Owned(P),
    Placed { _owner: &'static mut P },
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
}

fn context(source: u16, line: u8, hart: u8) -> u32 {
    u32::from(source) | (u32::from(line) << 16) | (u32::from(hart) << 24)
}
