//! Shared whole-record review resolution for memory layouts and peripherals.

use std::collections::BTreeMap;

use oer_registers_contracts::{FactAccuracy, FactCompleteness, FactProvenance, NameOrigin};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, ReviewAnnotation};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewMetadata {
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub provenance: Option<FactProvenance>,
    #[serde(default)]
    pub accuracy: Option<FactAccuracy>,
    #[serde(default)]
    pub completeness: Option<FactCompleteness>,
    #[serde(default)]
    pub naming: Option<NameOrigin>,
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

/// Materialize one applicable template if there is no explicit record. Unknown
/// and duplicate explicit entities remain for the existing model validators.
pub(crate) fn resolve(
    mut explicit: Vec<ReviewAnnotation>,
    entities: &BTreeMap<String, bool>,
    default_for: impl Fn(&str, bool) -> Option<ReviewMetadata>,
    scope: &str,
) -> Result<Vec<ReviewAnnotation>> {
    for (entity, opaque) in entities {
        let default =
            default_for(entity, *opaque).map(|metadata| metadata.for_entity(entity.clone()));
        if let Some(review) = explicit.iter().find(|review| review.entity == *entity) {
            if Some(review) == default.as_ref() {
                return Err(Error::message(format!(
                    "review entity {entity:?} repeats its applicable {scope} review default"
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
