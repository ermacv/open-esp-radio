//! Sparse, architecture-neutral reviewed knowledge.
//!
//! Generated observations remain outside this crate. A [`ReviewPack`] records
//! only accepted human decisions and the evidence/applicability that bounds
//! them. Contracts own canonical semantic and revision-occurrence identities;
//! consumers own the vocabulary carried by `kind` and `value`; this crate owns
//! fail-closed composition.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use oer_register_contracts::{
    Applicability, ApplicabilityContext, EffectiveFactMetadata, FactClassification, RecordMetadata,
    RevisionOccurrenceId, SemanticEntityId,
};
use oer_register_contracts::{EntityDomain, FactProvenance};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read reviewed-knowledge pack {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse reviewed-knowledge pack {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::de::Error,
    },

    #[error("invalid reviewed-knowledge pack {pack:?}: {reason}")]
    Invalid { pack: String, reason: String },

    #[error(
        "reviewed assertion conflict for {subject}/{kind:?}: {left:?} and {right:?} have overlapping applicability"
    )]
    Conflict {
        subject: Box<SemanticEntityId>,
        kind: String,
        left: String,
        right: String,
    },

    #[error(
        "reviewed entity-binding conflict for {occurrence}: {left:?} and {right:?} have overlapping applicability"
    )]
    BindingConflict {
        occurrence: RevisionOccurrenceId,
        left: String,
        right: String,
    },

    #[error("cannot select reviewed knowledge for the active applicability context: {reason}")]
    Selection { reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Scalar/list values keep reviewed diffs small and deterministic. Structured
/// domain-specific data belongs in several independently identified facts.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssertionValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    Strings(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewedAssertion {
    pub id: String,
    pub subject: SemanticEntityId,
    /// Consumer-owned fact vocabulary, for example `register-identity` or
    /// `hardware-write-semantics`.
    pub kind: String,
    pub value: AssertionValue,
    #[serde(flatten)]
    pub metadata: RecordMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VendorBugStatus {
    Suspected,
    Reviewed,
    Resolved,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct VendorBug {
    pub id: String,
    pub function: SemanticEntityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register: Option<SemanticEntityId>,
    pub kind: String,
    pub status: VendorBugStatus,
    pub observed: String,
    pub expected: String,
    #[serde(flatten)]
    pub metadata: RecordMetadata,
}

/// A reviewed mapping from one blob-local observation to one stable entity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewedEntityBinding {
    pub id: String,
    pub occurrence: RevisionOccurrenceId,
    pub semantic: SemanticEntityId,
    #[serde(flatten)]
    pub metadata: RecordMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewPack {
    pub schema: u32,
    pub id: String,
    pub classification: FactClassification,
    #[serde(default, skip_serializing_if = "Applicability::is_unbounded")]
    pub applies_to: Applicability,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<ReviewedAssertion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vendor_bugs: Vec<VendorBug>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ReviewedEntityBinding>,
}

impl ReviewPack {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let pack = toml_edit::de::from_str(&input).map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })?;
        validate_pack(&pack)?;
        Ok(pack)
    }

    pub fn from_toml(input: &str) -> Result<Self> {
        let pack = toml_edit::de::from_str(input).map_err(|source| Error::Parse {
            path: PathBuf::from("<memory>"),
            source,
        })?;
        validate_pack(&pack)?;
        Ok(pack)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EffectiveAssertion {
    pub pack: String,
    pub id: String,
    pub subject: SemanticEntityId,
    pub kind: String,
    pub value: AssertionValue,
    #[serde(flatten)]
    pub metadata: EffectiveFactMetadata,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EffectiveVendorBug {
    pub pack: String,
    pub id: String,
    pub function: SemanticEntityId,
    pub register: Option<SemanticEntityId>,
    pub kind: String,
    pub status: VendorBugStatus,
    pub observed: String,
    pub expected: String,
    #[serde(flatten)]
    pub metadata: EffectiveFactMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EffectiveEntityBinding {
    pub pack: String,
    pub id: String,
    pub occurrence: RevisionOccurrenceId,
    pub semantic: SemanticEntityId,
    #[serde(flatten)]
    pub metadata: EffectiveFactMetadata,
    pub note: Option<String>,
}

impl EffectiveEntityBinding {
    /// Whether one evidence locator cryptographically derives the declared
    /// blob-local occurrence from the exact applicability artifact set.
    pub fn has_authentic_occurrence_evidence(&self) -> bool {
        self.occurrence.domain() == self.semantic.domain()
            && has_authentic_occurrence_evidence(&self.occurrence, &self.metadata)
    }
}

/// Deterministically merged, fail-closed reviewed knowledge.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReviewKnowledge {
    assertions: BTreeMap<String, EffectiveAssertion>,
    vendor_bugs: BTreeMap<String, EffectiveVendorBug>,
    bindings: BTreeMap<String, EffectiveEntityBinding>,
}

impl ReviewKnowledge {
    pub fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let packs = paths
            .iter()
            .map(|path| ReviewPack::load(path))
            .collect::<Result<Vec<_>>>()?;
        Self::merge(packs)
    }

    pub fn merge(packs: impl IntoIterator<Item = ReviewPack>) -> Result<Self> {
        let mut packs = packs.into_iter().collect::<Vec<_>>();
        packs.sort_by(|left, right| left.id.cmp(&right.id));
        let mut pack_ids = BTreeSet::new();
        let mut record_ids = BTreeSet::new();
        let mut assertions = BTreeMap::new();
        let mut vendor_bugs = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        let mut semantic_assertions =
            BTreeMap::<(SemanticEntityId, String), Vec<(String, Applicability)>>::new();
        let mut occurrence_bindings =
            BTreeMap::<RevisionOccurrenceId, Vec<(String, Applicability)>>::new();

        for pack in packs {
            validate_pack(&pack)?;
            if !pack_ids.insert(pack.id.clone()) {
                return invalid(&pack.id, format!("duplicate pack id {:?}", pack.id));
            }
            for assertion in pack.assertions {
                if !record_ids.insert(assertion.id.clone()) {
                    return invalid(&pack.id, format!("duplicate record id {:?}", assertion.id));
                }
                let metadata = effective_metadata(
                    &pack.id,
                    &assertion.id,
                    pack.classification,
                    &pack.applies_to,
                    &assertion.metadata,
                )?;
                let key = (assertion.subject.clone(), assertion.kind.clone());
                if let Some((previous, _)) = semantic_assertions
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .find(|(_, previous)| previous.overlaps(&metadata.applies_to))
                {
                    return Err(Error::Conflict {
                        subject: Box::new(assertion.subject),
                        kind: assertion.kind,
                        left: previous.clone(),
                        right: assertion.id,
                    });
                }
                semantic_assertions
                    .entry(key)
                    .or_default()
                    .push((assertion.id.clone(), metadata.applies_to.clone()));
                assertions.insert(
                    assertion.id.clone(),
                    EffectiveAssertion {
                        pack: pack.id.clone(),
                        id: assertion.id,
                        subject: assertion.subject,
                        kind: assertion.kind,
                        value: assertion.value,
                        metadata,
                        note: assertion.note,
                    },
                );
            }
            for bug in pack.vendor_bugs {
                if !record_ids.insert(bug.id.clone()) {
                    return invalid(&pack.id, format!("duplicate record id {:?}", bug.id));
                }
                let metadata = effective_metadata(
                    &pack.id,
                    &bug.id,
                    pack.classification,
                    &pack.applies_to,
                    &bug.metadata,
                )?;
                vendor_bugs.insert(
                    bug.id.clone(),
                    EffectiveVendorBug {
                        pack: pack.id.clone(),
                        id: bug.id,
                        function: bug.function,
                        register: bug.register,
                        kind: bug.kind,
                        status: bug.status,
                        observed: bug.observed,
                        expected: bug.expected,
                        metadata,
                    },
                );
            }
            for binding in pack.bindings {
                if !record_ids.insert(binding.id.clone()) {
                    return invalid(&pack.id, format!("duplicate record id {:?}", binding.id));
                }
                let metadata = effective_metadata(
                    &pack.id,
                    &binding.id,
                    pack.classification,
                    &pack.applies_to,
                    &binding.metadata,
                )?;
                if let Some((previous, _)) = occurrence_bindings
                    .get(&binding.occurrence)
                    .into_iter()
                    .flatten()
                    .find(|(_, previous)| previous.overlaps(&metadata.applies_to))
                {
                    return Err(Error::BindingConflict {
                        occurrence: binding.occurrence,
                        left: previous.clone(),
                        right: binding.id,
                    });
                }
                occurrence_bindings
                    .entry(binding.occurrence.clone())
                    .or_default()
                    .push((binding.id.clone(), metadata.applies_to.clone()));
                bindings.insert(
                    binding.id.clone(),
                    EffectiveEntityBinding {
                        pack: pack.id.clone(),
                        id: binding.id,
                        occurrence: binding.occurrence,
                        semantic: binding.semantic,
                        metadata,
                        note: binding.note,
                    },
                );
            }
        }
        Ok(Self {
            assertions,
            vendor_bugs,
            bindings,
        })
    }

    pub fn assertions(&self) -> &BTreeMap<String, EffectiveAssertion> {
        &self.assertions
    }

    pub fn vendor_bugs(&self) -> &BTreeMap<String, EffectiveVendorBug> {
        &self.vendor_bugs
    }

    pub fn bindings(&self) -> &BTreeMap<String, EffectiveEntityBinding> {
        &self.bindings
    }

    /// Artifact sources that can affect selection of the loaded reviewed
    /// records. Callers only need to authenticate revision inputs from this
    /// set; unrelated stage bindings remain ordinary preflight inputs.
    pub fn constrained_artifact_sources(&self) -> BTreeSet<String> {
        self.assertions
            .values()
            .map(|record| &record.metadata.applies_to)
            .chain(
                self.vendor_bugs
                    .values()
                    .map(|record| &record.metadata.applies_to),
            )
            .chain(
                self.bindings
                    .values()
                    .map(|record| &record.metadata.applies_to),
            )
            .flat_map(|applicability| applicability.artifacts.iter())
            .map(|artifact| artifact.source().to_owned())
            .collect()
    }

    /// Select only facts compatible with one explicit project composition.
    ///
    /// A constrained fact cannot be selected when the corresponding context
    /// dimension is absent. If a context is broad enough to select two facts
    /// for the same subject/kind, selection is ambiguous and fails closed.
    pub fn select_for(&self, context: &ApplicabilityContext) -> Result<Self> {
        validate_context(context)?;
        let mut assertions = BTreeMap::new();
        let mut semantic = BTreeMap::<(SemanticEntityId, String), String>::new();
        for assertion in self.assertions.values() {
            if !matches_context(&assertion.id, &assertion.metadata.applies_to, context)? {
                continue;
            }
            let key = (assertion.subject.clone(), assertion.kind.clone());
            if let Some(previous) = semantic.insert(key.clone(), assertion.id.clone()) {
                return Err(Error::Selection {
                    reason: format!(
                        "assertions {previous:?} and {:?} both match {}/{}, so the context is ambiguous",
                        assertion.id, key.0, key.1
                    ),
                });
            }
            assertions.insert(assertion.id.clone(), assertion.clone());
        }
        let mut vendor_bugs = BTreeMap::new();
        for bug in self.vendor_bugs.values() {
            if matches_context(&bug.id, &bug.metadata.applies_to, context)? {
                vendor_bugs.insert(bug.id.clone(), bug.clone());
            }
        }
        let mut bindings = BTreeMap::new();
        let mut occurrences = BTreeMap::<RevisionOccurrenceId, String>::new();
        for binding in self.bindings.values() {
            if !matches_context(&binding.id, &binding.metadata.applies_to, context)? {
                continue;
            }
            if let Some(previous) =
                occurrences.insert(binding.occurrence.clone(), binding.id.clone())
            {
                return Err(Error::Selection {
                    reason: format!(
                        "entity bindings {previous:?} and {:?} both match {}, so the context is ambiguous",
                        binding.id, binding.occurrence
                    ),
                });
            }
            bindings.insert(binding.id.clone(), binding.clone());
        }
        Ok(Self {
            assertions,
            vendor_bugs,
            bindings,
        })
    }

    pub fn semantic_fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("review knowledge is JSON serializable");
        let mut hash = Sha256::new();
        hash.update(b"blobray/review-knowledge/v2\0");
        hash.update(encoded);
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

fn validate_context(context: &ApplicabilityContext) -> Result<()> {
    context.validate().map_err(|error| Error::Selection {
        reason: format!("invalid active applicability context: {error}"),
    })
}

fn matches_context(
    record: &str,
    applicability: &Applicability,
    context: &ApplicabilityContext,
) -> Result<bool> {
    applicability
        .matches_context(context)
        .map_err(|error| Error::Selection {
            reason: format!("record {record:?} cannot be selected: {error}"),
        })
}

fn validate_pack(pack: &ReviewPack) -> Result<()> {
    if pack.schema != 2 {
        return invalid(
            &pack.id,
            format!("requires schema = 2, got {}", pack.schema),
        );
    }
    validate_stable_id(&pack.id).map_err(|reason| Error::Invalid {
        pack: pack.id.clone(),
        reason,
    })?;
    validate_classification(&pack.id, "pack", pack.classification)?;
    validate_common_applicability(&pack.id, "pack", &pack.applies_to)?;
    let mut ids = BTreeSet::new();
    for assertion in &pack.assertions {
        validate_record_id(&pack.id, &assertion.id, &mut ids)?;
        validate_token(&pack.id, &assertion.id, "kind", &assertion.kind)?;
        if let AssertionValue::Strings(values) = &assertion.value
            && (values.is_empty() || !all_unique_nonempty(values))
        {
            return invalid(
                &pack.id,
                format!(
                    "assertion {:?} has an empty or duplicate string-list value",
                    assertion.id
                ),
            );
        }
        effective_metadata(
            &pack.id,
            &assertion.id,
            pack.classification,
            &pack.applies_to,
            &assertion.metadata,
        )?;
    }
    for bug in &pack.vendor_bugs {
        validate_record_id(&pack.id, &bug.id, &mut ids)?;
        if bug.function.domain() != EntityDomain::Function {
            return invalid(
                &pack.id,
                format!(
                    "vendor bug {:?} function must be a function entity, got {}",
                    bug.id,
                    bug.function.domain()
                ),
            );
        }
        if let Some(register) = &bug.register
            && register.domain() != EntityDomain::Register
        {
            return invalid(
                &pack.id,
                format!(
                    "vendor bug {:?} register must be a register entity, got {}",
                    bug.id,
                    register.domain()
                ),
            );
        }
        validate_token(&pack.id, &bug.id, "kind", &bug.kind)?;
        validate_text(&pack.id, &bug.id, "observed", &bug.observed)?;
        validate_text(&pack.id, &bug.id, "expected", &bug.expected)?;
        effective_metadata(
            &pack.id,
            &bug.id,
            pack.classification,
            &pack.applies_to,
            &bug.metadata,
        )?;
    }
    for binding in &pack.bindings {
        validate_record_id(&pack.id, &binding.id, &mut ids)?;
        if binding.occurrence.domain() != binding.semantic.domain() {
            return invalid(
                &pack.id,
                format!(
                    "entity binding {:?} maps {} occurrence to {} semantic entity",
                    binding.id,
                    binding.occurrence.domain(),
                    binding.semantic.domain()
                ),
            );
        }
        let metadata = effective_metadata(
            &pack.id,
            &binding.id,
            pack.classification,
            &pack.applies_to,
            &binding.metadata,
        )?;
        if metadata.applies_to.artifacts.len() != 1 {
            return invalid(
                &pack.id,
                format!(
                    "entity binding {:?} requires exactly one artifact identity until artifact-set occurrences are supported",
                    binding.id
                ),
            );
        }
        if !has_authentic_occurrence_evidence(&binding.occurrence, &metadata) {
            return invalid(
                &pack.id,
                format!(
                    "entity binding {:?} evidence must derive its occurrence {} from the exact artifact identity and locator",
                    binding.id, binding.occurrence
                ),
            );
        }
    }
    Ok(())
}

fn has_authentic_occurrence_evidence(
    occurrence: &RevisionOccurrenceId,
    metadata: &EffectiveFactMetadata,
) -> bool {
    metadata.applies_to.artifacts.len() == 1
        && metadata.evidence.iter().any(|evidence| {
            evidence.occurrence.as_ref() == Some(occurrence)
                && RevisionOccurrenceId::derive(
                    occurrence.domain(),
                    &metadata.applies_to.artifacts,
                    &evidence.locator,
                )
                .is_ok_and(|derived| derived == *occurrence)
        })
}

fn effective_metadata(
    pack: &str,
    record: &str,
    pack_classification: FactClassification,
    pack_applicability: &Applicability,
    metadata: &RecordMetadata,
) -> Result<EffectiveFactMetadata> {
    let metadata = metadata
        .effective(pack_classification, pack_applicability)
        .map_err(|error| Error::Invalid {
            pack: pack.to_owned(),
            reason: format!("record {record:?} has invalid metadata: {error}"),
        })?;
    validate_classification(pack, record, metadata.classification)?;
    Ok(metadata)
}

fn validate_record_id(pack: &str, id: &str, ids: &mut BTreeSet<String>) -> Result<()> {
    validate_stable_id(id).map_err(|reason| Error::Invalid {
        pack: pack.to_owned(),
        reason: format!("invalid record id {id:?}: {reason}"),
    })?;
    if !ids.insert(id.to_owned()) {
        return invalid(pack, format!("duplicate record id {id:?}"));
    }
    Ok(())
}

fn validate_stable_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/' | b'@' | b'+')
        })
        || !value.bytes().any(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("must be 1..160 lower-case ASCII identity characters".to_owned());
    }
    Ok(())
}

fn validate_token(pack: &str, record: &str, field: &str, value: &str) -> Result<()> {
    validate_stable_id(value).map_err(|reason| Error::Invalid {
        pack: pack.to_owned(),
        reason: format!("record {record:?} {field} {value:?}: {reason}"),
    })
}

fn validate_text(pack: &str, record: &str, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return invalid(
            pack,
            format!("record {record:?} has invalid empty/control {field}"),
        );
    }
    Ok(())
}

fn validate_classification(
    pack: &str,
    record: &str,
    classification: FactClassification,
) -> Result<()> {
    if classification.provenance == FactProvenance::Hint {
        return invalid(
            pack,
            format!("record {record:?} is a hint; hints cannot be accepted reviewed knowledge"),
        );
    }
    Ok(())
}

fn validate_common_applicability(
    pack: &str,
    record: &str,
    applicability: &Applicability,
) -> Result<()> {
    applicability.validate().map_err(|error| Error::Invalid {
        pack: pack.to_owned(),
        reason: format!("record {record:?} has invalid applicability: {error}"),
    })
}

fn all_unique_nonempty(values: &[String]) -> bool {
    let mut seen = BTreeSet::new();
    values
        .iter()
        .all(|value| !value.trim().is_empty() && seen.insert(value))
}

fn invalid<T>(pack: &str, reason: impl Into<String>) -> Result<T> {
    Err(Error::Invalid {
        pack: pack.to_owned(),
        reason: reason.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_register_contracts::{ArtifactIdentity, EntityDomain, RevisionOccurrenceId};

    const BASE: &str = r#"
schema = 2
id = "chip-a-radio"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[applies-to]
chips = ["chip-a"]
chip-revisions = ["rev0"]

[[assertions]]
id = "ieee802154.event-status.identity"
subject = "register:chip-a/cpu/0x20103128/32"
kind = "register-identity"
value = "IEEE802154_MAC.EVENT_STATUS"

[[assertions.evidence]]
source = "esp-idf-ieee802154-common-ll"
locator = "ieee802154_ll_get_events"

[[assertions]]
id = "ieee802154.event-status.access"
subject = "register:chip-a/cpu/0x20103128/32"
kind = "hardware-write-semantics"
value = "unknown"
note = "Masked self-write observed; W1C is not proven."

[[assertions.evidence]]
source = "esp-idf-ieee802154-common-ll"
locator = "ieee802154_ll_clear_events"

[[vendor-bugs]]
id = "vendor.ieee802154.event-clear-rmw"
function = "function:esp-idf/ieee802154-ll-clear-events"
register = "register:chip-a/cpu/0x20103128/32"
kind = "suspect-rmw-on-status"
status = "suspected"
observed = "The accessor performs a masked self-write."
expected = "Confirm the hardware clear contract before using the accessor."

[[vendor-bugs.evidence]]
source = "esp-idf-ieee802154-common-ll"
locator = "ieee802154_ll_clear_events"
"#;

    #[test]
    fn sparse_pack_round_trips_and_has_stable_fingerprint() {
        let pack = ReviewPack::from_toml(BASE).unwrap();
        let first = ReviewKnowledge::merge([pack.clone()]).unwrap();
        let second = ReviewKnowledge::merge([pack]).unwrap();
        assert_eq!(first.assertions().len(), 2);
        assert_eq!(first.vendor_bugs().len(), 1);
        assert_eq!(first.semantic_fingerprint(), second.semantic_fingerprint());
    }

    #[test]
    fn empty_sparse_pack_is_a_valid_review_destination() {
        let pack = ReviewPack::from_toml(
            r#"
schema = 2
id = "project-facts"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[applies-to]
chips = ["chip-alpha"]
"#,
        )
        .unwrap();
        let knowledge = ReviewKnowledge::merge([pack]).unwrap();
        assert!(knowledge.assertions().is_empty());
        assert!(knowledge.vendor_bugs().is_empty());
        assert!(knowledge.bindings().is_empty());
    }

    #[test]
    fn applicability_is_intersection_not_override() {
        let pack = ReviewPack::from_toml(BASE).unwrap();
        let effective = ReviewKnowledge::merge([pack]).unwrap();
        assert_eq!(
            effective.assertions()["ieee802154.event-status.identity"]
                .metadata
                .applies_to
                .chips,
            ["chip-a"]
        );
        assert_eq!(
            effective.assertions()["ieee802154.event-status.identity"]
                .metadata
                .applies_to
                .chip_revisions,
            ["rev0"]
        );
    }

    #[test]
    fn consumer_owned_identity_keeps_provenance_and_applicability() {
        let pack = ReviewPack::from_toml(
            r#"
schema = 2
id = "chip-a-register-identity"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[applies-to]
chips = ["chip-a"]

[[assertions]]
id = "ieee802154.new-status.identity"
subject = "register:chip-a/cpu/0x2010312c/32"
kind = "register-identity"
value = "IEEE802154_MAC.NEW_STATUS"
[assertions.applies-to]
chip-revisions = ["rev0"]
[[assertions.evidence]]
source = "esp-idf-ieee802154-reg"
locator = "status register offset"
"#,
        )
        .unwrap();

        let effective = ReviewKnowledge::merge([pack]).unwrap();
        let identity = &effective.assertions()["ieee802154.new-status.identity"];
        assert_eq!(identity.kind, "register-identity");
        assert_eq!(
            identity.value,
            AssertionValue::String("IEEE802154_MAC.NEW_STATUS".to_owned())
        );
        assert_eq!(identity.metadata.applies_to.chips, ["chip-a"]);
        assert_eq!(identity.metadata.applies_to.chip_revisions, ["rev0"]);
        assert_eq!(
            identity.metadata.classification.provenance,
            FactProvenance::Reviewed
        );
        assert_eq!(
            identity.metadata.evidence[0].source,
            "esp-idf-ieee802154-reg"
        );
    }

    #[test]
    fn overlapping_semantic_assertions_fail_closed() {
        let first = ReviewPack::from_toml(BASE).unwrap();
        let second = ReviewPack::from_toml(
            &BASE
                .replace("id = \"chip-a-radio\"", "id = \"chip-a-radio-conflict\"")
                .replace(
                    "id = \"ieee802154.event-status.identity\"",
                    "id = \"ieee802154.event-status.identity-conflict\"",
                )
                .replace(
                    "id = \"ieee802154.event-status.access\"",
                    "id = \"ieee802154.event-status.access-conflict\"",
                )
                .replace(
                    "id = \"vendor.ieee802154.event-clear-rmw\"",
                    "id = \"vendor.ieee802154.event-clear-rmw-conflict\"",
                ),
        )
        .unwrap();
        let error = ReviewKnowledge::merge([first, second]).unwrap_err();
        assert!(matches!(error, Error::Conflict { .. }));
    }

    #[test]
    fn disjoint_chip_revisions_may_carry_different_values() {
        let make = |id: &str, revision: &str, value: &str| {
            ReviewPack::from_toml(&format!(
                r#"
schema = 2
id = "{id}"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[applies-to]
chip-revisions = ["{revision}"]
[[assertions]]
id = "{id}.fact"
subject = "register:chip-a/cpu/0x1000/32"
kind = "register-identity"
value = "{value}"
[[assertions.evidence]]
source = "manual"
locator = "review"
"#
            ))
            .unwrap()
        };
        let merged = ReviewKnowledge::merge([
            make("rev-a", "a", "RADIO.CONTROL_A"),
            make("rev-b", "b", "RADIO.CONTROL_B"),
        ])
        .unwrap();
        assert_eq!(merged.assertions().len(), 2);

        let rev_a = merged
            .select_for(&ApplicabilityContext {
                chip_revisions: vec!["a".to_owned()],
                ..ApplicabilityContext::default()
            })
            .unwrap();
        assert_eq!(rev_a.assertions().len(), 1);
        assert_eq!(
            rev_a.assertions()["rev-a.fact"].value,
            AssertionValue::String("RADIO.CONTROL_A".to_owned())
        );

        let ambiguous = merged
            .select_for(&ApplicabilityContext {
                chip_revisions: vec!["a".to_owned(), "b".to_owned()],
                ..ApplicabilityContext::default()
            })
            .unwrap_err();
        assert!(ambiguous.to_string().contains("context is ambiguous"));

        let missing = merged
            .select_for(&ApplicabilityContext::default())
            .unwrap_err();
        assert!(missing.to_string().contains("chip revisions"));
    }

    #[test]
    fn exact_artifact_applicability_is_an_allowed_identity_set() {
        let digest_a = "a".repeat(64);
        let digest_b = "b".repeat(64);
        let applicability = Applicability {
            artifacts: vec![
                ArtifactIdentity::new("blob", digest_a.clone()).unwrap(),
                ArtifactIdentity::new("blob", digest_b).unwrap(),
            ],
            ..Applicability::default()
        };
        let selected = ApplicabilityContext {
            artifacts: vec![ArtifactIdentity::new("blob", digest_a).unwrap()],
            ..ApplicabilityContext::default()
        };

        assert!(matches_context("artifact.fact", &applicability, &selected).unwrap());
    }

    #[test]
    fn hints_and_unbounded_vendor_artifact_hashes_are_rejected() {
        let hint = BASE.replace("provenance = \"reviewed\"", "provenance = \"hint\"");
        assert!(
            ReviewPack::from_toml(&hint)
                .unwrap_err()
                .to_string()
                .contains("hint")
        );

        let invalid_hash = BASE.replace(
            "chips = [\"chip-a\"]",
            "chips = [\"chip-a\"]\nartifacts = [{ source = \"blob\", sha256 = \"bad\" }]",
        );
        assert!(
            ReviewPack::from_toml(&invalid_hash)
                .unwrap_err()
                .to_string()
                .contains("SHA-256")
        );
    }

    #[test]
    fn schema_one_and_untyped_subjects_are_rejected() {
        let legacy_schema = BASE.replacen("schema = 2", "schema = 1", 1);
        assert!(
            ReviewPack::from_toml(&legacy_schema)
                .unwrap_err()
                .to_string()
                .contains("requires schema = 2")
        );

        let legacy_subject = BASE.replace(
            "register:chip-a/cpu/0x20103128/32",
            "mmio:cpu:0x20103128/32",
        );
        assert!(ReviewPack::from_toml(&legacy_subject).is_err());
    }

    #[test]
    fn reviewed_binding_requires_matching_domains_artifacts_and_occurrence_evidence() {
        let artifact = ArtifactIdentity::new("esp-idf/lib/radio.a", "a".repeat(64)).unwrap();
        let occurrence = RevisionOccurrenceId::derive(
            EntityDomain::Function,
            std::slice::from_ref(&artifact),
            "radio.o:text+0x18",
        )
        .unwrap();
        let binding = format!(
            r#"
schema = 2
id = "radio-bindings"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[[bindings]]
id = "wifi.rx.binding"
occurrence = "{occurrence}"
semantic = "function:esp-idf/wifi/rx"
[bindings.applies-to]
artifacts = [{{ source = "esp-idf/lib/radio.a", sha256 = "{}" }}]
[[bindings.evidence]]
source = "manual-review"
locator = "radio.o:text+0x18"
occurrence = "{occurrence}"
"#,
            artifact.sha256()
        );

        let knowledge = ReviewKnowledge::merge([ReviewPack::from_toml(&binding).unwrap()]).unwrap();
        assert_eq!(knowledge.bindings().len(), 1);
        let effective = &knowledge.bindings()["wifi.rx.binding"];
        assert_eq!(effective.occurrence, occurrence);
        assert_eq!(
            effective.semantic,
            SemanticEntityId::function("esp-idf/wifi/rx").unwrap()
        );
        assert_eq!(
            effective.metadata.applies_to.artifacts.as_slice(),
            std::slice::from_ref(&artifact)
        );

        let same_occurrence = binding
            .replace("id = \"radio-bindings\"", "id = \"radio-bindings-copy\"")
            .replace("id = \"wifi.rx.binding\"", "id = \"wifi.rx.binding-copy\"");
        assert!(matches!(
            ReviewKnowledge::merge([
                ReviewPack::from_toml(&binding).unwrap(),
                ReviewPack::from_toml(&same_occurrence).unwrap()
            ]),
            Err(Error::BindingConflict { .. })
        ));

        let second_occurrence = RevisionOccurrenceId::derive(
            EntityDomain::Function,
            std::slice::from_ref(&artifact),
            "radio.o:text+0x30",
        )
        .unwrap();
        let same_semantic = same_occurrence
            .replace(&occurrence.to_string(), &second_occurrence.to_string())
            .replace("radio.o:text+0x18", "radio.o:text+0x30");
        let one_to_many = ReviewKnowledge::merge([
            ReviewPack::from_toml(&binding).unwrap(),
            ReviewPack::from_toml(&same_semantic).unwrap(),
        ])
        .unwrap();
        assert_eq!(one_to_many.bindings().len(), 2);
        assert!(
            one_to_many
                .bindings()
                .values()
                .all(|binding| binding.semantic
                    == SemanticEntityId::function("esp-idf/wifi/rx").unwrap())
        );
        let selected = one_to_many
            .select_for(&ApplicabilityContext {
                artifacts: vec![artifact.clone()],
                ..ApplicabilityContext::default()
            })
            .unwrap();
        assert_eq!(selected.bindings().len(), 2);

        let wrong_domain = binding.replace(
            "semantic = \"function:esp-idf/wifi/rx\"",
            "semantic = \"interface:esp-idf/wifi-osi\"",
        );
        assert!(
            ReviewPack::from_toml(&wrong_domain)
                .unwrap_err()
                .to_string()
                .contains("maps function occurrence to interface semantic entity")
        );

        let missing_occurrence_evidence = binding.replace(
            &format!("locator = \"radio.o:text+0x18\"\noccurrence = \"{occurrence}\"\n"),
            "locator = \"radio.o:text+0x18\"\n",
        );
        assert!(
            ReviewPack::from_toml(&missing_occurrence_evidence)
                .unwrap_err()
                .to_string()
                .contains("evidence must derive its occurrence")
        );

        let forged_locator = binding.replace("radio.o:text+0x18", "radio.o:text+0x19");
        assert!(
            ReviewPack::from_toml(&forged_locator)
                .unwrap_err()
                .to_string()
                .contains("evidence must derive its occurrence")
        );

        let forged_occurrence = binding.replace(&occurrence.sha256()[..8], "deadbeef");
        assert!(
            ReviewPack::from_toml(&forged_occurrence)
                .unwrap_err()
                .to_string()
                .contains("evidence must derive its occurrence")
        );

        let unbounded = binding.replace(
            &format!(
                "[bindings.applies-to]\nartifacts = [{{ source = \"esp-idf/lib/radio.a\", sha256 = \"{}\" }}]\n",
                artifact.sha256()
            ),
            "",
        );
        assert!(
            ReviewPack::from_toml(&unbounded)
                .unwrap_err()
                .to_string()
                .contains("requires exactly one artifact identity")
        );

        let multiple_artifacts = binding.replace(
            &format!(
                "artifacts = [{{ source = \"esp-idf/lib/radio.a\", sha256 = \"{}\" }}]",
                artifact.sha256()
            ),
            &format!(
                "artifacts = [{{ source = \"esp-idf/lib/radio.a\", sha256 = \"{}\" }}, {{ source = \"esp-idf/lib/companion.a\", sha256 = \"{}\" }}]",
                artifact.sha256(),
                "b".repeat(64)
            ),
        );
        assert!(
            ReviewPack::from_toml(&multiple_artifacts)
                .unwrap_err()
                .to_string()
                .contains("requires exactly one artifact identity")
        );
    }

    #[test]
    fn vendor_bug_entities_are_domain_typed() {
        let wrong_function = BASE.replace(
            "function:esp-idf/ieee802154-ll-clear-events",
            "interface:esp-idf/ieee802154-ll",
        );
        assert!(
            ReviewPack::from_toml(&wrong_function)
                .unwrap_err()
                .to_string()
                .contains("function must be a function entity")
        );

        let wrong_register = BASE.replace(
            "register = \"register:chip-a/cpu/0x20103128/32\"",
            "register = \"register-field:chip-a/cpu/0x20103128/32/0/1\"",
        );
        assert!(
            ReviewPack::from_toml(&wrong_register)
                .unwrap_err()
                .to_string()
                .contains("register must be a register entity")
        );
    }
}
