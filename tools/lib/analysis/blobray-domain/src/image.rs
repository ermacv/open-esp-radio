//! Portable synthetic image requests and linker evidence. No tool discovery or I/O.
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRegion {
    pub start: u32,
    pub length: u64,
}
impl ImageRegion {
    pub fn end(&self) -> Option<u64> {
        u64::from(self.start)
            .checked_add(self.length)
            .filter(|end| *end <= 1u64 << 32)
    }
    pub fn contains(&self, start: u64, length: u64) -> bool {
        start >= u64::from(self.start)
            && start
                .checked_add(length)
                .zip(self.end())
                .is_some_and(|(end, limit)| end <= limit)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageLayout {
    pub code: ImageRegion,
    pub data: ImageRegion,
}
impl ImageLayout {
    pub fn validate(&self) -> Result<()> {
        for region in [self.code, self.data] {
            if region.length == 0 || region.start % 4096 != 0 || region.end().is_none() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "image regions must be nonempty, page-aligned and within RV32",
                ));
            }
        }
        if u64::from(self.code.start) < self.data.end().unwrap()
            && u64::from(self.data.start) < self.code.end().unwrap()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "image regions overlap",
            ));
        }
        Ok(())
    }
}
/// One synthetic image of captured objects, every executable named by content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    /// Exact definitions in captured static ELF executables, bound to their
    /// names at their addresses. No implicit definitions.
    #[serde(default)]
    pub companions: Vec<SymbolId>,
    /// Captured archives and objects, in linker order. No implicit inputs.
    pub inputs: Vec<ArtifactId>,
    /// The entry symbol; its object is in one of `inputs`.
    pub entry: SymbolId,
    #[serde(default)]
    pub roots: Vec<SymbolId>,
    pub layout: ImageLayout,
    /// Undefined names no captured input defines, deliberately bound to
    /// `ABSENT_SYMBOL_ADDRESS`: executing or reading one stops with an
    /// execution gap, so only never-reached references may name them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
}
/// Companions proposed for a link request by a trial link. Every name the
/// selected closure leaves unresolved is either resolved to exactly one defined
/// function or data object of the first candidate executable that defines it,
/// or reported unresolved. A proposal grants nothing: the client copies the
/// exact symbols into its link request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanionProposal {
    pub resolved: Vec<ProposedCompanion>,
    /// Linker-visible names without a definition in any candidate.
    pub unresolved: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedCompanion {
    pub name: String,
    pub symbol: SymbolId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkerIdentity {
    pub implementation: String,
    pub version: String,
    pub executable: ArtifactId,
}
/// Roots of one linked image: the entry and every additional root.
pub const MAX_IMAGE_ROOTS: usize = 64;
/// Unmapped address every absent name resolves to.
pub const ABSENT_SYMBOL_ADDRESS: u32 = 0xffff_fff0;
/// Absent names one link request may declare.
pub const MAX_ABSENT_SYMBOLS: usize = 64;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRoot {
    pub symbol: SymbolId,
    pub address: u64,
    pub size: u64,
    #[serde(with = "oer_riscv_model::symbol_name")]
    pub name: Vec<u8>,
}
/// One linked image: what was linked, by which linker, and where every
/// root and placed input section lies.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageManifest {
    pub schema: u32,
    pub request: LinkRequest,
    pub contract: LinkerContract,
    pub linker: LinkerIdentity,
    pub abi: RiscvAbi,
    /// Content identity of the linked ELF.
    pub elf: ArtifactId,
    pub entry: u64,
    pub roots: Vec<ResolvedRoot>,
    pub segments: Vec<ImageSegment>,
    pub mappings: Vec<ImageMapping>,
    /// Bounded observations from the successful linker process.
    pub linker_diagnostics: LinkerDiagnostics,
}
/// One input section the linker placed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMapping {
    pub object: ObjectId,
    pub payload: ArtifactId,
    pub section: Vec<u8>,
    pub address: u64,
    pub size: u64,
    /// Exact only when one original section is identified and its extent is unchanged.
    pub exact: bool,
}

/// The successful tool exit and the last 8192 stderr bytes; never an unbounded log.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkerDiagnostics {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr_tail: Vec<u8>,
    pub stderr_truncated: bool,
}

/// Semantic analysis profile; tool version is independently retained in identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkerContract {
    #[serde(rename = "static-analysis-elf-link-v1")]
    ElfAnalysisLinkV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkEvidenceSource {
    Map,
    Extraction,
}

/// Byte interval in a retained raw output, before normalization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkEvidenceSpan {
    pub source: LinkEvidenceSource,
    pub offset: u64,
    pub length: u64,
}

/// Adapter observations are claims checked by application, not publication authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LinkObservation {
    SectionPlacement {
        object: ObjectId,
        section: Vec<u8>,
        address: u64,
        size: u64,
        evidence: LinkEvidenceSpan,
    },
    ArchiveExtraction {
        object: ObjectId,
        cause: Option<Vec<u8>>,
        referring: Option<ObjectId>,
        evidence: LinkEvidenceSpan,
    },
    ToolExit {
        code: Option<i32>,
        signal: Option<i32>,
    },
}
