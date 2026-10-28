//! The observer contract: how an owner hands an observation to whatever the
//! image chose to collect it, at no cost when instrumentation is off.
//!
//! An owner (a core, a runtime, a driver) holds an `O: Observer` value and
//! reports through [`observe!`](crate::observe). The observer receives a
//! copied event value and returns `()`: it never holds an owner, a register
//! block or a lease, and nothing it does reaches back into the owner's
//! decisions. Without `cfg(oer_trace)` an observation point expands to no
//! code: its event expression is neither evaluated nor compiled.

use crate::Event;

/// Receives the observations of an owner.
///
/// `event` takes a value that is `Copy` and `'static`, so an affine owner
/// (a register block, a lease, a permit) or a borrow of one cannot be an
/// observation, and returns `()`, so an observation cannot steer the owner.
/// The method is generic, so the trait has no `dyn` form: every call is a
/// direct call that stack analysis sees.
pub trait Observer {
    fn event<E: Event + Copy + 'static>(&self, event: E);
}

/// Children of an owner receive its observer by reference.
impl<O: Observer> Observer for &O {
    #[inline(always)]
    fn event<E: Event + Copy + 'static>(&self, event: E) {
        (**self).event(event);
    }
}

/// The observer that keeps nothing, an owner's default. It has no size, an
/// alignment of one and no drop glue, so an owner's field of this type costs
/// no memory, padding or code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Noop;

impl Observer for Noop {
    #[inline(always)]
    fn event<E: Event + Copy + 'static>(&self, _: E) {}
}

const _: () = {
    if core::mem::size_of::<Noop>() != 0 {
        panic!("Noop has no size");
    }
    if core::mem::align_of::<Noop>() != 1 {
        panic!("Noop adds no alignment");
    }
    if core::mem::needs_drop::<Noop>() {
        panic!("Noop has no drop glue");
    }
};

/// An observation point: `observe!(self.observer, Event { … })`.
///
/// With `cfg(oer_trace)` it passes the event to the observer. Without it the
/// event expression is removed before type checking, so no argument is
/// evaluated (no clock read, register read or frame parse) and the point
/// leaves no code. The observer operand is a binding or a field path, never
/// a call; without the cfg it is only borrowed, which keeps an owner's
/// observer field in use and evaluates nothing.
#[macro_export]
macro_rules! observe {
    ($root:ident $(. $field:tt)*, $event:expr $(,)?) => {{
        #[cfg(oer_trace)]
        {
            $crate::Observer::event(&$root $(. $field)*, $event);
        }
        #[cfg(not(oer_trace))]
        {
            let _ = &$root $(. $field)*;
        }
    }};
}
