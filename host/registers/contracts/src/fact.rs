use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::Error as _};

use crate::{
    ArtifactIdentity, FactAccuracy, FactCompleteness, FactProvenance, RevisionOccurrenceId,
    identity::{validate_sha256, validate_stable_id},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactValidationError(String);

impl FactValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for FactValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FactValidationError {}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FactClassification {
    pub provenance: FactProvenance,
    pub accuracy: FactAccuracy,
    pub completeness: FactCompleteness,
}

/// Bounds a fact to reusable ecosystem, chip and exact artifact identities.
///
/// Empty dimensions are wildcards. Construction and deserialization sort all
/// dimensions, reject duplicates, and reject non-canonical stable IDs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Applicability {
    pub ecosystems: Vec<String>,
    pub chips: Vec<String>,
    pub chip_revisions: Vec<String>,
    pub artifact_lineages: Vec<String>,
    pub artifacts: Vec<ArtifactIdentity>,
}

impl Applicability {
    pub fn new(
        ecosystems: Vec<String>,
        chips: Vec<String>,
        chip_revisions: Vec<String>,
        artifact_lineages: Vec<String>,
        artifacts: Vec<ArtifactIdentity>,
    ) -> Result<Self, FactValidationError> {
        Self {
            ecosystems,
            chips,
            chip_revisions,
            artifact_lineages,
            artifacts,
        }
        .normalized()
    }

    pub fn is_unbounded(&self) -> bool {
        self.ecosystems.is_empty()
            && self.chips.is_empty()
            && self.chip_revisions.is_empty()
            && self.artifact_lineages.is_empty()
            && self.artifacts.is_empty()
    }

    pub fn validate(&self) -> Result<(), FactValidationError> {
        let _ = self.clone().normalized()?;
        Ok(())
    }

    pub fn normalized(mut self) -> Result<Self, FactValidationError> {
        normalize_ids("ecosystems", &mut self.ecosystems)?;
        normalize_ids("chips", &mut self.chips)?;
        normalize_ids("chip revisions", &mut self.chip_revisions)?;
        normalize_ids("artifact lineages", &mut self.artifact_lineages)?;
        normalize_unique("artifacts", &mut self.artifacts)?;
        Ok(self)
    }

    pub fn intersection(&self, other: &Self) -> Option<Self> {
        Some(Self {
            ecosystems: intersect_dimension(&self.ecosystems, &other.ecosystems)?,
            chips: intersect_dimension(&self.chips, &other.chips)?,
            chip_revisions: intersect_dimension(&self.chip_revisions, &other.chip_revisions)?,
            artifact_lineages: intersect_dimension(
                &self.artifact_lineages,
                &other.artifact_lineages,
            )?,
            artifacts: intersect_dimension(&self.artifacts, &other.artifacts)?,
        })
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.intersection(other).is_some()
    }

    /// Fail-closed selection against one concrete project context.
    pub fn matches_context(
        &self,
        context: &ApplicabilityContext,
    ) -> Result<bool, FactValidationError> {
        self.validate()?;
        context.validate()?;
        let dimensions = [
            ("ecosystems", &self.ecosystems, &context.ecosystems),
            ("chips", &self.chips, &context.chips),
            (
                "chip revisions",
                &self.chip_revisions,
                &context.chip_revisions,
            ),
            (
                "artifact lineages",
                &self.artifact_lineages,
                &context.artifact_lineages,
            ),
        ];
        let mut missing = Vec::new();
        for (name, constraint, active) in dimensions {
            if constraint.is_empty() {
                continue;
            }
            if active.is_empty() {
                missing.push(name);
            } else if !constraint.iter().any(|item| active.contains(item)) {
                return Ok(false);
            }
        }
        if !self.artifacts.is_empty() {
            if context.artifacts.is_empty() {
                missing.push("artifacts");
            } else if !self
                .artifacts
                .iter()
                .any(|artifact| context.artifacts.contains(artifact))
            {
                return Ok(false);
            }
        }
        if !missing.is_empty() {
            return Err(FactValidationError::new(format!(
                "applicability constrains {}, but the active context omits that dimension",
                missing.join(", ")
            )));
        }
        Ok(true)
    }
}

impl Serialize for Applicability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ApplicabilityWire::from(self.clone().normalized().map_err(S::Error::custom)?)
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Applicability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        ApplicabilityWire::deserialize(deserializer)?
            .into_applicability()
            .map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplicabilityContext {
    pub ecosystems: Vec<String>,
    pub chips: Vec<String>,
    pub chip_revisions: Vec<String>,
    pub artifact_lineages: Vec<String>,
    pub artifacts: Vec<ArtifactIdentity>,
}

impl ApplicabilityContext {
    pub fn new(
        ecosystems: Vec<String>,
        chips: Vec<String>,
        chip_revisions: Vec<String>,
        artifact_lineages: Vec<String>,
        artifacts: Vec<ArtifactIdentity>,
    ) -> Result<Self, FactValidationError> {
        Self {
            ecosystems,
            chips,
            chip_revisions,
            artifact_lineages,
            artifacts,
        }
        .normalized()
    }

    pub fn validate(&self) -> Result<(), FactValidationError> {
        let _ = self.clone().normalized()?;
        Ok(())
    }

    pub fn normalized(mut self) -> Result<Self, FactValidationError> {
        normalize_ids("ecosystems", &mut self.ecosystems)?;
        normalize_ids("chips", &mut self.chips)?;
        normalize_ids("chip revisions", &mut self.chip_revisions)?;
        normalize_ids("artifact lineages", &mut self.artifact_lineages)?;
        normalize_unique("artifacts", &mut self.artifacts)?;
        Ok(self)
    }
}

impl Serialize for ApplicabilityContext {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ApplicabilityWire::from(self.clone().normalized().map_err(S::Error::custom)?)
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ApplicabilityContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let applicability = ApplicabilityWire::deserialize(deserializer)?
            .into_applicability()
            .map_err(de::Error::custom)?;
        Ok(Self {
            ecosystems: applicability.ecosystems,
            chips: applicability.chips,
            chip_revisions: applicability.chip_revisions,
            artifact_lineages: applicability.artifact_lineages,
            artifacts: applicability.artifacts,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ApplicabilityWire {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ecosystems: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chips: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chip_revisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_lineages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifacts: Vec<ArtifactIdentity>,
}

impl ApplicabilityWire {
    fn into_applicability(self) -> Result<Applicability, FactValidationError> {
        Applicability::new(
            self.ecosystems,
            self.chips,
            self.chip_revisions,
            self.artifact_lineages,
            self.artifacts,
        )
    }
}

impl From<Applicability> for ApplicabilityWire {
    fn from(value: Applicability) -> Self {
        Self {
            ecosystems: value.ecosystems,
            chips: value.chips,
            chip_revisions: value.chip_revisions,
            artifact_lineages: value.artifact_lineages,
            artifacts: value.artifacts,
        }
    }
}

impl From<ApplicabilityContext> for ApplicabilityWire {
    fn from(value: ApplicabilityContext) -> Self {
        Self {
            ecosystems: value.ecosystems,
            chips: value.chips,
            chip_revisions: value.chip_revisions,
            artifact_lineages: value.artifact_lineages,
            artifacts: value.artifacts,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EvidenceReference {
    pub source: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<RevisionOccurrenceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl EvidenceReference {
    pub fn new(
        source: impl Into<String>,
        locator: impl Into<String>,
        sha256: Option<String>,
        occurrence: Option<RevisionOccurrenceId>,
        note: Option<String>,
    ) -> Result<Self, FactValidationError> {
        let value = Self {
            source: source.into(),
            locator: locator.into(),
            sha256,
            occurrence,
            note,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), FactValidationError> {
        validate_stable_id("evidence source", &self.source)
            .map_err(|error| FactValidationError::new(error.to_string()))?;
        validate_text("evidence locator", &self.locator, 1024)?;
        if let Some(sha256) = &self.sha256 {
            validate_sha256(sha256).map_err(|error| FactValidationError::new(error.to_string()))?;
        }
        if let Some(note) = &self.note {
            validate_text("evidence note", note, 4096)?;
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for EvidenceReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct Wire {
            source: String,
            locator: String,
            #[serde(default)]
            sha256: Option<String>,
            #[serde(default)]
            occurrence: Option<RevisionOccurrenceId>,
            #[serde(default)]
            note: Option<String>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(
            wire.source,
            wire.locator,
            wire.sha256,
            wire.occurrence,
            wire.note,
        )
        .map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RecordMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<FactClassification>,
    #[serde(default, skip_serializing_if = "Applicability::is_unbounded")]
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
}

impl RecordMetadata {
    pub fn new(
        classification: Option<FactClassification>,
        applies_to: Applicability,
        evidence: Vec<EvidenceReference>,
    ) -> Result<Self, FactValidationError> {
        let value = Self {
            classification,
            applies_to: applies_to.normalized()?,
            evidence,
        };
        validate_evidence(&value.evidence)?;
        Ok(value)
    }

    pub fn effective(
        &self,
        inherited_classification: FactClassification,
        inherited_applicability: &Applicability,
    ) -> Result<EffectiveFactMetadata, FactValidationError> {
        let applies_to = inherited_applicability
            .intersection(&self.applies_to)
            .ok_or_else(|| {
                FactValidationError::new(
                    "record applicability is disjoint from inherited applicability",
                )
            })?;
        EffectiveFactMetadata::new(
            self.classification.unwrap_or(inherited_classification),
            applies_to,
            self.evidence.clone(),
        )
    }
}

impl<'de> Deserialize<'de> for RecordMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            classification: Option<FactClassification>,
            #[serde(default)]
            applies_to: Applicability,
            evidence: Vec<EvidenceReference>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.classification, wire.applies_to, wire.evidence).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EffectiveFactMetadata {
    pub classification: FactClassification,
    #[serde(default, skip_serializing_if = "Applicability::is_unbounded")]
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
}

impl EffectiveFactMetadata {
    pub fn new(
        classification: FactClassification,
        applies_to: Applicability,
        evidence: Vec<EvidenceReference>,
    ) -> Result<Self, FactValidationError> {
        let value = Self {
            classification,
            applies_to: applies_to.normalized()?,
            evidence,
        };
        validate_evidence(&value.evidence)?;
        Ok(value)
    }
}

impl<'de> Deserialize<'de> for EffectiveFactMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct Wire {
            classification: FactClassification,
            #[serde(default)]
            applies_to: Applicability,
            evidence: Vec<EvidenceReference>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.classification, wire.applies_to, wire.evidence).map_err(de::Error::custom)
    }
}

fn normalize_ids(field: &str, values: &mut Vec<String>) -> Result<(), FactValidationError> {
    for value in &*values {
        validate_stable_id(field, value)
            .map_err(|error| FactValidationError::new(error.to_string()))?;
    }
    normalize_unique(field, values)
}

fn normalize_unique<T: Ord>(field: &str, values: &mut [T]) -> Result<(), FactValidationError> {
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(FactValidationError::new(format!(
            "{field} contains duplicate values"
        )));
    }
    Ok(())
}

fn validate_evidence(evidence: &[EvidenceReference]) -> Result<(), FactValidationError> {
    if evidence.is_empty() {
        return Err(FactValidationError::new(
            "fact metadata requires at least one evidence reference",
        ));
    }
    let mut seen = BTreeSet::new();
    for reference in evidence {
        reference.validate()?;
        if !seen.insert(reference) {
            return Err(FactValidationError::new(
                "fact metadata contains duplicate evidence references",
            ));
        }
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, maximum: usize) -> Result<(), FactValidationError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(FactValidationError::new(format!(
            "{field} must be 1..{maximum} non-control characters without surrounding whitespace"
        )));
    }
    Ok(())
}

fn intersect_dimension<T: Clone + Ord>(left: &[T], right: &[T]) -> Option<Vec<T>> {
    if left.is_empty() {
        return Some(sorted_unique(right));
    }
    if right.is_empty() {
        return Some(sorted_unique(left));
    }
    let left = left.iter().collect::<BTreeSet<_>>();
    let right = right.iter().collect::<BTreeSet<_>>();
    let values = left
        .intersection(&right)
        .map(|value| (*value).clone())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn sorted_unique<T: Clone + Ord>(values: &[T]) -> Vec<T> {
    values
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityDomain;

    fn digest(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn artifact(source: &str, byte: char) -> ArtifactIdentity {
        ArtifactIdentity::new(source, digest(byte)).unwrap()
    }

    fn evidence() -> EvidenceReference {
        let artifact = artifact("esp-idf/lib/radio.a", 'a');
        EvidenceReference::new(
            "vendor-disassembly",
            "radio_rx+0x18",
            Some(digest('b')),
            Some(
                RevisionOccurrenceId::derive(EntityDomain::Function, &[artifact], "text+0x18")
                    .unwrap(),
            ),
            Some("reviewed call site".to_owned()),
        )
        .unwrap()
    }

    fn classification() -> FactClassification {
        FactClassification {
            provenance: FactProvenance::Reviewed,
            accuracy: FactAccuracy::Exact,
            completeness: FactCompleteness::Partial,
        }
    }

    #[test]
    fn applicability_normalizes_order_and_rejects_duplicates() {
        let value = Applicability::new(
            vec!["esp-idf".to_owned()],
            vec!["chip-beta".to_owned(), "chip-alpha".to_owned()],
            Vec::new(),
            Vec::new(),
            vec![artifact("sdk/b.a", 'b'), artifact("sdk/a.a", 'a')],
        )
        .unwrap();
        assert_eq!(value.chips, ["chip-alpha", "chip-beta"]);
        assert_eq!(value.artifacts[0].source(), "sdk/a.a");
        assert!(
            Applicability::new(
                vec!["esp-idf".to_owned(), "esp-idf".to_owned()],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn applicability_serde_is_strict_and_canonical() {
        let decoded: Applicability = serde_json::from_str(
            r#"{"chips":["chip-beta","chip-alpha"],"ecosystems":["vendor-sdk"]}"#,
        )
        .unwrap();
        assert_eq!(decoded.chips, ["chip-alpha", "chip-beta"]);
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            r#"{"ecosystems":["vendor-sdk"],"chips":["chip-alpha","chip-beta"]}"#
        );
        assert!(
            serde_json::from_str::<Applicability>(
                r#"{"chips":["chip-alpha"],"legacy-chip":"old-chip"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<Applicability>(r#"{"chips":["CHIP-ALPHA"]}"#).is_err());
    }

    #[test]
    fn applicability_intersection_and_context_are_fail_closed() {
        let fact = Applicability::new(
            vec!["esp-idf".to_owned()],
            vec!["chip-alpha".to_owned()],
            Vec::new(),
            Vec::new(),
            vec![artifact("sdk/radio.a", 'a')],
        )
        .unwrap();
        assert!(
            fact.matches_context(&ApplicabilityContext::default())
                .is_err()
        );
        let wrong = ApplicabilityContext::new(
            vec!["esp-idf".to_owned()],
            vec!["chip-beta".to_owned()],
            Vec::new(),
            Vec::new(),
            vec![artifact("sdk/radio.a", 'a')],
        )
        .unwrap();
        assert!(!fact.matches_context(&wrong).unwrap());
        let exact = ApplicabilityContext::new(
            vec!["esp-idf".to_owned()],
            vec!["chip-alpha".to_owned()],
            Vec::new(),
            Vec::new(),
            vec![artifact("sdk/radio.a", 'a')],
        )
        .unwrap();
        assert!(fact.matches_context(&exact).unwrap());
    }

    #[test]
    fn evidence_carries_optional_occurrence_and_rejects_bad_inputs() {
        let value = evidence();
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<EvidenceReference>(&json).unwrap(),
            value
        );
        assert!(EvidenceReference::new("Vendor Evidence", "site", None, None, None).is_err());
        assert!(EvidenceReference::new("vendor", " site", None, None, None).is_err());
        assert!(
            serde_json::from_str::<EvidenceReference>(
                r#"{"source":"vendor","locator":"site","legacy":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn metadata_inherits_classification_and_intersects_applicability() {
        let inherited = Applicability::new(
            vec!["esp-idf".to_owned()],
            vec!["chip-beta".to_owned(), "chip-alpha".to_owned()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let record = RecordMetadata::new(
            None,
            Applicability::new(
                Vec::new(),
                vec!["chip-alpha".to_owned()],
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
            vec![evidence()],
        )
        .unwrap();
        let effective = record.effective(classification(), &inherited).unwrap();
        assert_eq!(effective.classification, classification());
        assert_eq!(effective.applies_to.ecosystems, ["esp-idf"]);
        assert_eq!(effective.applies_to.chips, ["chip-alpha"]);
    }

    #[test]
    fn metadata_rejects_missing_or_duplicate_evidence_and_unknown_fields() {
        assert!(RecordMetadata::new(None, Applicability::default(), Vec::new()).is_err());
        assert!(
            RecordMetadata::new(None, Applicability::default(), vec![evidence(), evidence()])
                .is_err()
        );
        assert!(
            serde_json::from_str::<RecordMetadata>(
                r#"{"evidence":[],"note":"legacy record-owned field"}"#
            )
            .is_err()
        );
    }
}
