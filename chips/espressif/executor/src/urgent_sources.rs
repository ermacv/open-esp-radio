//! Cooperative admission observes exactly the sources its image composes.

pub(crate) fn pending(sources: &[u16], is_pending: impl FnMut(u16) -> bool) -> bool {
    sources.iter().copied().any(is_pending)
}

#[cfg(test)]
mod tests {
    extern crate std;

    #[test]
    fn empty_composition_reads_no_pending_bank() {
        assert!(!super::pending(&[], |_| panic!("no source was declared")));
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
