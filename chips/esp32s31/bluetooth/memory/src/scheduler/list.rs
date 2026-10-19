//! Ordered software mirror of one scheduler hardware list.
//!
//! The Controller executes a hardware list as a chain of items linked in start
//! order. This mirror holds the same order for event identities chosen by the
//! executor and decides where an event belongs before any descriptor or
//! register is touched. It never moves an event: overlapping windows are
//! rejected, because conflict resolution belongs to the portable arbiter.
//!
//! Every live window starts within one forward half-range of the list head,
//! so signed wrapping differences order the whole list.

#![forbid(unsafe_code)]

use crate::scheduler::window::{MAX_FORWARD_SPAN, SchedulerRawWindow};

/// Why an event cannot enter the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerListInsertError<I> {
    /// The identity is already listed.
    Duplicate,
    /// The window shares time with a listed event.
    Overlap { with: I },
    /// The window starts outside the forward half-range of the list head.
    OutsideForwardRange,
    /// Every entry is occupied.
    Full,
}

/// Neighbours of an inserted event in list order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerListPlacement<I> {
    /// The event before the inserted one; `None` makes it the new head.
    pub predecessor: Option<I>,
    /// The event after the inserted one; `None` makes it the new tail.
    pub successor: Option<I>,
}

impl<I> SchedulerListPlacement<I> {
    /// Whether the inserted event becomes the list head.
    pub const fn is_head(&self) -> bool {
        self.predecessor.is_none()
    }
}

/// Neighbours and window of a removed event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerListRemoval<I> {
    pub predecessor: Option<I>,
    pub successor: Option<I>,
    pub window: SchedulerRawWindow,
}

/// Result of scanning the list from its head for executed events.
///
/// This follows the vendor completion walk: executed events are taken from
/// the head, one unexecuted event may be passed, and an executed event behind
/// it is reported as an out-of-order completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerListCompletionScan {
    /// Number of leading events that executed without a gap.
    pub completed_prefix: usize,
    /// Whether an executed event follows the first unexecuted one.
    pub out_of_order: bool,
}

#[derive(Clone, Copy)]
struct Entry<I> {
    id: I,
    window: SchedulerRawWindow,
}

/// Fixed-capacity ordered mirror of one hardware list.
pub struct SchedulerList<I, const CAPACITY: usize> {
    entries: [Option<Entry<I>>; CAPACITY],
    len: usize,
}

impl<I: Copy + Eq, const CAPACITY: usize> SchedulerList<I, CAPACITY> {
    /// Construct an empty list.
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The head event and its window.
    pub fn head(&self) -> Option<(I, SchedulerRawWindow)> {
        self.entry(0).map(|entry| (entry.id, entry.window))
    }

    /// Events and windows in list order.
    pub fn iter(&self) -> impl Iterator<Item = (I, SchedulerRawWindow)> + '_ {
        self.entries[..self.len]
            .iter()
            .flatten()
            .map(|entry| (entry.id, entry.window))
    }

    /// Whether `id` is listed.
    pub fn contains(&self, id: I) -> bool {
        self.position(id).is_some()
    }

    /// Window of a listed event.
    pub fn window(&self, id: I) -> Option<SchedulerRawWindow> {
        self.position(id)
            .and_then(|index| self.entry(index))
            .map(|entry| entry.window)
    }

    /// Where `window` would be inserted, without changing the list.
    pub fn plan_insert(
        &self,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<SchedulerListPlacement<I>, SchedulerListInsertError<I>> {
        self.locate(id, window)
            .map(|index| self.placement_at(index))
    }

    /// Insert `window` for `id` in start order.
    pub fn insert(
        &mut self,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<SchedulerListPlacement<I>, SchedulerListInsertError<I>> {
        let index = self.locate(id, window)?;
        let placement = self.placement_at(index);
        self.entries[index..=self.len].rotate_right(1);
        self.entries[index] = Some(Entry { id, window });
        self.len += 1;
        Ok(placement)
    }

    /// Remove `id` and report the neighbours that must be relinked.
    pub fn remove(&mut self, id: I) -> Option<SchedulerListRemoval<I>> {
        let index = self.position(id)?;
        let window = self.entry(index)?.window;
        let predecessor = index
            .checked_sub(1)
            .and_then(|previous| self.entry(previous))
            .map(|entry| entry.id);
        let successor = self.entry(index + 1).map(|entry| entry.id);
        self.entries[index..self.len].rotate_left(1);
        self.len -= 1;
        self.entries[self.len] = None;
        Some(SchedulerListRemoval {
            predecessor,
            successor,
            window,
        })
    }

    /// Scan from the head with the executed predicate of each event.
    pub fn scan_completion(
        &self,
        mut executed: impl FnMut(I) -> bool,
    ) -> SchedulerListCompletionScan {
        let mut completed_prefix = 0;
        let mut passed_unexecuted = false;
        for (id, _) in self.iter() {
            match (executed(id), passed_unexecuted) {
                (true, false) => completed_prefix += 1,
                (true, true) => {
                    return SchedulerListCompletionScan {
                        completed_prefix,
                        out_of_order: true,
                    };
                }
                (false, false) => passed_unexecuted = true,
                (false, true) => break,
            }
        }
        SchedulerListCompletionScan {
            completed_prefix,
            out_of_order: false,
        }
    }

    fn entry(&self, index: usize) -> Option<Entry<I>> {
        if index < self.len {
            self.entries[index]
        } else {
            None
        }
    }

    fn position(&self, id: I) -> Option<usize> {
        self.entries[..self.len]
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.id == id))
    }

    fn placement_at(&self, index: usize) -> SchedulerListPlacement<I> {
        SchedulerListPlacement {
            predecessor: index
                .checked_sub(1)
                .and_then(|previous| self.entry(previous))
                .map(|entry| entry.id),
            successor: self.entry(index).map(|entry| entry.id),
        }
    }

    /// Index at which `window` keeps the list ordered and non-overlapping.
    fn locate(
        &self,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<usize, SchedulerListInsertError<I>> {
        if self.position(id).is_some() {
            return Err(SchedulerListInsertError::Duplicate);
        }
        if self.len == CAPACITY {
            return Err(SchedulerListInsertError::Full);
        }
        if let (Some((_, head)), Some(tail)) = (
            self.head(),
            self.len.checked_sub(1).and_then(|last| self.entry(last)),
        ) {
            // The whole list, including the new start, must span at most one
            // forward half-range.
            let span = if (window.start().wrapping_sub(head.start()) as i32) >= 0 {
                window.start().wrapping_sub(head.start())
            } else {
                tail.window.start().wrapping_sub(window.start())
            };
            if span > MAX_FORWARD_SPAN {
                return Err(SchedulerListInsertError::OutsideForwardRange);
            }
        }
        let index = self
            .iter()
            .position(|(_, listed)| (listed.start().wrapping_sub(window.start()) as i32) > 0)
            .unwrap_or(self.len);
        for neighbour in [index.checked_sub(1), Some(index)].into_iter().flatten() {
            if let Some(entry) = self.entry(neighbour)
                && entry.window.strictly_overlaps(window)
            {
                return Err(SchedulerListInsertError::Overlap { with: entry.id });
            }
        }
        Ok(index)
    }
}

impl<I: Copy + Eq, const CAPACITY: usize> Default for SchedulerList<I, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
