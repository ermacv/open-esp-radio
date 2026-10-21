//! Opaque owned context custody under an invariant body loan.

use core::marker::PhantomData;

use crate::{BodyLoan, SlotIdentity, SlotOwner};

/// A domain context's opaque representation at the body/queue boundary.
///
/// Both conversions retain the invariant body loan. Implementations receive
/// no backend authority and cannot extract the representation from the wrapper.
pub trait LoanedContext<'p, Id: SlotIdentity>: Sized {
    /// Representation selected independently of the body's loan.
    type Owned;
    /// Enclose the domain context without exposing its protected owned proofs.
    fn enclose(self) -> PortContext<'p, Id, Self::Owned>;
    /// Restore the domain context under the same body loan.
    fn restore(context: PortContext<'p, Id, Self::Owned>) -> Self;
}

type ContextBrand<'p, Id> = PhantomData<fn(&'p (), Id) -> (&'p (), Id)>;

/// A public context whose owned representation is selected before the body loan.
/// There is no dereference or unguarded extraction of that representation.
#[repr(transparent)]
pub struct PortContext<'p, Id: SlotIdentity, T> {
    value: T,
    brand: ContextBrand<'p, Id>,
}

impl<'p, Id: SlotIdentity, T> LoanedContext<'p, Id> for PortContext<'p, Id, T> {
    type Owned = T;
    fn enclose(self) -> Self {
        self
    }
    fn restore(context: Self) -> Self {
        context
    }
}

impl<'p, Id: SlotIdentity> LoanedContext<'p, Id> for () {
    type Owned = ();
    fn enclose(self) -> PortContext<'p, Id, ()> {
        PortContext::new(())
    }
    fn restore(_: PortContext<'p, Id, ()>) {}
}

impl<'p, Id: SlotIdentity, T> PortContext<'p, Id, T> {
    /// Wrap caller-owned data. A backend fixes `T` before introducing `'p`,
    /// so references into the body cannot become its stored context type.
    pub const fn new(value: T) -> Self {
        Self {
            value,
            brand: PhantomData,
        }
    }

    /// Move the complete opaque context into its backend's owned queue state.
    /// Conversion authority is private in the concrete backend frontend.
    pub fn into_owned<S: SlotOwner<Id>>(self, _: &BodyLoan<'p, Id, S>) -> T {
        self.value
    }

    /// Return owned queue context under the consumer's body loan.
    pub fn from_owned<S: SlotOwner<Id>>(value: T, _: &BodyLoan<'p, Id, S>) -> Self {
        Self::new(value)
    }

    /// Inspect only the payload's explicitly loaned projection.
    pub fn view(&self) -> T::View<'_, 'p>
    where
        T: ContextProjection<Id>,
        T: 'p,
        Id: 'p,
    {
        self.value.view(ContextLoan { brand: self.brand })
    }

    /// Recover public parts while retaining the loan on every protected proof.
    pub fn into_parts(self) -> T::Parts<'p>
    where
        T: ContextProjection<Id>,
        T: 'p,
        Id: 'p,
    {
        self.value.into_parts(ContextLoan { brand: self.brand })
    }
}

/// Read-only loan marker supplied to a context type's projection implementation.
/// It cannot mint or convert slots, permits, backend loans or reuse authority.
pub struct ContextLoan<'p, Id: SlotIdentity> {
    brand: ContextBrand<'p, Id>,
}

/// Public projections of one owned context representation.
///
/// A type containing a protected epoch keeps its owned fields private and
/// returns epoch views branded by `'p`. Plain application data may be returned
/// by value. Neither projection receives backend capacity-conversion authority.
pub trait ContextProjection<Id: SlotIdentity>: Sized {
    /// Borrowed public inspection; protected proofs retain the context loan.
    type View<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Consumed public parts, retaining the loan on protected proofs.
    type Parts<'p>
    where
        Self: 'p,
        Id: 'p;

    /// Borrow public data without exposing the raw owned representation.
    fn view<'a, 'p>(&'a self, loan: ContextLoan<'p, Id>) -> Self::View<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Consume the context into its public, correctly loaned parts.
    fn into_parts<'p>(self, loan: ContextLoan<'p, Id>) -> Self::Parts<'p>
    where
        Self: 'p,
        Id: 'p;
}

impl<Id: SlotIdentity> ContextProjection<Id> for () {
    type View<'a, 'p>
        = ()
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Parts<'p>
        = ()
    where
        Self: 'p,
        Id: 'p;
    fn view<'a, 'p>(&'a self, _: ContextLoan<'p, Id>)
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
    }
    fn into_parts<'p>(self, _: ContextLoan<'p, Id>)
    where
        Self: 'p,
        Id: 'p,
    {
    }
}

/// A closed sum used to compose opaque context families without unpacking them.
pub enum ContextChoice<A, B> {
    /// The first context family.
    First(A),
    /// The second context family.
    Second(B),
}

impl<'p, Id: SlotIdentity, A> PortContext<'p, Id, A> {
    /// Tag one complete context without exposing its owned representation.
    pub fn first<B>(self) -> PortContext<'p, Id, ContextChoice<A, B>> {
        PortContext::new(ContextChoice::First(self.value))
    }
    /// Tag the other complete context, retaining the same body loan.
    pub fn second<B>(self) -> PortContext<'p, Id, ContextChoice<B, A>> {
        PortContext::new(ContextChoice::Second(self.value))
    }
}

impl<'p, Id: SlotIdentity, A, B> PortContext<'p, Id, ContextChoice<A, B>> {
    /// Select the context family while keeping both alternatives opaque and loaned.
    pub fn into_choice(self) -> ContextChoice<PortContext<'p, Id, A>, PortContext<'p, Id, B>> {
        match self.value {
            ContextChoice::First(value) => ContextChoice::First(PortContext::new(value)),
            ContextChoice::Second(value) => ContextChoice::Second(PortContext::new(value)),
        }
    }
}
