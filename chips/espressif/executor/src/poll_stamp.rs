//! Each executor's core and the start of the task poll it runs now, packed
//! in one word for cooperative admission.

#[cfg(test)]
pub(crate) mod tests;

// Two low bits identify the registered core (0/1); 3 remains unassigned.
// The existing core-assignment word also carries a wrapping poll timestamp.
// A live poll is bounded far below the 2^30-microsecond timestamp period.
const STAMP_MASK: u32 = (1 << 30) - 1;
pub(crate) fn assigned_core(state: usize) -> Option<usize> {
    let core = state & 3;
    (core < 2).then_some(core)
}
pub(crate) fn poll_stamp(state: usize, now: u64) -> usize {
    (state & 3) | (((now as u32 & STAMP_MASK) as usize) << 2)
}
pub(crate) fn poll_elapsed(state: usize, now: u64) -> u32 {
    (now as u32).wrapping_sub((state >> 2) as u32) & STAMP_MASK
}
/// Reserve 100 us for admission, one <=50-us atom and the return path.
pub(crate) const POLL_ADMISSION_MICROS: u32 = oer_cooperate::POLL_ADMISSION_MICROS;
