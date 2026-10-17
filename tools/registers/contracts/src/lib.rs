//! Reviewed identities, applicability and evidence classification; no execution authority.
#![forbid(unsafe_code)]
mod fact;
mod identity;
pub use fact::{
    Applicability, ApplicabilityContext, EffectiveFactMetadata, EvidenceReference,
    FactClassification, FactValidationError, RecordMetadata,
};
pub use identity::{
    ArtifactIdentity, EntityDomain, IdentityError, RevisionOccurrenceId, SemanticEntityId,
    SemanticPath,
};

/// Origin of one asserted fact. A hint is navigation metadata only and must
/// never be promoted to a reviewed hardware meaning by generic analysis.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FactProvenance {
    Observed,
    Derived,
    Imported,
    Hint,
    Reviewed,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FactAccuracy {
    Exact,
    Bounded,
    Approximate,
    Unknown,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FactCompleteness {
    Complete,
    Partial,
    Unknown,
}

/// Where the name of a register or field comes from, and so how much of its
/// meaning the name may claim.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum NameOrigin {
    /// The vendor's own name, from a cited header, SVD or manual.
    Vendor,
    /// A name this project assigned from established behavior.
    Descriptive,
    /// The meaning is not established; the name only identifies the bits and
    /// ends in `_OPAQUE`.
    Opaque,
}
