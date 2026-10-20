//! Per-layout review templates, resolved before the existing review checks.

use oer_registers_contracts::{FactAccuracy, FactCompleteness, FactProvenance, NameOrigin};
use serde::Deserialize;

use crate::review_defaults::ReviewMetadata;

/// The ordinary template and an optional independent opaque template.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct LayoutReviewDefaults {
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    provenance: Option<FactProvenance>,
    #[serde(default)]
    accuracy: Option<FactAccuracy>,
    #[serde(default)]
    completeness: Option<FactCompleteness>,
    #[serde(default)]
    naming: Option<NameOrigin>,
    #[serde(default)]
    opaque: Option<ReviewMetadata>,
}

impl LayoutReviewDefaults {
    pub(super) fn metadata(&self, opaque: bool) -> ReviewMetadata {
        if opaque && let Some(metadata) = &self.opaque {
            return metadata.clone();
        }
        ReviewMetadata {
            sources: self.sources.clone(),
            provenance: self.provenance,
            accuracy: self.accuracy,
            completeness: self.completeness,
            naming: self.naming,
        }
    }
}
