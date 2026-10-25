//! The register model of a chip: block types, the chip root, memory
//! layouts and the ownership policy (`chip_model`, `chip_policy`), the thin
//! PAC renderer over them (`chip_render`) and the evidence catalogs every
//! fact cites.

#![forbid(unsafe_code)]

use std::{
    ops::Range,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub mod analog;
pub mod chip_model;
pub mod chip_policy;
pub mod chip_render;
mod descriptions;
mod layouts;
mod register_evidence;
mod review_defaults;

pub use descriptions::{description_directories, descriptions};
pub use layouts::{
    LayoutAccess, LayoutArray, LayoutField, LayoutKind, LayoutPhase, LayoutUnit, LayoutWriter,
    MemoryLayout, PointerEncoding,
};
pub use register_evidence::{
    RegisterEvidenceCatalog, RegisterEvidenceRange, RegisterEvidenceSet, RegisterEvidenceSource,
    SourceKind,
};
pub use review_defaults::ReviewMetadata;

/// One chip's register model as the repository holds it:
/// `chips/<chip>/registers/` with its root `chip.toml`, the block types and
/// layouts it includes, every evidence catalog of `evidence/` and the
/// ownership policy `policy.toml`.
pub struct Chip {
    /// `chips/<chip>/registers` below the checkout.
    pub directory: PathBuf,
    pub evidence: RegisterEvidenceSet,
    pub model: chip_model::ChipModel,
}

/// Why a chip's register model did not load.
#[derive(Debug, thiserror::Error)]
pub enum ChipError {
    #[error(transparent)]
    Evidence(#[from] Error),
    #[error(transparent)]
    Model(#[from] chip_model::Error),
    #[error(transparent)]
    Policy(#[from] chip_policy::Error),
}

impl Chip {
    /// The register directory of `chip` below the checkout `root`.
    pub fn directory(root: &Path, chip: &str) -> PathBuf {
        root.join("chips").join(chip).join("registers")
    }

    /// Load `chip`'s root and every evidence catalog below the checkout
    /// `root`.
    pub fn load(root: &Path, chip: &str) -> std::result::Result<Self, ChipError> {
        let directory = Self::directory(root, chip);
        let mut catalogs = Vec::new();
        for entry in std::fs::read_dir(directory.join("evidence")).map_err(Error::from)? {
            let path = entry.map_err(Error::from)?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                catalogs.push(path);
            }
        }
        catalogs.sort();
        let evidence = RegisterEvidenceSet::load_all(&catalogs)?;
        let model = chip_model::ChipModel::load(&directory.join("chip.toml"), &evidence)?;
        Ok(Self {
            directory,
            evidence,
            model,
        })
    }

    /// The ownership policy `policy.toml` over the root.
    pub fn policy(&self) -> std::result::Result<chip_policy::ChipPolicy, ChipError> {
        Ok(chip_policy::ChipPolicy::load(
            &self.directory.join("policy.toml"),
            &self.model,
            &self.evidence,
        )?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid register model: {0}")]
    Invalid(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml_edit::TomlError),

    #[error(transparent)]
    TomlDeserialize(#[from] toml_edit::de::Error),

    #[error(transparent)]
    TomlSerialize(#[from] toml_edit::ser::Error),

    #[error("invalid {kind} {path}: {reason}")]
    Manifest {
        kind: &'static str,
        path: PathBuf,
        reason: String,
        span: Option<Range<usize>>,
    },
}

impl Error {
    fn message(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    fn manifest(kind: &'static str, path: &Path, error: impl std::fmt::Display) -> Self {
        Self::manifest_span(kind, path, error, None)
    }

    fn manifest_span(
        kind: &'static str,
        path: &Path,
        error: impl std::fmt::Display,
        span: Option<Range<usize>>,
    ) -> Self {
        Self::Manifest {
            kind,
            path: path.to_owned(),
            reason: error.to_string(),
            span,
        }
    }

    /// Returns source-neutral manifest diagnostic data for a presentation layer.
    pub fn manifest_diagnostic(&self) -> Option<(&'static str, &Path, &str, Option<Range<usize>>)> {
        match self {
            Self::Manifest {
                kind,
                path,
                reason,
                span,
            } => Some((*kind, path, reason, span.clone())),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewAnnotation {
    pub entity: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<oer_registers_contracts::FactProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<oer_registers_contracts::FactAccuracy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<oer_registers_contracts::FactCompleteness>,
    /// Origin of a register or field name. An opaque name, and only an opaque
    /// name, ends in [`OPAQUE_SUFFIX`]; a vendor name cites its source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming: Option<oer_registers_contracts::NameOrigin>,
}

/// Suffix that marks a register or field whose meaning is not established.
pub const OPAQUE_SUFFIX: &str = "_OPAQUE";

/// Other spellings of unknown meaning; an opaque name uses [`OPAQUE_SUFFIX`].
const UNESTABLISHED_WORDS: [&str; 3] = ["UNKNOWN", "UNNAMED", "UNCLASSIFIED"];

/// Name words that record how the vendor writes bits rather than what they
/// mean: a name made of them describes no established meaning.
const WRITER_WORDS: [&[&str]; 3] = [&["SET", "BY"], &["FORCE", "ZERO"], &["FORCE", "ONE"]];

/// A name that states position or a writer's action instead of meaning:
/// a trailing register offset, bit positions alone, a writer's action, or a
/// complete written image. Such a name is opaque and ends in
/// [`OPAQUE_SUFFIX`], with the position inside it.
fn positional_reason(leaf: &str) -> Option<&'static str> {
    let words: Vec<&str> = leaf.split('_').collect();
    let offset = words.last().is_some_and(|word| {
        (3..=4).contains(&word.len())
            && word
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
            && word.chars().any(|c| c.is_ascii_digit())
    });
    if offset && words.len() > 1 {
        return Some("a register offset");
    }
    if words
        .iter()
        .all(|word| matches!(*word, "BIT" | "BITS") || word.chars().all(|c| c.is_ascii_digit()))
    {
        return Some("bit positions alone");
    }
    if WRITER_WORDS.iter().any(|pattern| {
        words
            .windows(pattern.len())
            .any(|window| window == *pattern)
    }) {
        return Some("the vendor's write action");
    }
    if matches!(words.as_slice(), ["IMAGE"] | ["INIT", "IMAGE"]) {
        return Some("a complete written image");
    }
    None
}

/// The only name an opaque register or field may carry: its position.
/// A register is `WORD_<offset>_OPAQUE` with its four-digit hexadecimal
/// offset in its parent, a field `BIT_<n>_OPAQUE` or
/// `BITS_<low>_<high>_OPAQUE`; an array inserts `_%s` before the suffix and
/// names the position of its first element.
fn opaque_position_name(position: Position, array: bool) -> String {
    let index = if array { "_%s" } else { "" };
    match position {
        Position::Register { offset } => format!("WORD_{offset:04X}{index}{OPAQUE_SUFFIX}"),
        Position::Field { offset, width: 1 } => format!("BIT_{offset}{index}{OPAQUE_SUFFIX}"),
        Position::Field { offset, width } => format!(
            "BITS_{offset}_{}{index}{OPAQUE_SUFFIX}",
            offset + width.saturating_sub(1)
        ),
    }
}

/// Where a register or field lies: a register's offset in its parent, a
/// field's lowest bit and width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Position {
    Register { offset: u32 },
    Field { offset: u32, width: u32 },
}

/// A register or field name claims no more meaning than its origin supports:
/// every one is reviewed as vendor, descriptive or opaque; an opaque entity,
/// and only an opaque one, ends in [`OPAQUE_SUFFIX`] and is named by its
/// position (`opaque_name`), no name spells unknown meaning another way,
/// and a vendor name cites the source it was taken from. A name of ours that
/// states position or a writer's action instead of meaning is opaque.
fn validate_naming(
    entity: &str,
    annotation: Option<&ReviewAnnotation>,
    opaque_name: &str,
) -> Result<()> {
    use oer_registers_contracts::NameOrigin;

    let leaf = entity.rsplit('.').next().unwrap_or(entity);
    if let Some(word) = UNESTABLISHED_WORDS
        .iter()
        .find(|word| leaf.contains(**word))
    {
        return Err(Error::message(format!(
            "register entity {entity:?} names unknown meaning with {word:?}; end it in {OPAQUE_SUFFIX:?} and review it as opaque"
        )));
    }
    let positional = opaque_name;
    let opaque_name = leaf.ends_with(OPAQUE_SUFFIX);
    if !opaque_name && leaf.contains(&OPAQUE_SUFFIX[1..]) {
        return Err(Error::message(format!(
            "register entity {entity:?} spells opaque inside its name; end it in {OPAQUE_SUFFIX:?}"
        )));
    }
    let Some(naming) = annotation.and_then(|annotation| annotation.naming) else {
        return Err(Error::message(format!(
            "register entity {entity:?} has no reviewed naming (vendor, descriptive or opaque)"
        )));
    };
    if naming == NameOrigin::Descriptive
        && let Some(reason) = positional_reason(leaf)
    {
        return Err(Error::message(format!(
            "register entity {entity:?} names {reason} instead of a meaning; name it by position with {OPAQUE_SUFFIX:?} and review it as opaque"
        )));
    }
    match Some(naming) {
        Some(NameOrigin::Opaque) if !opaque_name => Err(Error::message(format!(
            "register entity {entity:?} is reviewed as opaque but its name does not end in {OPAQUE_SUFFIX:?}"
        ))),
        Some(NameOrigin::Opaque) if leaf != positional => Err(Error::message(format!(
            "register entity {entity:?} is opaque but not named by its position; name it {positional:?}"
        ))),
        Some(NameOrigin::Vendor | NameOrigin::Descriptive) | None if opaque_name => {
            Err(Error::message(format!(
                "register entity {entity:?} ends in {OPAQUE_SUFFIX:?} but is not reviewed with naming = \"opaque\""
            )))
        }
        Some(NameOrigin::Vendor)
            if annotation.is_none_or(|annotation| annotation.sources.is_empty()) =>
        {
            Err(Error::message(format!(
                "register entity {entity:?} claims a vendor name without citing its source"
            )))
        }
        _ => Ok(()),
    }
}

fn mark_description(
    description: &mut Option<String>,
    naming: Option<&oer_registers_contracts::NameOrigin>,
) {
    use oer_registers_contracts::NameOrigin;

    let marker = match naming {
        Some(NameOrigin::Opaque) => "Opaque: meaning not established.",
        Some(NameOrigin::Descriptive) => "Project-assigned name.",
        Some(NameOrigin::Vendor) | None => return,
    };
    *description = Some(match description.take() {
        Some(text) if !text.trim().is_empty() => format!("{marker} {text}"),
        _ => marker.to_owned(),
    });
}

fn review_annotation_is_assertion(annotation: &ReviewAnnotation) -> bool {
    !annotation.sources.is_empty()
        && annotation
            .provenance
            .is_some_and(|provenance| provenance != oer_registers_contracts::FactProvenance::Hint)
        && annotation.accuracy.is_some()
        && annotation.completeness.is_some()
}

pub(crate) fn validate_relative_fragment(value: &str) -> Result<()> {
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::message(format!(
            "register model fragment must be a safe relative path: {value:?}"
        )));
    }
    Ok(())
}
