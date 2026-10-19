//! Deadlines of the station's state machines on the image's monotonic time.

use oer_time::Instant;

/// The earlier of two optional deadlines.
pub const fn earliest(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.as_micros() <= right.as_micros() {
            left
        } else {
            right
        }),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earliest_keeps_the_sooner_deadline() {
        let soon = Some(Instant::from_micros(1));
        let late = Some(Instant::from_micros(2));
        assert_eq!(earliest(soon, late), soon);
        assert_eq!(earliest(late, soon), soon);
        assert_eq!(earliest(None, late), late);
        assert_eq!(earliest(None, None), None);
    }
}
