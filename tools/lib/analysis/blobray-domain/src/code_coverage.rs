//! Vendor code coverage of the root closures that executions exercise.
use crate::*;

/// Upper bound of distinct functions in one closure report.
pub const MAX_CLOSURE_FUNCTIONS: usize = 4096;
/// Upper bound of decoded instructions over one closure report.
pub const MAX_CLOSURE_INSTRUCTIONS: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageCount {
    pub reached: u64,
    pub total: u64,
}
impl CoverageCount {
    pub fn complete(&self) -> bool {
        self.reached == self.total
    }
}

/// One conditional branch direction that no execution took.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchDirection {
    pub site: u32,
    pub taken: bool,
}

/// Statically reachable code of one function entry: its basic blocks and
/// conditional branches, and the boundaries where the closure stops.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureFunction {
    pub entry: u32,
    /// Decoded code as merged half-open `[start, end)` address ranges.
    pub code: Vec<[u32; 2]>,
    /// Basic-block leaders, ascending.
    pub blocks: Vec<u32>,
    /// Conditional branch sites, ascending.
    pub branches: Vec<u32>,
    /// Direct and resolved callees, including tail transfers, ascending.
    pub callees: Vec<u32>,
    /// Transfer sites whose target is a declared boundary (call model, FIFO
    /// service binding or execution goal); the closure does not enter it.
    pub modeled: Vec<u32>,
    /// Indirect transfer sites without a statically known target, and direct
    /// transfers leaving the executable captured code.
    pub unresolved: Vec<u32>,
    /// Indirect transfer sites without a statically known target that the
    /// closure follows only to their executed targets: targets no execution
    /// reached, such as the other arms of a jump table, stay outside it.
    pub followed: Vec<u32>,
    /// Instructions the decoder does not support.
    pub gaps: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionCodeCoverage {
    pub entry: u32,
    /// Defined code symbol starting at `entry`, when there is one.
    pub name: Option<String>,
    pub blocks: CoverageCount,
    /// Both directions of every conditional branch.
    pub directions: CoverageCount,
    pub uncovered_blocks: Vec<u32>,
    pub uncovered_directions: Vec<BranchDirection>,
    /// Entries of the closure functions this one calls or tail-transfers
    /// to, ascending: the closure's call graph.
    pub callees: Vec<u32>,
    pub modeled: Vec<u32>,
    pub unresolved: Vec<u32>,
    pub followed: Vec<u32>,
    pub gaps: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootCoverage {
    pub entry: u32,
    pub name: Option<String>,
    /// Entries of every function in the root's closure, ascending.
    pub functions: Vec<u32>,
    /// Distinct blocks and directions over the closure.
    pub blocks: CoverageCount,
    pub directions: CoverageCount,
}

/// Vendor coverage of the root closures of executions sharing one vendor target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeCoverageReport {
    pub executions: Vec<ArtifactId>,
    /// Every distinct vendor invocation entry, ascending.
    pub roots: Vec<RootCoverage>,
    /// Every closure function, ascending by entry.
    pub functions: Vec<FunctionCodeCoverage>,
    /// Executed vendor instructions outside every closure, for example code
    /// reached through an unresolved indirect transfer.
    pub outside: u64,
}
