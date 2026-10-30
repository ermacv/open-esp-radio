//! Stand equipment a scenario declares in named slots and its cases take.
//!
//! A slot names one piece of equipment of one type: two slots of the same
//! type are two pieces, which the stand file must hold as two distinct
//! resources. Every slot is resolved from the stand file before the lease
//! and claimed in the one request with the board. The session keeps every
//! opened piece: a case borrows it through [`crate::Cx::take`], and the
//! session finishes it after the cases, whatever the cases left running.

use std::{
    any::Any,
    cell::{RefCell, RefMut},
    marker::PhantomData,
};

use oer_lab_arbiter::claims::Claim;
use oer_lab_model::Stand;

/// A type of stand equipment.
///
/// A long operation on it (a capture, a traffic run) is its own handle with
/// an explicit `finish` that returns the result and a `cancel`; a handle
/// dropped without either is abandoned, and [`Equipment::finish`] stops it
/// and reports it.
pub trait Equipment: Sized + 'static {
    /// The stand resource that fills `slot`, and what to claim for it.
    fn resolve(stand: &Stand, slot: &'static str) -> Result<Resolved, String>;
    /// Open the resolved resource under the lease.
    fn open(resolved: &Resolved) -> Result<Self, String>;
    /// End every operation the cases left and release the equipment; the
    /// problem when it is not left usable.
    fn finish(&mut self) -> Result<(), String>;
}

/// The stand resource a slot resolved to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolved {
    /// The physical resource: two slots never resolve to the same one.
    pub resource: String,
    /// What the lease claims for it.
    pub claims: Vec<Claim>,
}

/// A named slot for one piece of equipment of type `T`.
pub struct Slot<T> {
    name: &'static str,
    _type: PhantomData<fn() -> T>,
}

impl<T: Equipment> Slot<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _type: PhantomData,
        }
    }

    /// The slot as a scenario declares it.
    pub const fn declare(&self) -> Declared {
        Declared {
            name: self.name,
            resolve: T::resolve,
            open: open::<T>,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// A slot in a scenario's declaration, with its equipment type erased.
#[derive(Clone, Copy)]
pub struct Declared {
    pub(crate) name: &'static str,
    pub(crate) resolve: fn(&Stand, &'static str) -> Result<Resolved, String>,
    pub(crate) open: fn(&Resolved) -> Result<Box<dyn Held>, String>,
}

impl std::fmt::Debug for Declared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Declared")
            .field("name", &self.name)
            .finish()
    }
}

fn open<T: Equipment>(resolved: &Resolved) -> Result<Box<dyn Held>, String> {
    Ok(Box::new(RefCell::new(T::open(resolved)?)))
}

/// Opened equipment in its cell, with its type erased.
pub(crate) trait Held {
    /// Finish the equipment; an error when a case still holds it.
    fn finish(&self) -> Result<(), String>;
    /// The cell, for a case to find by type.
    fn cell(&self) -> &dyn Any;
}

impl<T: Equipment> Held for RefCell<T> {
    fn finish(&self) -> Result<(), String> {
        self.try_borrow_mut()
            .map_err(|_| "a case still holds it".to_owned())?
            .finish()
    }
    fn cell(&self) -> &dyn Any {
        self
    }
}

/// Whether `slots` name each slot once: a scenario's declaration checked
/// at compile time ([`crate::Scenario::new`]).
pub(crate) const fn distinct(slots: &[Declared]) -> bool {
    let mut remaining = slots;
    while let Some((first, rest)) = remaining.split_first() {
        let mut others = rest;
        while let Some((other, tail)) = others.split_first() {
            if same(first.name.as_bytes(), other.name.as_bytes()) {
                return false;
            }
            others = tail;
        }
        remaining = rest;
    }
    true
}

const fn same(mut a: &[u8], mut b: &[u8]) -> bool {
    loop {
        match (a.split_first(), b.split_first()) {
            (None, None) => return true,
            (Some((x, a_rest)), Some((y, b_rest))) if *x == *y => {
                a = a_rest;
                b = b_rest;
            }
            _ => return false,
        }
    }
}

/// Resolve every slot of `slots` from `stand`; an error when one does not
/// resolve or two resolve to one resource.
pub(crate) fn resolve(
    stand: &Stand,
    slots: &[Declared],
) -> Result<Vec<(Declared, Resolved)>, String> {
    let mut resolved: Vec<(Declared, Resolved)> = Vec::new();
    for slot in slots {
        let found = (slot.resolve)(stand, slot.name)
            .map_err(|error| format!("slot `{}`: {error}", slot.name))?;
        if let Some((other, _)) = resolved
            .iter()
            .find(|(_, known)| known.resource == found.resource)
        {
            return Err(format!(
                "slots `{}` and `{}` both resolve to {}; the stand file needs one resource for each",
                other.name, slot.name, found.resource
            ));
        }
        resolved.push((*slot, found));
    }
    Ok(resolved)
}

/// The equipment of a session, by slot name.
#[derive(Default)]
pub(crate) struct Opened {
    held: Vec<(&'static str, Box<dyn Held>)>,
}

impl Opened {
    /// Open every resolved slot; the first that fails, with the slots
    /// opened before it kept for finishing.
    pub(crate) fn open(&mut self, resolved: &[(Declared, Resolved)]) -> Result<(), String> {
        for (slot, resource) in resolved {
            let held =
                (slot.open)(resource).map_err(|error| format!("slot `{}`: {error}", slot.name))?;
            self.held.push((slot.name, held));
        }
        Ok(())
    }

    /// Borrow `slot`'s equipment for a case.
    pub(crate) fn take<T: Equipment>(&self, slot: &Slot<T>) -> Result<RefMut<'_, T>, TakeError> {
        let (_, held) = self
            .held
            .iter()
            .find(|(name, _)| *name == slot.name)
            .ok_or(TakeError::Undeclared(slot.name))?;
        held.cell()
            .downcast_ref::<RefCell<T>>()
            .ok_or(TakeError::Type(slot.name))?
            .try_borrow_mut()
            .map_err(|_| TakeError::Taken(slot.name))
    }

    /// Finish every piece in reverse order of opening; each slot whose
    /// equipment was not left usable, with why.
    pub(crate) fn finish(&mut self) -> Vec<(&'static str, String)> {
        let mut problems = Vec::new();
        while let Some((name, held)) = self.held.pop() {
            if let Err(problem) = held.finish() {
                problems.push((name, problem));
            }
        }
        problems
    }
}

/// Why a case cannot take a slot's equipment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TakeError {
    /// The scenario declares no slot of this name.
    Undeclared(&'static str),
    /// The slot's equipment is not of the slot's type.
    Type(&'static str),
    /// The case holds the slot's equipment already.
    Taken(&'static str),
}

impl std::fmt::Display for TakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undeclared(name) => write!(formatter, "the scenario declares no slot `{name}`"),
            Self::Type(name) => write!(formatter, "slot `{name}` holds another equipment type"),
            Self::Taken(name) => write!(formatter, "slot `{name}` is taken already"),
        }
    }
}

impl std::error::Error for TakeError {}
#[cfg(test)]
mod tests;
