//! The A-MPDU values of a station's data arenas.
//!
//! - [`AmpduRate`]: a rate the backend aggregates at, which a data arena's
//!   A-MPDU publication takes
//!   ([`ArenaFormat::Aggregate`](crate::arena::ArenaFormat::Aggregate)) and
//!   only [`PortArena::ampdu_rate`](crate::PortArena::ampdu_rate) checks.

use super::PortRate;

/// A rate the backend aggregates at, checked by
/// [`PortArena::ampdu_rate`](crate::PortArena::ampdu_rate).
#[derive(Debug)]
pub struct AmpduRate<'p, K>(PortRate<'p, K>);
impl<K> Copy for AmpduRate<'_, K> {}
impl<K> Clone for AmpduRate<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'p, K> AmpduRate<'p, K> {
    /// A rate a port's arena checked; nothing outside this crate mints one.
    pub(crate) const fn checked(rate: PortRate<'p, K>) -> Self {
        Self(rate)
    }

    /// The rate.
    pub const fn rate(&self) -> &PortRate<'p, K> {
        &self.0
    }
}
