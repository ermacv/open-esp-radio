//! Cooperative admission observes exactly the sources its image composes.

use core::marker::PhantomData;
use oer_espressif_interrupt_table::InterruptSource;

mod sealed {
    pub trait Sources {}
}

/// Sources admitted by doorbell construction. `()` declares no urgent source.
/// Images obtain a typed source from the same constructor that checks the
/// doorbell's member, software line and interrupt binding.
pub trait UrgentSources: sealed::Sources {
    /// The immutable source list stored by the receiving executor.
    const SOURCES: &'static [u16];
}

impl sealed::Sources for () {}
impl UrgentSources for () {
    const SOURCES: &'static [u16] = &[];
}

/// A doorbell's admitted urgent source, named by its exact member type.
/// Only the doorbell constructor issues this zero-sized observation.
pub struct UrgentSource<Member: InterruptSource> {
    _member: PhantomData<fn() -> Member>,
}

impl<Member: InterruptSource> Copy for UrgentSource<Member> {}
impl<Member: InterruptSource> Clone for UrgentSource<Member> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Member: InterruptSource> UrgentSource<Member> {
    pub(crate) const fn new() -> Self {
        Self {
            _member: PhantomData,
        }
    }
}

impl<Member: InterruptSource> sealed::Sources for UrgentSource<Member> {}
impl<Member: InterruptSource> UrgentSources for UrgentSource<Member> {
    const SOURCES: &'static [u16] = &[Member::SOURCE];
}

pub(crate) fn pending(sources: &[u16], is_pending: impl FnMut(u16) -> bool) -> bool {
    sources.iter().copied().any(is_pending)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::{InterruptSource, UrgentSource, UrgentSources};

    struct FromCpu3Irq;
    impl oer_espressif_interrupt_table::InterruptSource for FromCpu3Irq {
        const SOURCE: u16 = 68;
    }

    fn sources<U: UrgentSources>(_: U) -> &'static [u16] {
        U::SOURCES
    }

    #[test]
    fn empty_composition_reads_no_pending_bank() {
        assert!(!super::pending(sources(()), |_| panic!(
            "no source was declared"
        )));
    }

    #[test]
    fn a_noncopy_members_urgent_observation_can_serve_both_local_executors() {
        let urgent = UrgentSource::<FromCpu3Irq>::new();
        assert_eq!(core::mem::size_of_val(&urgent), 0);
        assert_eq!(sources(urgent), &[FromCpu3Irq::SOURCE]);
        assert!(super::pending(sources(urgent), |source| source == FromCpu3Irq::SOURCE));
    }

    #[test]
    fn only_composed_sources_stop_admission() {
        let mut reads = std::vec::Vec::new();
        assert!(super::pending(&[71, 105], |source| {
            reads.push(source);
            source == 105
        }));
        assert_eq!(reads, [71, 105]);
        assert!(!super::pending(&[71], |source| source == 105));
    }
}
