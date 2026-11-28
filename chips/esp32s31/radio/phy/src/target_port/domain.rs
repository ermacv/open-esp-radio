//! Protocol-neutral registration of the PHY's state.
//!
//! Registration needs only the shared-PHY registers, the platform services
//! used by the target port and the caller's calibration inputs. The owner's
//! `register` lends it the state in its place.

use super::*;

use crate::executor::boundary::Boundary;

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
/// What a completed registration gives beside its filled place.
pub(super) struct Registration {
    pub(super) calibration_cache: Option<PhyCalibrationCache>,
    pub(super) outcome: PhyRegisterOutcome,
    pub(super) counters: PhyTargetPortCounters,
    pub(super) witness: TargetRegistrationWitness,
}

/// Why a registration did not complete, with the port's counters.
#[derive(Clone, Copy, Debug)]
pub struct PhyRegisterFailed {
    /// The leaf cause.
    pub error: TargetPhyRegisterError,
    /// The port's counters at the failure.
    pub counters: PhyTargetPortCounters,
}

pub(super) async fn execute_registration<P, R, D, O>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + crate::executor::wait::WaitTally
     ),
    transition: &mut PhyRegisterTransition,
    state: &mut PhyState,
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
    // The HAL begins the registration's epoch before its first hardware edge.
    registers.begin_registration();
    let mut port =
        TargetPhyRegisterPort::<_, _, D, _, _>::new(platform, registers, timer, observer);
    let result = run_phy_register(transition, state, &mut port).await;
    (result, port.counters(), TargetRegistrationWitness::new())
}

/// Run the registration over `state`, which its owner's place holds.
#[must_use = "PHY registration must be driven to a terminal result"]
#[allow(
    private_bounds,
    reason = "the sealed physical routes use the private shared engine"
)]
pub(super) fn register_state<'a, P, R, D, O>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + crate::executor::wait::WaitTally
        ),
    platform: &'a mut P,
    registers: &'a mut R,
    state: &'a mut PhyState,
    config: PhyRegisterConfig,
    observer: O,
) -> impl core::future::Future<Output = Result<Registration, PhyRegisterFailed>> + 'a
where
    R: PhyInitializationAccess + crate::executor::step::Registers,
    D: PhyShortDelay + 'a,
    O: PhyTargetObserver + 'a,
{
    Boundary::new(async move {
        let mut transition = config.into_transition();
        let (result, counters, witness) = execute_registration::<P, R, D, O>(
            timer,
            &mut transition,
            state,
            platform,
            registers,
            observer,
        )
        .await;
        finish_registration(transition, result, counters, witness)
    })
}

#[inline(never)]
#[expect(
    clippy::result_large_err,
    reason = "a registration's failure is its report, made once an operation"
)]
fn finish_registration(
    transition: PhyRegisterTransition,
    result: Result<PhyRegisterOutcome, PhyRegisterRunError<PhyTargetPortError>>,
    counters: PhyTargetPortCounters,
    witness: TargetRegistrationWitness,
) -> Result<Registration, PhyRegisterFailed> {
    let outcome = result.map_err(|error| PhyRegisterFailed {
        error: TargetPhyRegisterError::Run(error),
        counters,
    })?;
    match transition.into_completed_cache() {
        Ok(calibration_cache) => Ok(Registration {
            calibration_cache,
            outcome,
            counters,
            witness,
        }),
        Err(_) => Err(PhyRegisterFailed {
            error: TargetPhyRegisterError::MissingCompletedModelOwner,
            counters,
        }),
    }
}
