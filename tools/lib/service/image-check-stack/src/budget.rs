//! The resources of one image analysis: the working memory it may reserve,
//! the work units it may charge and the time it may take. Running out of
//! any fails the analysis with `ResourceLimited` instead of running on.
//!
//! The owner chose the limits from the measured analysis of every image
//! class
//! (<https://github.com/the predecessor repository/issues/524#issuecomment->),
//! and kept 512 MiB of working memory once it accounted the whole analysis:
//! about twice the largest image's accounted peak, 240 MB for the former
//! wifi-ble-coex runtime image (tag `legacy-wifi-2026-10-16`)
//! (<https://github.com/the predecessor repository/issues/524#issuecomment->).
//! The work budget is the primary bound: it does not depend on the
//! machine's load. The deadline is a backstop. Working memory covers the
//! image's and the ROM's bytes, their facts, the value analysis, and every
//! structure of the stack analysis that grows with the image: the facts it
//! keeps of each function, the DWARF and type facts, and the resolutions
//! (`oer-tool-analysis-riscv-stack`). Outside it stay the caches addr2line grows for the
//! inline chains, which the working memory's contract leaves to the
//! library, and the bounds the stack checks compute afterwards. The runtime and
//! bootstrap stack reports state what the image's analysis used of these
//! limits; the interrupt check reads its image facts within the same limits.
use oer_tool_analysis_riscv_model::{Error, ErrorCode, Result, RunControl, RunPosition};
use std::time::{Duration, Instant};

/// Working memory one image analysis may reserve.
pub const MEMORY_LIMIT: u64 = 512 << 20;
/// Work units one image analysis may charge.
pub const WORK_UNITS: u64 = 1_000_000_000;
/// Wall-clock time one image analysis may take.
pub const TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Checkpoints between wall-clock checks.
const DEADLINE_INTERVAL: u32 = 256;

/// What one image analysis used of its resources.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    /// The most working memory reserved at once, in bytes.
    pub memory_bytes: u64,
    pub work_units: u64,
}

/// The work units and deadline of one image analysis.
pub struct Budget {
    units: u64,
    limit: u64,
    deadline: Instant,
    checks: u32,
    position: RunPosition,
}

impl Budget {
    pub fn new(units: u64, timeout: Duration) -> Self {
        Self {
            units: 0,
            limit: units,
            deadline: Instant::now() + timeout,
            checks: 0,
            position: RunPosition::default(),
        }
    }

    /// The work units charged so far.
    pub fn used(&self) -> u64 {
        self.units
    }
}

impl RunControl for Budget {
    fn position(&self) -> RunPosition {
        self.position
    }
    fn set_position(&mut self, position: RunPosition) {
        self.position = position;
    }
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.units = self.units.saturating_add(units);
        if self.units > self.limit {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                format!("the image analysis exceeds its {} work units", self.limit),
            ));
        }
        self.checks += 1;
        if self.checks == DEADLINE_INTERVAL {
            self.checks = 0;
            if Instant::now() > self.deadline {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "the image analysis exceeds its time limit",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_admits_exactly_its_units() {
        let mut budget = Budget::new(10, TIMEOUT);
        budget.checkpoint(4).unwrap();
        budget.checkpoint(6).unwrap();
        assert_eq!(budget.used(), 10);
        let error = budget.checkpoint(1).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
    }

    #[test]
    fn a_passed_deadline_stops_the_analysis_at_the_next_clock_check() {
        let mut budget = Budget::new(u64::MAX, Duration::ZERO);
        for _ in 1..DEADLINE_INTERVAL {
            budget.checkpoint(0).unwrap();
        }
        let error = budget.checkpoint(0).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert!(error.message.contains("time limit"), "{error:?}");
    }
}
