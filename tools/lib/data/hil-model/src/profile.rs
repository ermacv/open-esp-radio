//! The stand profile: spans measured on the stand, from which the
//! infrastructure waits (boot to Hello, flash, a fixture's readiness) and
//! the plan's image costs derive.

use core::fmt;
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

use crate::{
    Factor, Provisional, Resolved, SpanName, WaitError,
    repetitions::{self, Confidence, Probability},
};

/// The quantile every span reports.
pub const QUANTILE: Probability = match Probability::new(0.999) {
    Some(quantile) => quantile,
    None => panic!("0.999 is a probability"),
};
/// The confidence that the reported value is at least that quantile.
pub const CONFIDENCE: Confidence = match Confidence::new(0.95) {
    Some(confidence) => confidence,
    None => panic!("0.95 is a confidence"),
};

/// A run's identifier in the run store: decimal milliseconds, a hyphen and
/// lowercase hexadecimal digits, such as `-000d170b`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RunId(String);

impl RunId {
    pub fn parse(text: &str) -> Result<Self, InvalidRunId> {
        let (millis, suffix) = text.split_once('-').ok_or(InvalidRunId)?;
        let digits = !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit());
        let hex = !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if digits && hex {
            Ok(Self(text.to_owned()))
        } else {
            Err(InvalidRunId)
        }
    }
}

/// A run ID that is not `<millis>-<hex>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidRunId;

impl fmt::Display for InvalidRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a run ID is `<millis>-<hex>`")
    }
}

impl std::error::Error for InvalidRunId {}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RunId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}

/// Whether a span's samples support its quantile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "support", deny_unknown_fields)]
pub enum Support {
    /// At least the samples [`QUANTILE`] at [`CONFIDENCE`] needs. A struct
    /// variant, so `deny_unknown_fields` reaches it too: an internally
    /// tagged unit variant would accept any extra field.
    Measured {},
    /// Too few samples so far; why the span is used anyway. Every plan that
    /// uses it carries the label.
    Provisional { reason: String },
}

/// One measured span.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MeasuredSpan {
    /// The largest of `samples` observations, which bounds the
    /// [`QUANTILE`] with [`CONFIDENCE`] once there are enough of them.
    pub maximum_micros: u64,
    pub samples: u64,
    pub margin: Factor,
    /// Why this margin: every bound records it ( Q5).
    pub margin_reason: String,
    pub support: Support,
    /// The runs the samples come from.
    pub evidence: Vec<RunId>,
}

/// A profile entry that breaks the rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    /// No sample, or a maximum of zero: nothing was measured.
    Empty(SpanName),
    NoMarginReason(SpanName),
    NoEvidence(SpanName),
    NoProvisionalReason(SpanName),
    /// Too few samples for `Measured`.
    Unsupported {
        span: SpanName,
        samples: u64,
        required: u64,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(span) => write!(f, "span `{span}` has no sample or a zero maximum"),
            Self::NoMarginReason(span) => {
                write!(f, "span `{span}` does not say why its margin was chosen")
            }
            Self::NoEvidence(span) => write!(f, "span `{span}` names no run"),
            Self::NoProvisionalReason(span) => {
                write!(f, "span `{span}` is provisional without a reason")
            }
            Self::Unsupported {
                span,
                samples,
                required,
            } => {
                write!(
                    f,
                    "span `{span}` has {samples} samples and claims support; {required} are needed"
                )
            }
        }
    }
}

/// Every violation of a profile, in span order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileErrors(pub Vec<ProfileError>);

impl fmt::Display for ProfileErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            error.fmt(f)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProfileErrors {}

/// The profile as written, before its rules are checked.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawStandProfile {
    pub spans: BTreeMap<SpanName, MeasuredSpan>,
}

/// The measured spans of one stand. It exists only after its rules hold
/// ([`TryFrom<RawStandProfile>`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "RawStandProfile", into = "RawStandProfile")]
pub struct StandProfile {
    spans: BTreeMap<SpanName, MeasuredSpan>,
}

impl From<StandProfile> for RawStandProfile {
    fn from(profile: StandProfile) -> Self {
        Self {
            spans: profile.spans,
        }
    }
}

impl TryFrom<RawStandProfile> for StandProfile {
    type Error = ProfileErrors;

    fn try_from(raw: RawStandProfile) -> Result<Self, Self::Error> {
        let required = repetitions::quantile_samples(QUANTILE, CONFIDENCE);
        let mut errors = Vec::new();
        for (name, span) in &raw.spans {
            if span.samples == 0 || span.maximum_micros == 0 {
                errors.push(ProfileError::Empty(name.clone()));
            }
            if span.margin_reason.trim().is_empty() {
                errors.push(ProfileError::NoMarginReason(name.clone()));
            }
            if span.evidence.is_empty() {
                errors.push(ProfileError::NoEvidence(name.clone()));
            }
            match &span.support {
                Support::Measured {} if span.samples < required => {
                    errors.push(ProfileError::Unsupported {
                        span: name.clone(),
                        samples: span.samples,
                        required,
                    });
                }
                Support::Provisional { reason } if reason.trim().is_empty() => {
                    errors.push(ProfileError::NoProvisionalReason(name.clone()));
                }
                _ => {}
            }
        }
        if errors.is_empty() {
            Ok(Self { spans: raw.spans })
        } else {
            Err(ProfileErrors(errors))
        }
    }
}

impl StandProfile {
    /// A stand nothing has been measured on.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            spans: BTreeMap::new(),
        }
    }

    /// A span's budget: its maximum times its margin, labelled when it is
    /// provisional.
    pub fn budget(&self, name: &SpanName) -> Result<Resolved, WaitError> {
        let span = self
            .spans
            .get(name)
            .ok_or_else(|| WaitError::NoSpan(name.clone()))?;
        let provisional = match &span.support {
            Support::Measured {} => None,
            Support::Provisional { reason } => Some(Provisional {
                span: name.clone(),
                reason: reason.clone(),
            }),
        };
        Ok(Resolved {
            micros: span.margin.apply(span.maximum_micros)?,
            provisional,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use core::num::NonZeroU32;

    use super::*;
    use crate::name;

    pub(crate) fn run_ref() -> RunId {
        RunId::parse("1788729094522-000d170b").unwrap()
    }

    pub(crate) fn span(maximum_micros: u64, support: Support) -> MeasuredSpan {
        MeasuredSpan {
            maximum_micros,
            samples: 201,
            margin: Factor::new(6, NonZeroU32::new(5).unwrap()).unwrap(),
            margin_reason: "flash time grows with the image".into(),
            support,
            evidence: vec![run_ref()],
        }
    }

    fn profile(spans: Vec<(SpanName, MeasuredSpan)>) -> Result<StandProfile, ProfileErrors> {
        StandProfile::try_from(RawStandProfile {
            spans: spans.into_iter().collect(),
        })
    }

    #[test]
    fn too_few_samples_cannot_claim_support() {
        let flash = name!(SpanName, "flash.application");
        assert_eq!(
            profile(vec![(
                flash.clone(),
                span(25_000_000, Support::Measured {})
            )]),
            Err(ProfileErrors(vec![ProfileError::Unsupported {
                span: flash.clone(),
                samples: 201,
                required: 2_995
            }]))
        );
        let provisional = Support::Provisional {
            reason: "first stand measurement".into(),
        };
        let profile = profile(vec![(flash.clone(), span(25_000_000, provisional))]).unwrap();
        let resolved = profile.budget(&flash).unwrap();
        assert_eq!(resolved.micros, 30_000_000);
        assert_eq!(
            resolved.provisional,
            Some(Provisional {
                span: flash,
                reason: "first stand measurement".into()
            })
        );
    }

    #[test]
    fn a_span_records_why_its_margin_was_chosen_and_its_runs() {
        let boot = name!(SpanName, "boot-to-hello");
        let mut entry = span(900_000, Support::Provisional { reason: " ".into() });
        entry.margin_reason = " ".into();
        entry.evidence.clear();
        assert_eq!(
            profile(vec![(boot.clone(), entry)]),
            Err(ProfileErrors(vec![
                ProfileError::NoMarginReason(boot.clone()),
                ProfileError::NoEvidence(boot.clone()),
                ProfileError::NoProvisionalReason(boot),
            ]))
        );
    }

    #[test]
    fn a_span_without_samples_or_maximum_is_refused() {
        let boot = name!(SpanName, "boot-to-hello");
        let reason = || Support::Provisional {
            reason: "first".into(),
        };
        let mut none = span(900_000, reason());
        none.samples = 0;
        assert_eq!(
            profile(vec![(boot.clone(), none)]),
            Err(ProfileErrors(vec![ProfileError::Empty(boot.clone())]))
        );
        assert_eq!(
            profile(vec![(boot.clone(), span(0, reason()))]),
            Err(ProfileErrors(vec![ProfileError::Empty(boot)]))
        );
    }

    #[test]
    fn a_profile_is_read_only_through_its_rules() {
        let text = r#"{"spans":{"boot-to-hello":{"maximum-micros":1,"samples":1,"margin":[1,1],
            "margin-reason":"r","support":{"support":"measured"},"evidence":[]}}}"#;
        assert!(serde_json::from_str::<StandProfile>(text).is_err());
    }

    /// A field no reader knows is refused at every level, so a run reference
    /// reaches the profile only through `evidence`, the field a reference
    /// check will read once a profile is tracked (owner decision
    /// on ).
    #[test]
    fn a_profile_refuses_fields_it_does_not_read() {
        let provisional = Support::Provisional {
            reason: "first".into(),
        };
        let valid = profile(vec![(
            name!(SpanName, "boot-to-hello"),
            span(900_000, provisional),
        )])
        .unwrap();
        let value = serde_json::to_value(&valid).unwrap();
        assert_eq!(
            serde_json::from_value::<StandProfile>(value.clone()).unwrap(),
            valid
        );
        // Both supports refuse an unknown field: `Provisional` here, and a
        // measured span below.
        let mut measured = value.clone();
        measured["spans"]["boot-to-hello"]["support"] = serde_json::json!({"support": "measured"});
        assert_eq!(
            serde_json::from_value::<RawStandProfile>(measured.clone())
                .unwrap()
                .spans
                .values()
                .next()
                .unwrap()
                .support,
            Support::Measured {}
        );
        measured["spans"]["boot-to-hello"]["support"]["cited-runs"] = serde_json::json!([]);
        let error = serde_json::from_value::<RawStandProfile>(measured).unwrap_err();
        assert!(
            error.to_string().contains("unknown field"),
            "measured: {error}"
        );
        for pointer in ["", "/spans/boot-to-hello", "/spans/boot-to-hello/support"] {
            let mut extended = value.clone();
            extended
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("cited-runs".into(), serde_json::json!([]));
            let error = serde_json::from_value::<StandProfile>(extended).unwrap_err();
            assert!(
                error.to_string().contains("unknown field"),
                "{pointer}: {error}"
            );
        }
    }

    #[test]
    fn only_store_run_ids_parse() {
        assert!(RunId::parse("1788729094522-000d170b").is_ok());
        for text in [
            "",
            "1788729094522",
            "-000d170b",
            "1788729094522-",
            "17887x-00",
            "1788729094522-000D170B",
            "a-b",
        ] {
            assert_eq!(RunId::parse(text), Err(InvalidRunId), "{text:?}");
        }
    }
}
