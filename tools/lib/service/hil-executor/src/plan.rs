//! A tier's plan: what the stand cannot run, the image segments, their
//! lease limits and the check against the tier's budget, all before
//! anything is built or leased.
//!
//! A segment is one image on one board lane. Every segment takes its own
//! lease, whose hard limit is the segment's estimate times a margin (
//! Q4). A segment closes before that limit would pass the stand's longest
//! lease, and a scenario that cannot fit in any lease is refused rather
//! than given a shorter margin. The image is built before its first
//! segment's lease, so builds count toward the tier's wall time only.
//! Segments are summed as if serial; parallel lanes come with the
//! lane-isolation result.
//!
//! Every instance's waits are sized by its contract's ceilings, which no
//! backend may exceed (, owner decisions  and ),
//! and the plan adds each instance's boot to Hello from the stand profile,
//! so no scenario can leave it out. The plan labels every provisional
//! stand span it used.

use core::fmt;

use oer_hil_model::{
    Estimate, Factor, Instance, Overflow, Provisional, Resources, ScenarioInstance, SpanName, Spec,
    StandOffer, StandProfile, TargetCapabilities, Tier, Unmet, WaitError, sum,
};

type EstimateFn = dyn Fn(&StandProfile) -> Result<Estimate, WaitError>;

/// One scenario instance in the plan.
pub struct Planned {
    pub id: ScenarioInstance,
    pub image: TargetCapabilities,
    pub resources: Resources,
    estimate: Box<EstimateFn>,
}

impl Planned {
    pub fn of<S: Spec>(instance: &Instance<S>) -> Self
    where
        S::Params: 'static,
    {
        let params = instance.params.clone();
        Self {
            id: instance.id(),
            image: S::target(&instance.params),
            resources: S::resources(&instance.params),
            estimate: Box::new(move |profile| S::estimate(&params, profile)),
        }
    }
}

/// What the stand costs beyond the scenarios' own work: the stand spans
/// that measure it.
#[derive(Clone, Debug)]
pub struct StandCosts {
    /// Building an image when no build of its recipe is cached.
    pub build: SpanName,
    /// Writing an image to a board, once per segment.
    pub flash: SpanName,
    /// A reset to the agent's Hello, once per scenario instance.
    pub boot: SpanName,
}

/// How a segment's lease limit derives from its estimate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeasePolicy {
    pub margin: Factor,
    /// The stand's longest lease.
    pub cap_micros: u64,
}

/// One image on one board lane, under one lease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub image: TargetCapabilities,
    pub scenarios: Vec<ScenarioInstance>,
    /// The build before this segment's lease: the image's first segment
    /// builds it, later segments reuse the build.
    pub build_micros: u64,
    /// What the lease holds: the flash and the scenarios with their boots.
    pub lease_estimate_micros: u64,
    /// The estimate times the margin, at most the stand's cap.
    pub lease_limit_micros: u64,
}

impl Segment {
    pub fn wall_micros(&self) -> Result<u64, Overflow> {
        sum([self.build_micros, self.lease_estimate_micros])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub tier: Tier,
    pub segments: Vec<Segment>,
    pub total_micros: u64,
    /// Every provisional stand span the plan used.
    pub provisional: Vec<Provisional>,
}

/// Why a tier cannot be planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    /// The stand lacks boards, board capabilities or fixtures.
    Refused(Vec<(ScenarioInstance, Vec<Unmet>)>),
    /// A wait has no length; `None` for the stand costs.
    Wait {
        scenario: Option<ScenarioInstance>,
        error: WaitError,
    },
    /// A time does not fit in 64-bit microseconds; `None` for the tier's
    /// total.
    Overflow { scenario: Option<ScenarioInstance> },
    /// The plan exceeds the tier's budget; the largest scenarios first.
    OverBudget {
        budget_micros: u64,
        total_micros: u64,
        largest: Vec<(ScenarioInstance, u64)>,
    },
    /// One scenario alone does not fit in the longest lease.
    Unsplittable {
        scenario: ScenarioInstance,
        lease_micros: u64,
        cap_micros: u64,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusals) => {
                f.write_str("the stand cannot run:")?;
                for (scenario, unmet) in refusals {
                    for reason in unmet {
                        write!(f, " {scenario} ({reason});")?;
                    }
                }
                Ok(())
            }
            Self::Wait {
                scenario: Some(scenario),
                error,
            } => write!(f, "{scenario}: {error}"),
            Self::Wait {
                scenario: None,
                error,
            } => write!(f, "stand costs: {error}"),
            Self::Overflow {
                scenario: Some(scenario),
            } => write!(f, "{scenario}: {Overflow}"),
            Self::Overflow { scenario: None } => write!(f, "the tier's total: {Overflow}"),
            Self::OverBudget {
                budget_micros,
                total_micros,
                largest,
            } => {
                write!(
                    f,
                    "the plan takes {total_micros} µs of a {budget_micros} µs budget; largest:"
                )?;
                for (scenario, micros) in largest {
                    write!(f, " {scenario} ({micros} µs)")?;
                }
                Ok(())
            }
            Self::Unsplittable {
                scenario,
                lease_micros,
                cap_micros,
            } => {
                write!(
                    f,
                    "{scenario} needs a {lease_micros} µs lease; the stand's longest is {cap_micros} µs"
                )
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// How many of the largest scenarios an over-budget refusal names.
const NAMED: usize = 3;

/// The plan's inputs besides the scenarios.
pub struct PlanInputs<'a> {
    pub profile: &'a StandProfile,
    pub offer: &'a StandOffer,
    pub costs: &'a StandCosts,
    pub lease: LeasePolicy,
}

/// Plans `scenarios` for `tier`.
pub fn plan(tier: Tier, scenarios: &[Planned], inputs: &PlanInputs<'_>) -> Result<Plan, PlanError> {
    // An empty tier uses no stand span.
    if scenarios.is_empty() {
        return Ok(Plan {
            tier,
            segments: Vec::new(),
            total_micros: 0,
            provisional: Vec::new(),
        });
    }
    let refusals: Vec<_> = scenarios
        .iter()
        .filter_map(|planned| {
            let unmet = planned.resources.unmet(inputs.offer);
            (!unmet.is_empty()).then(|| (planned.id.clone(), unmet))
        })
        .collect();
    if !refusals.is_empty() {
        return Err(PlanError::Refused(refusals));
    }

    let mut provisional = Vec::new();
    let mut cost = |span: &SpanName| -> Result<u64, PlanError> {
        let resolved = inputs
            .profile
            .budget(span)
            .map_err(|error| PlanError::Wait {
                scenario: None,
                error,
            })?;
        provisional.extend(resolved.provisional);
        Ok(resolved.micros)
    };
    let build_micros = cost(&inputs.costs.build)?;
    let flash_micros = cost(&inputs.costs.flash)?;
    let boot_micros = cost(&inputs.costs.boot)?;

    // Images in first-use order: a scenario joins the first image of its
    // chip whose composition it shares, or either side needs none.
    let mut images: Vec<(TargetCapabilities, Vec<&Planned>)> = Vec::new();
    for planned in scenarios {
        let joined = images.iter_mut().find_map(|(image, members)| {
            let union = image.union(&planned.image).ok()?;
            *image = union;
            members.push(planned);
            Some(())
        });
        if joined.is_none() {
            images.push((planned.image.clone(), vec![planned]));
        }
    }

    let mut segments = Vec::new();
    let mut sizes = Vec::new();
    for (image, members) in images {
        let mut builds = true;
        let mut current = Vec::new();
        let mut leased = flash_micros;
        for planned in members {
            let id = || Some(planned.id.clone());
            let overflow = |_| PlanError::Overflow { scenario: id() };
            let estimate = (planned.estimate)(inputs.profile).map_err(|error| PlanError::Wait {
                scenario: id(),
                error,
            })?;
            let own =
                sum([boot_micros, estimate.total_micros().map_err(overflow)?]).map_err(overflow)?;
            provisional.extend(estimate.provisional().iter().cloned());
            sizes.push((planned.id.clone(), own));
            let with = sum([leased, own]).map_err(overflow)?;
            if !current.is_empty()
                && inputs.lease.margin.apply(with).map_err(overflow)? > inputs.lease.cap_micros
            {
                let closed = segment(
                    &image,
                    core::mem::take(&mut current),
                    builds,
                    build_micros,
                    leased,
                    inputs.lease,
                )
                .map_err(overflow)?;
                segments.push(closed);
                builds = false;
                leased = flash_micros;
            }
            let leased_with = sum([leased, own]).map_err(overflow)?;
            let lease_micros = inputs.lease.margin.apply(leased_with).map_err(overflow)?;
            if lease_micros > inputs.lease.cap_micros {
                return Err(PlanError::Unsplittable {
                    scenario: planned.id.clone(),
                    lease_micros,
                    cap_micros: inputs.lease.cap_micros,
                });
            }
            current.push(planned.id.clone());
            leased = leased_with;
        }
        if !current.is_empty() {
            let last = segment(&image, current, builds, build_micros, leased, inputs.lease)
                .map_err(|_| PlanError::Overflow { scenario: None })?;
            segments.push(last);
        }
    }

    let total_micros = segments
        .iter()
        .try_fold(0_u64, |total, segment| sum([total, segment.wall_micros()?]))
        .map_err(|_| PlanError::Overflow { scenario: None })?;
    if let Some(budget_micros) = tier.budget_micros()
        && total_micros > budget_micros
    {
        sizes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        sizes.truncate(NAMED);
        return Err(PlanError::OverBudget {
            budget_micros,
            total_micros,
            largest: sizes,
        });
    }
    provisional.sort();
    provisional.dedup();
    Ok(Plan {
        tier,
        segments,
        total_micros,
        provisional,
    })
}

fn segment(
    image: &TargetCapabilities,
    scenarios: Vec<ScenarioInstance>,
    builds: bool,
    build_micros: u64,
    leased: u64,
    lease: LeasePolicy,
) -> Result<Segment, Overflow> {
    Ok(Segment {
        image: image.clone(),
        scenarios,
        build_micros: if builds { build_micros } else { 0 },
        lease_estimate_micros: leased,
        lease_limit_micros: lease.margin.apply(leased)?,
    })
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;
    use std::collections::BTreeSet;

    use oer_hil_model::{
        BoardCapability, BoardNeed, BoardOffer, BoardRole, ChipId, Composition, CompositionName,
        Conformance, ContractName, InstanceName, LpClockSource, MeasuredSpan, RawStandProfile,
        RunId, RunRef, ScenarioId, Sha256, Subject, Support, Verdict, Wait, name,
    };
    use serde::{Deserialize, Serialize};

    use super::*;

    const SECOND: u64 = 1_000_000;
    const STATION: &str = "oer-esp32s31-ieee80211-system";

    struct Port;

    impl Subject for Port {
        const CONTRACT: ContractName = name!(ContractName, "toy::Port");
    }

    /// The toy port's terminal ceiling, as its conformance crate's
    /// `CEILINGS` would hold it.
    #[allow(
        clippy::disallowed_methods,
        reason = "stands for a port's CEILINGS constant"
    )]
    const TERMINAL_CEILING: oer_time::Duration = oer_time::Duration::from_secs(2);

    /// A scenario whose one repetition waits for the measured span `span`
    /// and once for the port's terminal ceiling.
    struct Fixed;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct Params {
        span: SpanName,
        chip: ChipId,
        composition: Option<CompositionName>,
        crystal: bool,
    }

    impl Spec for Fixed {
        const ID: ScenarioId = name!(ScenarioId, "fixed");
        type Purpose = Conformance;
        type Subject = Port;
        type Params = Params;
        type Criteria = ();
        type Trace = ();
        type Finding = ();

        fn resources(params: &Params) -> Resources {
            let capabilities = if params.crystal {
                vec![BoardCapability::LpClock(LpClockSource::Xtal32k)]
            } else {
                Vec::new()
            };
            Resources {
                boards: vec![BoardNeed {
                    role: BoardRole::Dut,
                    chip: params.chip.clone(),
                    capabilities,
                }],
                fixtures: Vec::new(),
            }
        }

        fn target(params: &Params) -> TargetCapabilities {
            TargetCapabilities {
                chip: params.chip.clone(),
                composition: params
                    .composition
                    .clone()
                    .map_or(Composition::Any, Composition::Product),
                capabilities: BTreeSet::from([name!(CapabilityKey, "conformance.toy")]),
            }
        }

        fn estimate(params: &Params, profile: &StandProfile) -> Result<Estimate, WaitError> {
            let terminal = Wait::contract(TERMINAL_CEILING, Factor::ONE);
            Estimate::from_waits(
                &[],
                &[Wait::measured(params.span.clone()), terminal],
                1,
                profile,
            )
        }

        fn verdict((): &(), (): &()) -> Verdict<()> {
            Verdict::Pass
        }
    }

    fn measured(seconds: u64) -> MeasuredSpan {
        measured_micros(seconds * SECOND)
    }

    fn measured_micros(maximum_micros: u64) -> MeasuredSpan {
        MeasuredSpan {
            maximum_micros,
            samples: 3_000,
            margin: Factor::ONE,
            margin_reason: "test".into(),
            support: Support::Measured {},
            evidence: vec![RunRef {
                run: RunId::parse("1788729094522-000d170b").unwrap(),
                sha256: Sha256([0; 32]),
            }],
        }
    }

    fn raw(spans: &[(&'static str, u64)]) -> RawStandProfile {
        let mut raw = RawStandProfile::default();
        raw.spans
            .insert(name!(SpanName, "image.build"), measured(50));
        raw.spans
            .insert(name!(SpanName, "flash.application"), measured(25));
        raw.spans
            .insert(name!(SpanName, "boot-to-hello"), measured(2));
        for (span, seconds) in spans {
            raw.spans
                .insert(SpanName::parse(span).unwrap(), measured(*seconds));
        }
        raw
    }

    fn profile(spans: &[(&'static str, u64)]) -> StandProfile {
        StandProfile::try_from(raw(spans)).unwrap()
    }

    fn costs() -> StandCosts {
        StandCosts {
            build: name!(SpanName, "image.build"),
            flash: name!(SpanName, "flash.application"),
            boot: name!(SpanName, "boot-to-hello"),
        }
    }

    fn offer() -> StandOffer {
        let board = |chip: ChipId| BoardOffer {
            chip,
            roles: vec![BoardRole::Dut],
            capabilities: vec![BoardCapability::LpClock(LpClockSource::RcSlow)],
        };
        StandOffer {
            boards: vec![
                board(name!(ChipId, "esp32s31")),
                board(name!(ChipId, "esp32c5")),
            ],
            fixtures: Vec::new(),
        }
    }

    fn lease(cap_seconds: u64) -> LeasePolicy {
        LeasePolicy {
            margin: Factor::new(3, NonZeroU32::new(2).unwrap()).unwrap(),
            cap_micros: cap_seconds * SECOND,
        }
    }

    fn on(
        chip: &'static str,
        instance: &'static str,
        span: &'static str,
        composition: Option<&'static str>,
    ) -> Planned {
        Planned::of(&Instance::<Fixed>::new(
            InstanceName::parse(instance).unwrap(),
            Params {
                span: SpanName::parse(span).unwrap(),
                chip: ChipId::parse(chip).unwrap(),
                composition: composition.map(|name| CompositionName::parse(name).unwrap()),
                crystal: false,
            },
        ))
    }

    fn fixed(
        instance: &'static str,
        span: &'static str,
        composition: Option<&'static str>,
    ) -> Planned {
        on("esp32s31", instance, span, composition)
    }

    fn run(
        tier: Tier,
        scenarios: &[Planned],
        profile: &StandProfile,
        cap: u64,
    ) -> Result<Plan, PlanError> {
        let (offer, costs) = (offer(), costs());
        plan(
            tier,
            scenarios,
            &PlanInputs {
                profile,
                offer: &offer,
                costs: &costs,
                lease: lease(cap),
            },
        )
    }

    #[test]
    fn one_image_holds_a_smoke_tier_and_each_instance_boots_once() {
        let profile = profile(&[("toy.case", 10)]);
        let scenarios = [
            fixed("a", "toy.case", None),
            fixed("b", "toy.case", Some(STATION)),
        ];
        let plan = run(Tier::Smoke, &scenarios, &profile, 3_600).unwrap();
        assert_eq!(plan.segments.len(), 1);
        let segment = &plan.segments[0];
        assert_eq!(segment.build_micros, 50 * SECOND);
        // Flash, then per instance: boot 2 s, span 10 s, terminal ceiling 2 s.
        assert_eq!(segment.lease_estimate_micros, (25 + 14 + 14) * SECOND);
        assert_eq!(
            segment.lease_limit_micros,
            segment.lease_estimate_micros * 3 / 2
        );
        assert_eq!(plan.total_micros, (50 + 25 + 28) * SECOND);
        assert!(plan.provisional.is_empty());
    }

    #[test]
    fn a_provisional_stand_span_is_labelled_in_the_plan() {
        let mut raw = raw(&[("toy.case", 10)]);
        let flash = name!(SpanName, "flash.application");
        let mut early = measured(25);
        early.samples = 201;
        early.support = Support::Provisional {
            reason: "first stand measurement".into(),
        };
        raw.spans.insert(flash.clone(), early);
        let profile = StandProfile::try_from(raw).unwrap();
        let plan = run(
            Tier::Smoke,
            &[fixed("a", "toy.case", None)],
            &profile,
            3_600,
        )
        .unwrap();
        assert_eq!(
            plan.provisional,
            vec![Provisional {
                span: flash,
                reason: "first stand measurement".into()
            }]
        );
    }

    #[test]
    fn a_board_without_a_crystal_refuses_the_plan() {
        let profile = profile(&[("toy.case", 10)]);
        let crystal = Planned::of(&Instance::<Fixed>::new(
            name!(InstanceName, "sleep"),
            Params {
                span: name!(SpanName, "toy.case"),
                chip: name!(ChipId, "esp32s31"),
                composition: None,
                crystal: true,
            },
        ));
        let Err(PlanError::Refused(refusals)) = run(Tier::Full, &[crystal], &profile, 3_600) else {
            panic!("refused");
        };
        assert_eq!(refusals.len(), 1);
    }

    #[test]
    fn a_long_tier_splits_into_leases_that_fit_the_cap_and_builds_once() {
        let profile = profile(&[("toy.long", 598)]);
        let scenarios: Vec<_> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|name| fixed(name, "toy.long", Some(STATION)))
            .collect();
        let plan = run(Tier::Full, &scenarios, &profile, 1_200).unwrap();
        assert!(plan.segments.len() > 1);
        assert!(
            plan.segments
                .iter()
                .all(|segment| segment.lease_limit_micros <= 1_200 * SECOND)
        );
        assert_eq!(
            plan.segments
                .iter()
                .filter(|segment| segment.build_micros > 0)
                .count(),
            1
        );
        assert_eq!(
            plan.segments
                .iter()
                .map(|segment| segment.scenarios.len())
                .sum::<usize>(),
            6
        );
    }

    #[test]
    fn the_build_never_shortens_a_lease_margin() {
        // The instance takes 2 s boot + 2_346 s + the 2 s ceiling = 2_350 s.
        // With the 25 s flash its lease is 1.5 × 2_375 s = 3_562.5 s, inside
        // the 3_600 s cap; were the 50 s build inside the lease it would
        // need 1.5 × 2_425 s = 3_637.5 s and the plan would refuse it.
        let profile = profile(&[("toy.long", 2_346)]);
        let plan = run(Tier::Full, &[fixed("a", "toy.long", None)], &profile, 3_600).unwrap();
        let segment = &plan.segments[0];
        assert_eq!(segment.lease_estimate_micros, (25 + 2_350) * SECOND);
        assert_eq!(segment.build_micros, 50 * SECOND);
        assert_eq!(
            segment.lease_limit_micros,
            segment.lease_estimate_micros * 3 / 2
        );
    }

    #[test]
    fn an_unsplittable_scenario_reports_the_lease_it_needs() {
        let profile = profile(&[("toy.soak", 2_394)]);
        assert_eq!(
            run(Tier::Full, &[fixed("a", "toy.soak", None)], &profile, 3_600),
            Err(PlanError::Unsplittable {
                scenario: ScenarioInstance {
                    scenario: name!(ScenarioId, "fixed"),
                    instance: name!(InstanceName, "a")
                },
                lease_micros: (25 + 2_398) * SECOND * 3 / 2,
                cap_micros: 3_600 * SECOND,
            })
        );
    }

    #[test]
    fn an_over_budget_plan_names_its_largest_scenarios() {
        let profile = profile(&[("toy.a", 100), ("toy.b", 200), ("toy.c", 50), ("toy.d", 10)]);
        let scenarios = [
            fixed("a", "toy.a", None),
            fixed("b", "toy.b", None),
            fixed("c", "toy.c", None),
            fixed("d", "toy.d", None),
        ];
        let Err(PlanError::OverBudget { largest, .. }) =
            run(Tier::Smoke, &scenarios, &profile, 3_600)
        else {
            panic!("over budget");
        };
        let names: Vec<_> = largest
            .iter()
            .map(|(id, _)| id.instance.to_string())
            .collect();
        assert_eq!(names, ["b", "a", "c"]);
    }

    #[test]
    fn an_overflowing_scenario_is_named() {
        // The instance's own work (the span and the 2 s ceiling) just fits
        // in 64 bits; its boot does not.
        let mut raw = raw(&[]);
        raw.spans.insert(
            name!(SpanName, "toy.huge"),
            measured_micros(u64::MAX - 2 * SECOND),
        );
        let profile = StandProfile::try_from(raw).unwrap();
        assert_eq!(
            run(Tier::Full, &[fixed("a", "toy.huge", None)], &profile, 3_600),
            Err(PlanError::Overflow {
                scenario: Some(ScenarioInstance {
                    scenario: name!(ScenarioId, "fixed"),
                    instance: name!(InstanceName, "a")
                }),
            })
        );
    }

    #[test]
    fn an_empty_tier_needs_no_stand_span() {
        let profile = StandProfile::try_from(RawStandProfile::default()).unwrap();
        let plan = run(Tier::Smoke, &[], &profile, 3_600).unwrap();
        assert!(plan.segments.is_empty() && plan.provisional.is_empty());
        assert_eq!(plan.total_micros, 0);
    }

    #[test]
    fn compositions_and_chips_get_separate_images() {
        let profile = profile(&[("toy.case", 10)]);
        let scenarios = [
            fixed("a", "toy.case", Some(STATION)),
            fixed("b", "toy.case", Some("toy-bluetooth-system")),
            on("esp32c5", "c", "toy.case", None),
            fixed("d", "toy.case", None),
        ];
        let plan = run(Tier::Full, &scenarios, &profile, 3_600).unwrap();
        assert_eq!(plan.segments.len(), 3);
        assert!(plan.segments.iter().all(|segment| segment.build_micros > 0));
        assert_eq!(plan.segments[2].image.chip, name!(ChipId, "esp32c5"));
    }
}
