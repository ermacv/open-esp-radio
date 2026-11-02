//! Each executor's core and the start of the task poll it runs now, packed
//! in one word for cooperative admission.

#[cfg(test)]
pub(crate) mod tests;

/// The poll stamp of a chip with `HARTS` harts: the low bits identify
/// every hart and one unassigned sentinel; the remaining bits carry a
/// wrapping poll timestamp, whose period is 2^(32 - hart bits)
/// microseconds.
pub(crate) struct Stamp<const HARTS: usize>;

/// The stamp of the chip the platform port's geometry describes.
#[cfg(feature = "chip")]
pub(crate) type ChipStamp = Stamp<{ oer_espressif_platform_port::HARTS }>;

impl<const HARTS: usize> Stamp<HARTS> {
    const HART_BITS: u32 = HARTS.ilog2() + 1;
    const HART_MASK: usize = (1 << Self::HART_BITS) - 1;
    pub(crate) const STAMP_MASK: u32 = u32::MAX >> Self::HART_BITS;
    const GEOMETRY: () = {
        if Self::HART_BITS >= u32::BITS {
            panic!("the poll stamp retains timestamp bits");
        }
        if POLL_ADMISSION_MICROS >= Self::STAMP_MASK {
            panic!("the admission budget fits before timestamp wrap");
        }
    };

    pub(crate) fn assigned_core(state: usize) -> Option<usize> {
        let () = const { Self::GEOMETRY };
        let core = state & Self::HART_MASK;
        (core < HARTS).then_some(core)
    }
    pub(crate) fn poll_stamp(state: usize, now: u64) -> usize {
        let () = const { Self::GEOMETRY };
        (state & Self::HART_MASK) | (((now as u32 & Self::STAMP_MASK) as usize) << Self::HART_BITS)
    }
    pub(crate) fn poll_elapsed(state: usize, now: u64) -> u32 {
        let () = const { Self::GEOMETRY };
        (now as u32).wrapping_sub((state >> Self::HART_BITS) as u32) & Self::STAMP_MASK
    }
}

/// Reserve 100 us for admission, one <=50-us atom and the return path.
pub(crate) const POLL_ADMISSION_MICROS: u32 = oer_cooperate::POLL_ADMISSION_MICROS;
