//! What the core tells its caller's switchable diagnostics of its own steps
//! (docs/architecture.md, "Switchable network-core diagnostics"): the running
//! places a readiness walk visits, the ends an ended chain tells, the frames
//! built whole in the port's slot, and a lending refusal for want of credit.
//!
//! A caller lends a [`Count`] to the calls that take one. `()` counts nothing:
//! its [`Count::ENABLED`] is `false`, so the count points leave no code. An
//! enabled count is asked [`Count::on`] at each point, the only cost it adds
//! while off; on, it only counts, and nothing the core decides reads it. The
//! core reads no clock for it.

/// Receives the core's steps while it is on. Each method takes `&self`, so a
/// lazy walk ([`crate::NetworkOwner::demands`]) can count as it goes: an
/// implementation counts through interior mutability of its own.
pub trait Count {
    /// Whether this count can ever be on; the core asks nothing of one that
    /// cannot.
    const ENABLED: bool = true;

    /// Whether it counts now.
    fn on(&self) -> bool;

    /// A readiness walk visited one running place.
    fn walked(&self);

    /// An ended chain told one end.
    fn told(&self);

    /// A chosen work's frame was built whole in the port's slot.
    fn materialised(&self);

    /// A lending socket's datagram found its socket at its credit, with
    /// `loans` receive loans out of the pool's account.
    fn no_credit(&self, loans: usize);
}

/// The count that keeps nothing.
impl Count for () {
    const ENABLED: bool = false;

    #[inline(always)]
    fn on(&self) -> bool {
        false
    }

    #[inline(always)]
    fn walked(&self) {}

    #[inline(always)]
    fn told(&self) {}

    #[inline(always)]
    fn materialised(&self) {}

    #[inline(always)]
    fn no_credit(&self, _loans: usize) {}
}

/// Count with `count`, only if it can be and is on: a disabled count costs
/// nothing, and an enabled one that is off costs its [`Count::on`].
#[inline(always)]
pub(crate) fn counted<C: Count + ?Sized>(count: &C, step: impl FnOnce(&C)) {
    if C::ENABLED && count.on() {
        step(count);
    }
}
