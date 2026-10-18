//! A rejected enrollment is not a terminal application failure.
use trouble_host::{Error, PairingFailedReason};

pub(super) fn result(result: Result<(), Error>) -> Result<(), Error> {
    match result {
        // Trouble reports the expected SMP outcome even after processing the
        // local decline and attempting to enqueue Pairing Failed. The caller
        // also requests disconnect; this is not proof of on-air delivery.
        Err(Error::Security(PairingFailedReason::NumericComparisonFailed)) => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_expected_local_numeric_decline_is_nonterminal() {
        assert_eq!(result(Ok(())), Ok(()));
        assert_eq!(
            result(Err(Error::Security(
                PairingFailedReason::NumericComparisonFailed
            ))),
            Ok(())
        );
        for error in [
            Error::Disconnected,
            Error::InsufficientSpace,
            Error::InvalidValue,
            Error::Security(PairingFailedReason::AuthenticationRequirements),
        ] {
            assert_eq!(result(Err(error.clone())), Err(error));
        }
    }
}
