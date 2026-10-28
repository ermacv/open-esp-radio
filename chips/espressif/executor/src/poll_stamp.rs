//! Each executor's core and the start of the task poll it runs now, packed
//! in one word for cooperative admission.

#[cfg(test)]
pub(crate) mod tests;

use oer_espressif_platform_port::HARTS;

// The low bits identify every fact-defined hart and one unassigned sentinel.
// The existing core-assignment word also carries a wrapping poll timestamp.
// The wrapping timestamp period is 2^(32 - HART_BITS) microseconds.
const HART_BITS: u32 = HARTS.ilog2() + 1;
const HART_MASK: usize = (1 << HART_BITS) - 1;
const STAMP_MASK: u32 = u32::MAX >> HART_BITS;
const _: () = {
    assert!(
        HART_BITS < u32::BITS,
        "the poll stamp retains timestamp bits"
    );
    assert!(
        POLL_ADMISSION_MICROS < STAMP_MASK,
        "the admission budget fits before timestamp wrap"
    );
};
pub(crate) fn assigned_core(state: usize) -> Option<usize> {
    let core = state & HART_MASK;
    (core < HARTS).then_some(core)
}
pub(crate) fn poll_stamp(state: usize, now: u64) -> usize {
    (state & HART_MASK) | (((now as u32 & STAMP_MASK) as usize) << HART_BITS)
}
pub(crate) fn poll_elapsed(state: usize, now: u64) -> u32 {
    (now as u32).wrapping_sub((state >> HART_BITS) as u32) & STAMP_MASK
}
/// Reserve 100 us for admission, one <=50-us atom and the return path.
pub(crate) const POLL_ADMISSION_MICROS: u32 = oer_cooperate::POLL_ADMISSION_MICROS;
