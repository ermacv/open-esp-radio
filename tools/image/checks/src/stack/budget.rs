//! The resources of one image analysis: the working memory it may reserve,
//! the work units it may charge and the time it may take. Running out of
//! any fails the analysis with `ResourceLimited` instead of running on.
//!
//! Working memory is capped at 1 GiB for the full station/IP image together
//! with the pinned ROM, typed debug facts and interrupt analysis.
//! The work budget is the primary bound: it does not depend on the
//! machine's load. The deadline is a backstop. Working memory covers the
//! image's and the ROM's bytes, their facts, the value analysis, and every
//! structure of the stack analysis that grows with the image: the facts it
//! keeps of each function, the DWARF and type facts, and the resolutions
//! (`oer-analysis-riscv-stack`). Outside it stay the caches addr2line grows for the
//! inline chains, which the working memory's contract leaves to the
//! library, and the pre-existing bound scratch and critical-path structures.
//! First-discovery predecessors are admitted by each bound's caller through
//! a ledger, alongside the image's retained facts, within these same limits.
//! The runtime and bootstrap stack reports state the original image-analysis
//! usage; the separate reachability reports include the peak with witnesses.
//! The interrupt check reads its image facts within the same limits.
use oer_analysis_riscv_model::{Error, ErrorCode, Result, RunControl, RunPosition};
use std::time::{Duration, Instant};

/// Working memory one image analysis may reserve.
// The complete station + owned IP stack needs room beside the pinned ROM
// analysis; this caps host working memory, independently of image RAM.
pub const MEMORY_LIMIT: u64 = 1024 << 20;
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
