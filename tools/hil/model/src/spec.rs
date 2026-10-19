//! A scenario's typed specification and its instances' identity.

use core::fmt::{self, Debug};

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    ContractName, Estimate, InstanceName, PurposeKind, Resources, ScenarioId, ScenarioInstance,
    Sha256, StandProfile, TargetCapabilities, Verdict, WaitError,
    digest::canonical_json,
    finite::{self, NonFinite},
};

/// The contract a scenario exercises. The contract's bound ceilings are its
/// port conformance crate's typed `CEILINGS` constant, which the scenario's
/// waits and verdict use directly.
pub trait Subject {
    const CONTRACT: ContractName;
}

/// Everything about a scenario but its execution: identity, purpose,
/// typed parameters, criteria, trace and findings, what it needs and how
/// long it takes, and its verdict. Execution is a separate trait in the
/// executor, so this one stays a format a reader links to recompute
/// verdicts.
pub trait Spec {
    const ID: ScenarioId;
    /// [`Conformance`](crate::Conformance), [`Interop`](crate::Interop),
    /// [`Measurement`](crate::Measurement) or an
    /// [`Investigation`](crate::Investigation); the executor admits the
    /// scenario by it.
    type Purpose: PurposeKind;
    type Subject: Subject;
    /// One instance's parameters; instances replace copied scenarios.
    type Params: Clone + Debug + Serialize + DeserializeOwned;
    /// Acceptance thresholds. A bound the
    /// backend reports is not a criterion: it travels in the trace.
    type Criteria: Clone + Debug + Serialize + DeserializeOwned;
    /// What a run records; the verdict's only input besides the criteria.
    type Trace: Debug + Serialize + DeserializeOwned;
    /// What a failed verdict names.
    type Finding: Clone + Debug + Eq + Serialize + DeserializeOwned;

    /// Boards by role, fixtures and board capabilities.
    fn resources(params: &Self::Params) -> Resources;

    /// The chip, composition and agent capabilities the image must serve.
    fn target(params: &Self::Params) -> TargetCapabilities;

    /// One instance's own work: its waits, sized by the contract's ceilings
    /// and the stand profile, and its repetitions.
    fn estimate(params: &Self::Params, profile: &StandProfile) -> Result<Estimate, WaitError>;

    /// The verdict: a pure function of the recorded trace and the criteria.
    fn verdict(trace: &Self::Trace, criteria: &Self::Criteria) -> Verdict<Self::Finding>;
}

/// One named, parameterised instance of a scenario.
pub struct Instance<S: Spec> {
    pub name: InstanceName,
    pub params: S::Params,
}

// By hand: a derive would require the scenario's marker type itself to be
// `Clone` and `Debug`.
impl<S: Spec> Clone for Instance<S> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            params: self.params.clone(),
        }
    }
}

impl<S: Spec> Debug for Instance<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instance")
            .field("name", &self.name)
            .field("params", &self.params)
            .finish()
    }
}

impl<S: Spec> Instance<S> {
    pub const fn new(name: InstanceName, params: S::Params) -> Self {
        Self { name, params }
    }

    pub fn id(&self) -> ScenarioInstance {
        ScenarioInstance {
            scenario: S::ID,
            instance: self.name.clone(),
        }
    }

    /// SHA-256 over the canonical JSON ([`canonical_json`], keys sorted
    /// at every level, negative zero written as zero) of the scenario, the
    /// instance, its parameters and the criteria, so reordering a struct's
    /// fields keeps the identity. The verdict code's identity is the
    /// runner's (the observer build the run records); together they pin what
    /// a recorded verdict was computed by.
    pub fn identity(&self, criteria: &S::Criteria) -> Result<Sha256, IdentityError> {
        // JSON writes NaN and infinities as `null`, which would give
        // different thresholds one identity.
        finite::check(&self.params).map_err(IdentityError::NonFinite)?;
        finite::check(criteria).map_err(IdentityError::NonFinite)?;
        let value = serde_json::json!({
            "scenario": S::ID,
            "instance": self.name,
            "params": serde_json::to_value(&self.params).map_err(IdentityError::Json)?,
            "criteria": serde_json::to_value(criteria).map_err(IdentityError::Json)?,
        });
        Ok(Sha256::of(&canonical_json(&value)))
    }
}

/// Why an instance has no identity: a defect of the scenario.
#[derive(Debug)]
pub enum IdentityError {
    /// The parameters or criteria hold a NaN or an infinite number.
    NonFinite(NonFinite),
    /// The parameters or criteria have no JSON form.
    Json(serde_json::Error),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite(error) => write!(f, "a non-finite number: {error}"),
            Self::Json(error) => write!(f, "no JSON form: {error}"),
        }
    }
}

impl std::error::Error for IdentityError {}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::{Composition, Conformance, name};

    struct Port;

    impl Subject for Port {
        const CONTRACT: ContractName = name!(ContractName, "toy::Port");
    }

    struct Toy;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct Params {
        b: u32,
        a: Vec<u32>,
    }

    /// The same parameters with their fields declared in the other order.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct Reordered {
        a: Vec<u32>,
        b: u32,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct Criteria {
        at_most: f64,
    }

    macro_rules! toy {
        ($name:ident, $params:ty) => {
            impl Spec for $name {
                const ID: ScenarioId = name!(ScenarioId, "toy");
                type Purpose = Conformance;
                type Subject = Port;
                type Params = $params;
                type Criteria = Criteria;
                type Trace = ();
                type Finding = ();

                fn resources(_: &$params) -> Resources {
                    Resources::default()
                }

                fn target(_: &$params) -> TargetCapabilities {
                    TargetCapabilities {
                        chip: name!(ChipId, "esp32s31"),
                        composition: Composition::Any,
                        capabilities: Default::default(),
                    }
                }

                fn estimate(_: &$params, profile: &StandProfile) -> Result<Estimate, WaitError> {
                    Estimate::from_waits(&[], &[], 0, profile)
                }

                fn verdict((): &(), _: &Criteria) -> Verdict<()> {
                    Verdict::Pass
                }
            }
        };
    }

    struct ToyReordered;
    toy!(Toy, Params);
    toy!(ToyReordered, Reordered);

    #[test]
    fn the_identity_does_not_depend_on_field_order() {
        let criteria = Criteria { at_most: 1.5 };
        let a = Instance::<Toy>::new(name!(InstanceName, "x"), Params { b: 2, a: vec![1] });
        let b =
            Instance::<ToyReordered>::new(name!(InstanceName, "x"), Reordered { a: vec![1], b: 2 });
        assert_eq!(
            a.identity(&criteria).unwrap(),
            b.identity(&criteria).unwrap()
        );
        let other = Instance::<Toy>::new(name!(InstanceName, "x"), Params { b: 3, a: vec![1] });
        assert_ne!(
            a.identity(&criteria).unwrap(),
            other.identity(&criteria).unwrap()
        );
    }

    #[test]
    fn negative_zero_criteria_share_zero_s_identity() {
        let instance = Instance::<Toy>::new(
            name!(InstanceName, "x"),
            Params {
                b: 2,
                a: Vec::new(),
            },
        );
        assert_eq!(
            instance.identity(&Criteria { at_most: -0.0 }).unwrap(),
            instance.identity(&Criteria { at_most: 0.0 }).unwrap()
        );
    }

    #[test]
    fn an_instance_clones_and_prints_although_its_marker_does_neither() {
        // `Toy` derives neither `Clone` nor `Debug`.
        let instance = Instance::<Toy>::new(name!(InstanceName, "x"), Params { b: 2, a: vec![1] });
        let copy = instance.clone();
        assert_eq!(copy.name, instance.name);
        assert_eq!(copy.params.a, instance.params.a);
        let printed = format!("{instance:?}");
        assert!(
            printed.starts_with("Instance { name: InstanceName(")
                && printed.contains("params: Params { b: 2"),
            "{printed}"
        );
    }

    #[test]
    fn non_finite_criteria_have_no_identity() {
        let instance = Instance::<Toy>::new(
            name!(InstanceName, "x"),
            Params {
                b: 2,
                a: Vec::new(),
            },
        );
        for at_most in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                instance.identity(&Criteria { at_most }),
                Err(IdentityError::NonFinite(_))
            ));
        }
    }
}
