//! What a caller's analysis may know of the functions its calls reach.
//!
//! The value analysis is intraprocedural: a call's result registers are
//! unknown to the caller. A [`CallReturns`] port answers, for a callee, the
//! finite set of constants every return of it leaves in a result register,
//! when its own analysis proved one; the caller then takes that set for the
//! call's result. A port answers only for a callee whose every return it
//! knows: one unknown return means no answer.
/// The function a call reaches, as its caller names it: an address in a
/// linked image, or the global symbol a relocatable object's call names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Callee<'a> {
    Address(u32),
    Symbol(&'a [u8]),
}

/// The constant results of the functions calls reach.
pub trait CallReturns {
    /// Every constant `register` (`a0` or `a1`) holds when `callee`
    /// returns, sorted and distinct, if every return of it is known to be
    /// one of them; at most [`crate::MAX_VALUE_ALTERNATIVES`].
    fn returns(&self, callee: Callee<'_>, register: u8) -> Option<&[u32]>;
}

/// No callee's results are known: every call's result stays unknown.
pub struct NoReturns;

impl CallReturns for NoReturns {
    fn returns(&self, _: Callee<'_>, _: u8) -> Option<&[u32]> {
        None
    }
}
