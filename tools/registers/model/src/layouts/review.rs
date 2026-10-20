//! Per-layout review templates, resolved before the existing review checks.

use std::collections::BTreeMap;

use oer_registers_contracts::{FactAccuracy, FactCompleteness, FactProvenance, NameOrigin};
use serde::Deserialize;

use crate::{Error, Result, ReviewAnnotation};

/// Independent metadata of the default for explicitly opaque entities.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ReviewMetadata {
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
}

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
    fn metadata(&self, opaque: bool) -> ReviewMetadata {
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

impl ReviewMetadata {
    fn for_entity(self, entity: String) -> ReviewAnnotation {
        ReviewAnnotation {
            entity,
            sources: self.sources,
            provenance: self.provenance,
            accuracy: self.accuracy,
            completeness: self.completeness,
            naming: self.naming,
        }
    }
}

/// Materialize exactly one applicable template when no explicit review exists.
/// Explicit records replace a whole template; no keys are merged. Unknown or
/// duplicate explicit entities stay in the result for the ordinary validator.
pub(super) fn resolve(
    defaults: Option<&LayoutReviewDefaults>,
    mut explicit: Vec<ReviewAnnotation>,
    entities: &BTreeMap<String, bool>,
) -> Result<Vec<ReviewAnnotation>> {
    for (entity, opaque) in entities {
        let default =
            defaults.map(|defaults| defaults.metadata(*opaque).for_entity(entity.clone()));
        let review = explicit.iter().find(|review| review.entity == *entity);
        if let Some(review) = review {
            if Some(review) == default.as_ref() {
                return Err(Error::message(format!(
                    "review entity {entity:?} repeats its applicable layout review default"
                )));
            }
        } else if let Some(default) = default {
            explicit.push(default);
        }
        if *opaque
            && explicit
                .iter()
                .find(|review| review.entity == *entity)
                .is_some_and(|review| review.naming != Some(NameOrigin::Opaque))
        {
            return Err(Error::message(format!(
                "explicitly opaque entity {entity:?} requires naming = \"opaque\""
            )));
        }
    }
    Ok(explicit)
}
