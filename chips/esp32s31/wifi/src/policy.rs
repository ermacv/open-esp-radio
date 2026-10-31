//! The single-channel image still validates the arbiter's complete move budget.
use oer_ieee80211_core_arbiter::{
    ArbiterPolicy, ArbiterPolicyError, CheckedArbiterPolicy, NoReservation,
};
use oer_time::Duration;

/// Fit two publication/retune bounds and the minimum listen window. W3
/// performs no off-channel work once associated; this is the constructor's
/// valid absence budget, independent of the software beacon-loss deadline.
pub fn arbiter(
    publication_bound: Duration,
) -> Result<CheckedArbiterPolicy<NoReservation>, ArbiterPolicyError> {
    let mut policy = ArbiterPolicy::DEFAULT;
    let absence = publication_bound
        .as_micros()
        .checked_add(policy.retune.as_micros())
        .and_then(|one| one.checked_mul(2))
        .and_then(|moves| moves.checked_add(policy.min_listen.as_micros()))
        .ok_or(ArbiterPolicyError::AbsenceBudgetOverflow)?;
    policy.absence = Duration::from_micros(absence);
    policy.checked(publication_bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_long_backend_publication_bound_is_valid_before_any_owner_moves() {
        let bound = Duration::from_millis(250);
        assert!(matches!(
            ArbiterPolicy::DEFAULT.checked::<NoReservation>(bound),
            Err(ArbiterPolicyError::AbsenceTooShort { .. })
        ));
        assert!(arbiter(bound).is_ok());
        assert!(arbiter(Duration::from_secs(1)).is_ok());
        assert!(matches!(
            arbiter(Duration::from_micros(u64::MAX)),
            Err(ArbiterPolicyError::AbsenceBudgetOverflow)
        ));
    }
}
