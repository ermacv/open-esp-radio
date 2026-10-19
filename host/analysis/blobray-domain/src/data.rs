//! Exact data observations. No runtime-memory claim.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DataSelector {
    /// Explicit section-relative bytes, including data without a sized symbol.
    Section {
        section: u32,
        offset: u64,
        length: u64,
    },
    /// A defined physical static or dynamic symbol; a zero-sized symbol requires an explicit length.
    Symbol {
        symbol: SymbolId,
        length: Option<u64>,
    },
    /// Virtual address in an executable ELF, never an object/file offset.
    Image { address: u64, length: u64 },
}
/// Exact bytes of one captured object, named by content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRequest {
    pub object: ObjectId,
    /// A symbol the object must define, when the ranges are about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<SymbolId>,
    pub ranges: Vec<DataSelector>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSpan {
    pub selector: DataSelector,
    pub section: u32,
    pub section_range: CodeRange,
    pub file_range: CodeRange,
    pub image_address: Option<u64>,
    /// True means captured initialization bytes, not an immutable runtime value.
    pub writable: bool,
    pub digest: ArtifactId,
    pub export_offset: u64,
    /// All relocations in this section are supplied; none are silently applied.
    pub section_relocations: u64,
    /// Known relocation write extents intersecting the selected byte range.
    pub overlapping_relocations: u64,
    /// Section relocations whose write extent the parser cannot establish.
    /// Such relocations prevent numeric interpretation even outside the range.
    pub unknown_relocation_extents: u64,
}
/// A physical object span; absent file backing (e.g. NOBITS) never invents bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataLocation {
    pub selector: DataSelector,
    pub section: u32,
    pub section_range: CodeRange,
    pub file_range: Option<CodeRange>,
    pub image_address: Option<u64>,
    /// ELF section flag only; not a claim about runtime memory permissions.
    pub section_writable: bool,
}
