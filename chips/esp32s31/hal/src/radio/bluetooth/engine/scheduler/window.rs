//! Raw-tick scheduler windows in the wrapping Controller time domain.
//!
//! Controller time wraps, so windows are ordered by signed differences. A
//! window is valid only when its duration lies in the forward half-range,
//! which keeps every comparison between live windows unambiguous.

#![forbid(unsafe_code)]

/// Longest forward span that signed wrapping comparisons order unambiguously.
pub(crate) const MAX_FORWARD_SPAN: u32 = i32::MAX as u32;

/// Raw-tick scheduler window `[start, end)` inside one forward half-range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerRawWindow {
    start: u32,
    end: u32,
}

impl SchedulerRawWindow {
    pub(crate) const fn new(start: u32, end: u32) -> Option<Self> {
        let duration = end.wrapping_sub(start);
        if duration == 0 || duration > MAX_FORWARD_SPAN {
            None
        } else {
            Some(Self { start, end })
        }
    }

    /// Bind a projected scheduler window before timeline admission.
    pub const fn from_projected_scheduler_window(start: u32, end: u32) -> Option<Self> {
        Self::new(start, end)
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }

    pub const fn duration(self) -> u32 {
        self.end.wrapping_sub(self.start)
    }

    /// Whether the windows share time. Touching boundaries do not overlap.
    pub(crate) const fn strictly_overlaps(self, other: Self) -> bool {
        (other.end.wrapping_sub(self.start) as i32) > 0
            && (self.end.wrapping_sub(other.start) as i32) > 0
    }
}
