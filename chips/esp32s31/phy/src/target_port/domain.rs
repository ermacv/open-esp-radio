//! Protocol-neutral registration of one shared PHY domain.
//!
//! Registration needs only the shared-PHY registers, the platform services
//! used by the target port and the caller's calibration inputs. The
//! concurrent domain runs it through the arbiter's shared-PHY borrow and
//! keeps the resulting [`PhyDomain`] under the arbiter.

use super::*;
use crate::domain::PhyDomain;

use super::concurrent::operations::RegisterFuture;

/// Caller-owned inputs for one production common-PHY registration.
///
/// A valid retained cache seeds cold partial calibration; the transition
/// still republishes every hardware-resident product and returns a fresh
/// cache only after terminal success.
pub struct PhyRegisterConfig {
    calibration_identity: PhyCalibrationIdentity,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl PhyRegisterConfig {
    /// Request one fresh full registration and calibration cache.
    pub const fn new(calibration_identity: PhyCalibrationIdentity) -> Self {
        Self {
            calibration_identity,
            calibration_cache: None,
        }
    }

    /// Supply a retained cache for validated cold partial calibration.
    pub fn with_calibration_cache(mut self, calibration_cache: PhyCalibrationCache) -> Self {
        self.calibration_cache = Some(calibration_cache);
        self
    }

    // Keep construction in the caller's return place; inlining this large
    // owner into the registration poll creates another temporary stack copy.
    #[inline(never)]
    fn into_transition(self) -> PhyRegisterTransition {
        PhyRegisterTransition::with_production_config_and_calibration(
            self.calibration_identity,
            self.calibration_cache,
        )
    }
}

/// Successful registration of one shared PHY domain.
///
/// The domain carries the target-issued registration proof and an empty
/// client set. It does not retain the borrowed platform or registers.
#[must_use = "a registered PHY domain is the unique owner of its epoch"]
pub struct PhyDomainRegistered {
    domain: PhyDomain,
    calibration_cache: Option<PhyCalibrationCache>,
    outcome: PhyRegisterOutcome,
    counters: PhyTargetPortCounters,
}

impl PhyDomainRegistered {
    /// Borrow the registered domain.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Borrow the fresh cache produced by this run, when requested.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    /// Inspect the terminal registration result.
    pub const fn outcome(&self) -> PhyRegisterOutcome {
        self.outcome
    }

    /// Inspect concrete target operation counts for diagnostics.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Move all successful outputs.
    pub fn into_parts(
        self,
    ) -> (
        PhyDomain,
        Option<PhyCalibrationCache>,
        PhyRegisterOutcome,
        PhyTargetPortCounters,
    ) {
        (
            self.domain,
            self.calibration_cache,
            self.outcome,
            self.counters,
        )
    }
}

/// Fail-stop result of a domain registration.
///
/// The borrowed platform and registers return to their owner, but this type
/// exposes no retry or state extractor: any failure may follow an ambiguous
/// hardware edge. [`Self::failure_cleanup_completed`] distinguishes executed
/// terminal cleanup from an epoch requiring shared-PHY escalation.
#[must_use = "failed PHY registration retains the poisoned transition"]
pub struct PhyDomainRegisterFailure {
    transition: PhyRegisterTransition,
    counters: PhyTargetPortCounters,
    error: TargetPhyRegisterError,
}

impl PhyDomainRegisterFailure {
    /// Whether the real target transition completed its failure cleanup.
    /// This does not authorize retry or mint a registered domain.
    pub fn failure_cleanup_completed(&self) -> bool {
        self.transition.failure_cleanup_completed()
    }

    /// Inspect the exact terminal failure.
    pub const fn error(&self) -> TargetPhyRegisterError {
        self.error
    }

    /// Inspect concrete target operations completed before failure.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Inspect the last semantic state for fail-stop diagnostics only.
    pub fn state(&self) -> Option<&PhyState> {
        self.transition.state()
    }
}

/// Run one registration transition through the concrete target port.
///
/// A fresh registration epoch begins on `registers` before the first edge;
/// the returned epoch is the one the completed state must describe.
pub(super) async fn execute_registration<P, R, D, O>(
    timer: &impl oer_cooperate::CooperativeTimer,
    transition: &mut PhyRegisterTransition,
    platform: &mut P,
    registers: &mut R,
    observer: O,
) -> (
    Result<PhyRegisterOutcome, PhyRegisterRunError<PhyTargetPortError>>,
    PhyTargetPortCounters,
    TargetRegistrationWitness,
)
where
    R: PhyInitializationAccess + crate::executor::step::Registers,
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    let grant = registers.begin_registration();
    let mut port =
        TargetPhyRegisterPort::<_, _, D, _, _>::new(platform, registers, timer, observer);
    let result = run_phy_register(transition, &mut port).await;
    (
        result,
        port.counters(),
        TargetRegistrationWitness::new(grant),
    )
}

impl PhyDomain {
    /// Register the shared PHY through the borrowed `registers`.
    ///
    /// This is the protocol-neutral production mint path for a registered
    /// domain; the domain alone grants no register access.
    ///
    /// # Cancellation
    ///
    /// Drive this future to a terminal result once polled. Cancellation may
    /// strand a partially applied hardware edge and never returns
    /// registration proof; the owner of `registers` must stay fail-stop until
    /// hardware reset.
    #[must_use = "PHY registration must be driven to a terminal result"]
    #[allow(
        clippy::result_large_err,
        reason = "fail-stop error retains the allocation-free PHY transition"
    )]
    #[allow(
        private_bounds,
        reason = "the sealed physical routes use the private shared engine"
    )]
    pub fn register<'a, P, R, D, O>(
        timer: &'a impl oer_cooperate::CooperativeTimer,
        platform: &'a mut P,
        registers: &'a mut R,
        config: PhyRegisterConfig,
        observer: O,
    ) -> impl core::future::Future<Output = Result<PhyDomainRegistered, PhyDomainRegisterFailure>> + 'a
    where
        R: PhyInitializationAccess + crate::executor::step::Registers,
        D: PhyShortDelay + 'a,
        O: PhyTargetObserver + 'a,
    {
        RegisterFuture {
            future: async move {
                let mut transition = config.into_transition();
                let (result, counters, witness) = execute_registration::<P, R, D, O>(
                    timer,
                    &mut transition,
                    platform,
                    registers,
                    observer,
                )
                .await;
                finish_registration(transition, result, counters, witness)
            },
        }
    }
}

// Construct the large terminal result in the caller's return place. Keeping
// the consumed transition here avoids a second owner copy in the poll frame.
#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "failure retains the unique transition"
)]
fn finish_registration(
    transition: PhyRegisterTransition,
    result: Result<PhyRegisterOutcome, PhyRegisterRunError<PhyTargetPortError>>,
    counters: PhyTargetPortCounters,
    witness: TargetRegistrationWitness,
) -> Result<PhyDomainRegistered, PhyDomainRegisterFailure> {
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(PhyDomainRegisterFailure {
                transition,
                counters,
                error: TargetPhyRegisterError::Run(error),
            });
        }
    };
    match transition.into_model_parts() {
        Ok((state, calibration_cache)) => Ok(PhyDomainRegistered {
            domain: PhyDomain::from_target_completion(state, witness),
            calibration_cache,
            outcome,
            counters,
        }),
        Err(transition) => Err(PhyDomainRegisterFailure {
            transition,
            counters,
            error: TargetPhyRegisterError::MissingCompletedModelOwner,
        }),
    }
}
