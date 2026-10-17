//! Direct hardware observation with a finite attempt bound, not a time delay.

/// Read immediately and retry directly, as in the rev0 PBus completion loop.
/// The bound counts observations; it does not promise a minimum elapsed time.
/// Returns false on exhaustion, preserving the caller's typed timeout handling.
#[inline]
pub(crate) fn bounded<E>(mut observe: impl FnMut() -> Result<bool, E>) -> Result<bool, E> {
    for _ in 0..crate::HARDWARE_EDGE_LIMIT {
        if observe()? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_stops_at_first_or_last_allowed_read() {
        for ready_at in [1, 2, crate::HARDWARE_EDGE_LIMIT] {
            let mut reads = 0;
            let result = bounded::<()>(|| {
                reads += 1;
                Ok(reads == ready_at)
            });
            assert_eq!(result, Ok(true));
            assert_eq!(reads, ready_at);
        }
    }

    #[test]
    fn stuck_hardware_exhausts_the_finite_read_budget() {
        let mut reads = 0;
        assert_eq!(
            bounded::<()>(|| {
                reads += 1;
                Ok(false)
            }),
            Ok(false)
        );
        assert_eq!(reads, crate::HARDWARE_EDGE_LIMIT);
    }

    #[test]
    fn observation_error_stops_without_further_access() {
        let mut reads = 0;
        let result = bounded(|| {
            reads += 1;
            if reads == 3 {
                Err("invalid edge")
            } else {
                Ok(false)
            }
        });
        assert_eq!(result, Err("invalid edge"));
        assert_eq!(reads, 3);
    }
}
