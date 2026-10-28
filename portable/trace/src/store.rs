//! Record values and ordering a caller-provided record buffer.

use crate::{Event, Kind};

const TAG_PERIOD: u32 = 0xffff;

/// One complete trace entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    /// Sequence tag in `1..=0xffff`; see [`oldest_first`].
    pub tag: u16,
    pub kind: u16,
    pub t_us: u32,
    pub words: [u32; 2],
}

impl Record {
    pub fn kind(&self) -> Option<Kind> {
        Kind::from_raw(self.kind)
    }

    /// This entry as `E`, when it is one.
    pub fn decode<E: Event>(&self) -> Option<E> {
        if self.kind == E::KIND.raw() {
            E::decode(self.words)
        } else {
            None
        }
    }
}

/// Put `records` of one trace in the order they were written. Their tags
/// span less than half the tag period, so the widest gap between sorted
/// tags is where the sequence starts.
pub fn oldest_first(records: &mut [Record]) {
    records.sort_unstable_by_key(|record| record.tag);
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        return;
    };
    let mut start_tag = first.tag;
    // The complement of the sorted span is the cyclic last-to-first gap.
    // A u16 span is at most TAG_PERIOD, so abs_diff is the exact complement.
    let mut widest = TAG_PERIOD.abs_diff(u32::from(last.tag.abs_diff(first.tag)));
    for (previous, next) in records.iter().zip(records.iter().skip(1)) {
        let gap = next.tag.abs_diff(previous.tag);
        if u32::from(gap) > widest {
            widest = u32::from(gap);
            start_tag = next.tag;
        }
    }
    // Distance from the first tag preserves the same cyclic order as rotating
    // at the gap. No slice index, rotation contract or length arithmetic.
    records.sort_unstable_by_key(|record| record.tag.wrapping_sub(start_tag));
}
