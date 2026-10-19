//! Captured inventory, execution, comparison and request records over the
//! identities and contracts of `oer-analysis-riscv-model`. No filesystem access.

#![forbid(unsafe_code)]

use oer_analysis_riscv_model::*;
use serde::{Deserialize, Serialize};

mod data;
pub use data::*;
mod mmio;
pub use mmio::*;
mod registers;
pub use registers::*;
mod execution;
pub use execution::*;
mod code_coverage;
pub use code_coverage::*;
mod command_bank;
pub use command_bank::*;
mod image;
pub use image::*;
mod stream;
pub use stream::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCode {
    UnavailableInput,
    UnsupportedFormat,
    MalformedContainer,
    MalformedObject,
    MalformedName,
    MalformedTable,
    MissingMember,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub context: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerKind {
    Elf,
    Archive,
    ThinArchive,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInventory {
    pub kind: ContainerKind,
    /// False if archive framing prevents enumeration of the remaining payloads.
    pub members_complete: bool,
    pub objects: Vec<ObjectInventory>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ArtifactInventory {
    pub fn complete(&self) -> bool {
        self.members_complete
            && self.diagnostics.is_empty()
            && self
                .objects
                .iter()
                .all(|o| o.elf.is_some() && o.diagnostics.is_empty())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectInventory {
    pub id: ObjectId,
    #[serde(with = "oer_analysis_riscv_model::symbol_name::option")]
    pub name: Option<Vec<u8>>,
    pub content: Option<ArtifactId>,
    pub elf: Option<ElfInventory>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElfInventory {
    pub bits: u8,
    pub little_endian: bool,
    pub machine: u16,
    pub object_type: u16,
    pub entry: u64,
    pub sections: Vec<SectionRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub relocations: Vec<RelocationRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionRecord {
    pub index: u32,
    #[serde(with = "oer_analysis_riscv_model::symbol_name::option")]
    pub name: Option<Vec<u8>>,
    pub section_type: u32,
    pub flags: u64,
    pub address: u64,
    pub file_offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub alignment: u64,
    pub entry_size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolRecord {
    pub id: SymbolId,
    #[serde(with = "oer_analysis_riscv_model::symbol_name::option")]
    pub name: Option<Vec<u8>>,
    pub name_offset: u32,
    pub value: u64,
    pub size: u64,
    pub binding: u8,
    pub symbol_type: u8,
    pub other: u8,
    pub raw_section: u16,
    pub extended_section: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelocationRecord {
    pub section: u32,
    pub index: u64,
    pub offset: u64,
    pub relocation_type: u32,
    pub symbol_table_section: u32,
    pub symbol_index: u32,
    pub addend: Option<i64>,
}

mod audit;
pub use audit::*;

mod device;
pub use device::*;

mod external_call;
pub use external_call::*;

mod comparison;
pub use comparison::*;

mod call_observation;
pub use call_observation::*;

mod call_boundary;
pub use call_boundary::*;

mod timeline;
pub use timeline::*;

mod effect_contract;
pub use effect_contract::*;
mod layout_projection;
pub use layout_projection::*;
