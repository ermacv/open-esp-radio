//! Source-only review scopes; SVD peripherals never contain this metadata.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use svd_rs::Peripheral;

use crate::{
    Error, RegisterModelFragment, Result, ReviewAnnotation, ReviewEntityKind,
    collect_review_entities,
    review_defaults::{self, ReviewMetadata},
};

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct FragmentSource {
    schema: u32,
    peripherals: Vec<Peripheral>,
    #[serde(default)]
    review: Vec<ReviewAnnotation>,
    #[serde(default)]
    peripheral_review: Vec<PeripheralReview>,
}

/// A scope names exactly one SVD peripheral, even in a multi-peripheral file.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct PeripheralReview {
    entity: String,
    #[serde(default)]
    opaque_entities: Vec<String>,
    #[serde(default)]
    peripheral: Option<ReviewMetadata>,
    #[serde(default)]
    register: Option<ReviewMetadata>,
    #[serde(default)]
    field: Option<ReviewMetadata>,
    #[serde(default)]
    opaque: Option<ReviewMetadata>,
}

impl TryFrom<FragmentSource> for RegisterModelFragment {
    type Error = Error;

    fn try_from(source: FragmentSource) -> Result<Self> {
        let review = resolve(
            &source.peripherals,
            source.review,
            &source.peripheral_review,
        )?;
        Ok(Self {
            schema: source.schema,
            peripherals: source.peripherals,
            review,
        })
    }
}

pub(crate) fn resolve(
    peripherals: &[Peripheral],
    mut explicit: Vec<ReviewAnnotation>,
    scopes: &[PeripheralReview],
) -> Result<Vec<ReviewAnnotation>> {
    let mut seen = BTreeSet::new();
    for scope in scopes {
        if !seen.insert(&scope.entity) {
            return Err(Error::message(format!(
                "duplicate peripheral review scope {:?}",
                scope.entity
            )));
        }
        let peripheral = peripherals
            .iter()
            .find(|p| p.name == scope.entity)
            .ok_or_else(|| {
                Error::message(format!(
                    "peripheral review scope {:?} does not exist in this fragment",
                    scope.entity
                ))
            })?;
        let mut kinds = BTreeMap::new();
        collect_review_entities(peripheral, &mut kinds, &mut BTreeMap::new());
        let mut opaque = BTreeSet::new();
        for entity in &scope.opaque_entities {
            if !opaque.insert(entity) {
                return Err(Error::message(format!(
                    "duplicate opaque entity {entity:?}"
                )));
            }
            if !kinds.get(entity).is_some_and(|kind| kind.is_named_bits()) {
                return Err(Error::message(format!(
                    "opaque entity {entity:?} is not a register or field of {:?}",
                    scope.entity
                )));
            }
        }
        // Interrupts and enumeration values keep their explicit records. They
        // do not introduce another level of review inheritance.
        let entities = kinds
            .iter()
            .filter(|(_, kind)| {
                matches!(
                    kind,
                    ReviewEntityKind::Peripheral
                        | ReviewEntityKind::Cluster
                        | ReviewEntityKind::Register
                        | ReviewEntityKind::Field
                )
            })
            .map(|(entity, _)| (entity.clone(), opaque.contains(entity)))
            .collect();
        explicit = review_defaults::resolve(
            explicit,
            &entities,
            |entity, is_opaque| {
                if is_opaque && let Some(metadata) = &scope.opaque {
                    return Some(metadata.clone());
                }
                match kinds[entity] {
                    ReviewEntityKind::Peripheral => scope.peripheral.as_ref(),
                    ReviewEntityKind::Cluster | ReviewEntityKind::Register => {
                        scope.register.as_ref()
                    }
                    ReviewEntityKind::Field => scope.field.as_ref(),
                    _ => None,
                }
                .cloned()
            },
            "peripheral",
        )?;
        for entity in entities.keys() {
            if !explicit.iter().any(|review| review.entity == *entity) {
                return Err(Error::message(format!(
                    "peripheral entity {entity:?} has no resolved review"
                )));
            }
        }
    }
    Ok(explicit)
}

#[cfg(test)]
mod tests;
